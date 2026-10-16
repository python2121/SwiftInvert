// SwiftInvert render chain — the WGSL twin of NegPipeline.metal (macOS) and
// NegPipeline.comp (Linux/Vulkan), compiled by naga at runtime to MSL /
// SPIR-V / HLSL. ONE module, six `@compute` entry points; the numerical
// source of truth is `negkit::reference` (the CPU chain), which every kernel
// here is pinned against at mean < 0.01 / max < 0.04 (neggpu/tests/parity.rs).
//
// Mechanics shared with the GLSL port:
//  - storage buffers of interleaved RGB f32 (RgbImage's own layout — upload
//    and readback are memcpys), 1-D pixel index, bounds-checked against
//    `push.n`;
//  - uniform blocks byte-identical to `neggpu::uniforms` / ShaderTypes.swift
//    (NormUniforms 48 B, CurveUniforms 304 B — tests/layout.rs pins Rust,
//    naga validates this side at pipeline creation, parity pins both).
//
// Where this file deliberately differs from the GLSL/MSL:
//  - `Push` is a 16-byte UNIFORM (push constants are native-only in wgpu);
//    its two spare words carry the render width and an optional row pitch
//    for `encode_u8`, so the packed display frame can be written straight
//    into a 256-byte-aligned texture row layout (the zero-copy egui path);
//  - the dispatch may be 2-D (a 1-D grid tops out at 65535 × 256 ≈ 16.7 MP,
//    short of a 24 MP export): `pixel_index` folds `global_invocation_id.y`
//    back into the flat index through `num_workgroups.x`;
//  - the Contrast Mask's gid → (x, y) split is INTEGER division (the GLSL
//    does it in f32, which loses integer exactness past 2^24 pixels);
//  - the skin-hue wrap uses round-half-away-from-zero like the CPU oracle
//    (the MSL uses `rint`, the GLSL `roundEven`).
// Every binding number is unique across the module; each entry point's
// pipeline layout carries only the bindings that entry point uses.

const WORKGROUP: u32 = 256u;
const PI: f32 = 3.14159265358979323846;

// Per-dispatch scalars: `n` = pixel count, `flags` bit 0 = sRGB re-expression
// of the encoded output (display on an unmanaged canvas / the sRGB export
// leg), `width` = render width in pixels, `row_pitch` = output row pitch in
// PIXELS for `encode_u8` (0 = tightly packed).
struct Push {
    n: u32,
    flags: u32,
    width: u32,
    row_pitch: u32,
}

struct NormUniforms {
    floors: vec4<f32>,
    ceils: vec4<f32>,
    wp_offset: f32,
    bp_offset: f32,
    _pad: vec2<f32>,
}

// Byte-identical to ShaderTypes.swift's CurveUniforms (std140 == C here):
// 8 vec4 (128 B), 24 scalars (→ 224), band_hues 224, band_saturations 240,
// separation_damping 256, skin_protection 260, hue_trim 264,
// auto_highlight 268, mask_scale 272, mask_dims 288, total 304.
struct CurveUniforms {
    pivots: vec4<f32>,
    slopes: vec4<f32>,
    curvatures: vec4<f32>,
    cmy_offsets: vec4<f32>,
    shadow_cmy: vec4<f32>,
    mid_cmy: vec4<f32>,
    highlight_cmy: vec4<f32>,
    d_min_rgb: vec4<f32>,
    toe: f32,
    shoulder: f32,
    toe_width: f32,
    shoulder_width: f32,
    d_max: f32,
    a_toe_base: f32,
    a_sh_base: f32,
    width_ref: f32,
    toe_height: f32,
    sh_height: f32,
    zone_center: f32,
    true_black: f32,
    dark_shadows_lift: f32,
    v_star: f32,
    midtone_gamma: f32,
    gamma_width: f32,
    shadows_lift: f32,
    shadow_contrast: f32,
    highlights_shift: f32,
    highlight_contrast: f32,
    vibrance: f32,
    saturation: f32,
    pre_saturation: f32,
    print_saturation: f32,
    band_hues: vec4<f32>,
    band_saturations: vec4<f32>,
    separation_damping: f32,
    skin_protection: f32,
    hue_trim: f32,
    // Highlight Hold's automatic burn, applied through upstream's Zone
    // Density highlight weight.
    auto_highlight: f32,
    // Contrast Mask: xyz = per-channel pre-curve scale for the plane sample,
    // w = 1 when the mask is active (0 gates every plane read off).
    mask_scale: vec4<f32>,
    // xy = plane dims; zw = RENDER dims (the host fills them before upload —
    // a flat storage buffer has no size to query; the Metal mirror asks its
    // output texture).
    mask_dims: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> nu: NormUniforms;
@group(0) @binding(3) var<uniform> cu: CurveUniforms;
@group(0) @binding(4) var<uniform> push: Push;
// Levels remap (interactive histogram): per channel c, [c*16 + 0..7] anchor
// inputs, [c*16 + 8..15] anchor outputs, [48 + c] anchor count (51 floats,
// padded to 52 on the host).
@group(0) @binding(5) var<storage, read> levels: array<f32>;
// Contrast Mask plane: single-channel floats on the analysis grid. Always
// bound (a 4-byte dummy when the mask is off; `mask_scale.w` gates reads).
@group(0) @binding(6) var<storage, read> mask_plane: array<f32>;
// 4 × 256 histogram bins (R, G, B, Rec.709 luma), raw counts.
@group(0) @binding(7) var<storage, read_write> bins: array<atomic<u32>>;
// Packed RGBA8 display output.
@group(0) @binding(8) var<storage, read_write> dst_u8: array<u32>;

// Flat pixel index from a possibly 2-D dispatch grid.
fn pixel_index(gid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return gid.x + gid.y * nwg.x * WORKGROUP;
}

// ── Shared math helpers (mirror NegPipeline.metal / .comp) ────────────────

fn fast_sigmoid(x: f32) -> f32 {
    if (x >= 0.0) {
        return 1.0 / (1.0 + exp(-x));
    }
    let z = exp(x);
    return z / (1.0 + z);
}

// Numerically stable softplus log(1 + exp(x)).
fn softplus(x: f32) -> f32 {
    return max(x, 0.0) + log(1.0 + exp(-abs(x)));
}

// Working-space OETF (Adobe RGB 1998: pure 563/256 gamma, no linear segment).
fn oetf_encode(t: f32) -> f32 {
    return pow(clamp(t, 0.0, 1.0), 0.45470693);
}

fn log10f(x: f32) -> f32 {
    return log(x) * 0.4342944819032518;
}

// Round half away from zero — the CPU oracle's `f64::round` (WGSL `round`
// is half-to-even).
fn round_away(x: f32) -> f32 {
    return sign(x) * floor(abs(x) + 0.5);
}

// ── CIELAB in the working space (linear Adobe RGB 1998, D65) — mirrors
//    negkit::lab (TO_XYZ / TO_RGB row-major there; column-major here). ────
const WORKING_TO_XYZ: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.5767309, 0.2973769, 0.0270343),
    vec3<f32>(0.1855540, 0.6273491, 0.0706872),
    vec3<f32>(0.1881852, 0.0752741, 0.9911085));
const XYZ_TO_WORKING: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(2.0413690, -0.9692660, 0.0134474),
    vec3<f32>(-0.5649464, 1.8760108, -0.1183897),
    vec3<f32>(-0.3446944, 0.0415560, 1.0154096));
// sRGB (linear, D65) from XYZ — the display/export gamut leg.
const SRGB_FROM_XYZ: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(3.2404542, -0.9692660, 0.0556434),
    vec3<f32>(-1.5371385, 1.8760108, -0.2040259),
    vec3<f32>(-0.4985314, 0.0415560, 1.0572252));
const WORKING_WHITE: vec3<f32> = vec3<f32>(0.95047, 1.0, 1.08883);
const LAB_EPS: f32 = 0.008856;
const LAB_KAPPA: f32 = 7.787;

fn rgb_to_lab(rgb: vec3<f32>) -> vec3<f32> {
    let xyz = (WORKING_TO_XYZ * max(rgb, vec3<f32>(0.0))) / WORKING_WHITE;
    let f = select(LAB_KAPPA * xyz + 16.0 / 116.0, pow(xyz, vec3<f32>(1.0 / 3.0)),
                   xyz > vec3<f32>(LAB_EPS));
    return vec3<f32>(116.0 * f.y - 16.0, 500.0 * (f.x - f.y), 200.0 * (f.y - f.z));
}

fn lab_to_xyz(lab: vec3<f32>) -> vec3<f32> {
    let fy = (lab.x + 16.0) / 116.0;
    let f = vec3<f32>(lab.y / 500.0 + fy, fy, fy - lab.z / 200.0);
    let f3 = f * f * f;
    return select((f - 16.0 / 116.0) / LAB_KAPPA, f3, f3 > vec3<f32>(LAB_EPS)) * WORKING_WHITE;
}

fn lab_to_rgb(lab: vec3<f32>) -> vec3<f32> {
    return max(XYZ_TO_WORKING * lab_to_xyz(lab), vec3<f32>(0.0));
}

// ── Gamut-aware chroma boost + Skin Protection rein (literals mirror
//    negkit::lab — keep in sync) ──────────────────────────────────────────
const SKIN_HUE_CENTER_DEG: f32 = 52.0;
const SKIN_HUE_SIGMA_DEG: f32 = 20.0;
const SKIN_CHROMA_FULL: f32 = 35.0;
const SKIN_CHROMA_ZERO: f32 = 60.0;
const SKIN_L_LO: f32 = 15.0;
const SKIN_L_HI: f32 = 95.0;
const SKIN_CHROMA_GATE: f32 = 2.0;
const SKIN_CEIL_AT_FULL: f32 = 22.0;
const SKIN_KNEE_START_FRAC: f32 = 0.6;
const GAMUT_ITERATIONS: i32 = 10;
const GAMUT_TOL: f32 = 1e-4;

// 0 (not skin) … 1 (dead centre of the skin locus).
fn skin_weight(l_val: f32, a: f32, b: f32) -> f32 {
    let chroma = sqrt(a * a + b * b);
    if (chroma < SKIN_CHROMA_GATE) {
        return 0.0;
    }
    let hue_deg = atan2(b, a) * (180.0 / PI);
    var dist = hue_deg - SKIN_HUE_CENTER_DEG;
    dist -= 360.0 * round_away(dist / 360.0);
    let x = dist / SKIN_HUE_SIGMA_DEG;
    let w_hue = exp(-0.5 * x * x);
    let w_chroma = 1.0 - smoothstep(SKIN_CHROMA_FULL, SKIN_CHROMA_ZERO, chroma);
    let w_light = smoothstep(0.0, SKIN_L_LO, l_val) * (1.0 - smoothstep(SKIN_L_HI, 100.0, l_val));
    return w_hue * w_chroma * w_light;
}

// One-directional soft chroma ceiling inside the skin mask.
fn skin_chroma_rein(lab: vec3<f32>, strength: f32) -> vec3<f32> {
    if (strength <= 0.0) {
        return lab;
    }
    let ceiling = SKIN_CEIL_AT_FULL / strength;
    let start = SKIN_KNEE_START_FRAC * ceiling;
    let span = ceiling - start;
    let chroma = sqrt(lab.y * lab.y + lab.z * lab.z);
    if (chroma <= start) {
        return lab;
    }
    let w = skin_weight(lab.x, lab.y, lab.z);
    if (w <= 0.0) {
        return lab;
    }
    let knee = start + span * (1.0 - exp(-(chroma - start) / span));
    let scale = (chroma + w * (knee - chroma)) / chroma;
    return vec3<f32>(lab.x, lab.y * scale, lab.z * scale);
}

// Raw in-gamut check (no lower clamp — the raw values ARE the decision).
fn in_gamut_lab(l_val: f32, a: f32, b: f32) -> bool {
    let rgb = XYZ_TO_WORKING * lab_to_xyz(vec3<f32>(l_val, a, b));
    return all(rgb >= vec3<f32>(-GAMUT_TOL)) && all(rgb <= vec3<f32>(1.0 + GAMUT_TOL));
}

// Effective boost: the target if it fits, else a soft knee toward the
// bisected in-gamut headroom (caller ensures saturation > 1).
fn gamut_aware_boost(lab: vec3<f32>, saturation: f32) -> f32 {
    if (in_gamut_lab(lab.x, lab.y * saturation, lab.z * saturation)) {
        return saturation;
    }
    var lo = 1.0;
    var hi = saturation;
    let still_ok = in_gamut_lab(lab.x, lab.y, lab.z);
    for (var i = 0; i < GAMUT_ITERATIONS; i++) {
        let mid = (lo + hi) / 2.0;
        if (still_ok && in_gamut_lab(lab.x, lab.y * mid, lab.z * mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s_max = max(lo, 1.0 + GAMUT_TOL);
    let knee = s_max - 1.0;
    return 1.0 + knee * (1.0 - exp(-(saturation - 1.0) / knee));
}

// ── Levels remap (interactive histogram) — mirrors negkit::reference::levels_remap.
fn levels_remap(e: f32, channel: u32) -> f32 {
    let count = min(u32(levels[48u + channel]), 8u);
    if (count == 0u) {
        return e;
    }
    let xs = channel * 16u;
    let ys = xs + 8u;
    var x0 = 0.0;
    var y0 = 0.0;
    for (var i = 0u; i < count; i++) {
        let x1 = levels[xs + i];
        let y1 = levels[ys + i];
        if (e <= x1) {
            return y0 + (e - x0) * (y1 - y0) / max(x1 - x0, 1e-4);
        }
        x0 = x1;
        y0 = y1;
    }
    return y0 + (e - x0) * (1.0 - y0) / max(1.0 - x0, 1e-4);
}

// Separation Damping reference spread — K::SEPARATION_DAMPING_REF_SPREAD.
const SEPARATION_DAMPING_REF: f32 = 0.35;

fn separation_damping_gain(k: f32, damping: f32, chroma: f32) -> f32 {
    if (k <= 0.0) {
        return 0.0;
    }
    let h = (SEPARATION_DAMPING_REF - chroma) / (SEPARATION_DAMPING_REF + chroma);
    return min(pow(k, (1.0 - damping) + damping * h), 3.0);
}

// Colour-mixer band membership: raised-cosine hue window × the shared
// chroma gate (negkit::lab::BAND_CENTERS_DEG / BAND_HALF_WIDTHS_DEG).
fn band_weight(hue_deg: f32, gate: f32, center: f32, half_width: f32) -> f32 {
    let dh = (hue_deg - center + 540.0) % 360.0 - 180.0;
    let t = min(abs(dh) / half_width, 1.0);
    return gate * 0.5 * (1.0 + cos(PI * t));
}

// Region-mask constants — K::TONE_REGION_SHARPNESS / K::*_TONE_ANCHOR.
const TONE_SHARPNESS: f32 = 3.5;
const SHADOW_ANCHOR: f32 = 1.40;
const HIGHLIGHT_ANCHOR: f32 = 0.30;
const DARK_SHADOW_ANCHOR: f32 = 1.85;

// The piecewise sRGB OETF.
fn srgb_encode(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return 12.92 * v;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// Working-space encode + levels remap of one linear pixel, then the
// optional sRGB re-expression (flags bit 0): decode the pure-power TRC,
// gamut matrix, clamp (per-channel clip is fine for a canvas), sRGB OETF.
fn encode_pixel(i: u32) -> vec3<f32> {
    var e = vec3<f32>(
        levels_remap(oetf_encode(src[i]), 0u),
        levels_remap(oetf_encode(src[i + 1u]), 1u),
        levels_remap(oetf_encode(src[i + 2u]), 2u));
    if ((push.flags & 1u) != 0u) {
        let lin = pow(max(e, vec3<f32>(0.0)), vec3<f32>(2.19921875));
        let srgb_lin = clamp(SRGB_FROM_XYZ * (WORKING_TO_XYZ * lin), vec3<f32>(0.0), vec3<f32>(1.0));
        e = vec3<f32>(srgb_encode(srgb_lin.x), srgb_encode(srgb_lin.y), srgb_encode(srgb_lin.z));
    }
    return e;
}

// ═══════════════════════════════ Kernels ══════════════════════════════════

// log10 (lower clip only) → per-channel stretch by the final bounds.
@compute @workgroup_size(256)
fn normalize_log(@builtin(global_invocation_id) gid3: vec3<u32>,
                 @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let i = gid * 3u;
    let epsilon = 1e-6;
    for (var ch = 0u; ch < 3u; ch++) {
        let log_color = log10f(max(src[i + ch], epsilon));
        let f = nu.floors[ch] + nu.wp_offset;
        let c = nu.ceils[ch] + nu.bp_offset;
        let delta = c - f;
        var denom = delta;
        if (abs(delta) < epsilon) {
            denom = select(-epsilon, epsilon, delta >= 0.0);
        }
        dst[i + ch] = (log_color - f) / denom;
    }
}

// Bilinear plane sample at output pixel (x, y): OpenCV's half-pixel-centre
// convention, taps clamped (edge replication). Mirrors ContrastMask.sample
// (Swift) / contrast_mask_sample (MSL, GLSL) — keep the mirrors in lock step.
fn contrast_mask_sample(x: f32, y: f32) -> f32 {
    let dims = cu.mask_dims.xy;
    let pf = (vec2<f32>(x, y) + 0.5) * dims / cu.mask_dims.zw - 0.5;
    let lo = clamp(floor(pf), vec2<f32>(0.0), dims - 1.0);
    let hi = min(lo + 1.0, dims - 1.0);
    let f = clamp(pf - lo, vec2<f32>(0.0), vec2<f32>(1.0));
    let w = u32(dims.x);
    let v00 = mask_plane[u32(lo.y) * w + u32(lo.x)];
    let v10 = mask_plane[u32(lo.y) * w + u32(hi.x)];
    let v01 = mask_plane[u32(hi.y) * w + u32(lo.x)];
    let v11 = mask_plane[u32(hi.y) * w + u32(hi.x)];
    let top = v00 + (v10 - v00) * f.x;
    let bot = v01 + (v11 - v01) * f.x;
    return top + (bot - top) * f.y;
}

// Per-channel quadratic core, held at its vertex on the side where it would
// fold back (NegPy d4dc3e49) — mirrors negkit::reference::quadratic_core.
fn quadratic_core(k: f32, pivot: f32, curv: f32, u_in: f32) -> f32 {
    var u = u_in;
    if (curv != 0.0) {
        let vertex = -k / (2.0 * curv);
        if ((curv > 0.0 && u < vertex) || (curv < 0.0 && u > vertex)) {
            u = vertex;
        }
    }
    return k * (u - pivot) + curv * u * u;
}

// Asymmetric H&D print curve → LINEAR reflectance (mirrors PrintCurve::print_pixel).
@compute @workgroup_size(256)
fn print_curve(@builtin(global_invocation_id) gid3: vec3<u32>,
               @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let i = gid * 3u;
    var color = vec3<f32>(src[i], src[i + 1u], src[i + 2u]);

    // Pre-saturation: scale density deviations from the per-pixel neutral.
    if (cu.pre_saturation != 1.0) {
        let m = (color.r + color.g + color.b) / 3.0;
        color = vec3<f32>(m) + cu.pre_saturation * (color - vec3<f32>(m));
    }

    // Contrast Mask: 1-D gid → (x, y) through the render width (integer
    // split; the GLSL's f32 split is exact only below 2^24 pixels).
    var mask_add = vec3<f32>(0.0);
    if (cu.mask_scale.w != 0.0) {
        let out_w = max(u32(cu.mask_dims.z), 1u);
        let py = gid / out_w;
        let px = gid - py * out_w;
        mask_add = cu.mask_scale.xyz * contrast_mask_sample(f32(px), f32(py));
    }

    let eps = 1e-6;
    let a_hl = cu.a_sh_base * cu.width_ref / max(cu.shoulder_width, eps);
    let a_sh_base = cu.a_toe_base * cu.width_ref / max(cu.toe_width, eps);
    let a_sh = select(a_sh_base * (1.0 - cu.toe * 4.0), a_sh_base, cu.toe >= 0.0);
    let d_min_rgb = cu.d_min_rgb.xyz;
    let d_min_eff = max(d_min_rgb + vec3<f32>(cu.shoulder * cu.sh_height), vec3<f32>(0.0));
    let d_max_base = select(cu.d_max, cu.d_max - cu.toe * cu.toe_height, cu.toe >= 0.0);
    let d_max_eff = max(vec3<f32>(d_max_base), d_min_eff + vec3<f32>(0.1));
    // True Black (BPC) references the PHYSICAL d_max so toe lifts survive.
    let bpc_db = select(cu.d_max, cu.d_max + cu.toe * cu.toe_height, cu.toe < 0.0);
    let bpc_black = pow(10.0, -bpc_db);
    let has_tone = cu.shadows_lift != 0.0 || cu.shadow_contrast != 0.0
        || cu.dark_shadows_lift != 0.0 || cu.highlights_shift != 0.0
        || cu.highlight_contrast != 0.0;
    let has_band_cmy = any(cu.shadow_cmy.xyz != vec3<f32>(0.0))
        || any(cu.mid_cmy.xyz != vec3<f32>(0.0))
        || any(cu.highlight_cmy.xyz != vec3<f32>(0.0));

    var dens = vec3<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch++) {
        let val = color[ch] + cu.cmy_offsets[ch] + mask_add[ch];
        var v = quadratic_core(cu.slopes[ch], cu.pivots[ch], cu.curvatures[ch], val);

        if (cu.midtone_gamma != 0.0) {
            v = v + cu.midtone_gamma * cu.gamma_width * tanh((v - cu.v_star) / cu.gamma_width);
        }

        // Regional tone: sigmoid-masked shifts + anchor-pivoted contrast,
        // parallel form (both masks on the incoming v).
        if (has_tone) {
            let w_s = fast_sigmoid(TONE_SHARPNESS * (v - SHADOW_ANCHOR));
            let w_h = fast_sigmoid(TONE_SHARPNESS * (HIGHLIGHT_ANCHOR - v));
            let w_ds = fast_sigmoid(TONE_SHARPNESS * (v - DARK_SHADOW_ANCHOR));
            v = v + (-cu.shadows_lift + cu.shadow_contrast * (v - SHADOW_ANCHOR)) * w_s
                  - cu.dark_shadows_lift * w_ds
                  + (-cu.highlights_shift + cu.highlight_contrast * (v - HIGHLIGHT_ANCHOR)) * w_h;
        }

        // 3-band colour balance on the tone-region masks (mids = remainder).
        if (has_band_cmy) {
            let w_s = fast_sigmoid(TONE_SHARPNESS * (v - SHADOW_ANCHOR));
            let w_h = fast_sigmoid(TONE_SHARPNESS * (HIGHLIGHT_ANCHOR - v));
            let w_m = max(1.0 - w_s - w_h, 0.0);
            v = v + cu.shadow_cmy[ch] * w_s + cu.mid_cmy[ch] * w_m + cu.highlight_cmy[ch] * w_h;
        }

        // Highlight Hold's automatic burn: upstream's Zone Density highlight
        // term (last density op before the knees). 4.0 / 0.40 mirror
        // K::ZONE_DENSITY_SHARPNESS / −K::ZONE_DENSITY_HIGHLIGHT_OFFSET;
        // zone_center is K::ANCHOR_TARGET_DENSITY.
        if (cu.auto_highlight != 0.0) {
            let w_z = 1.0 - fast_sigmoid(4.0 * (v - (cu.zone_center - 0.40)));
            v = v + cu.auto_highlight * w_z;
        }

        let v1 = d_min_eff[ch] + softplus(a_hl * (v - d_min_eff[ch])) / a_hl;
        dens[ch] = d_max_eff[ch] - softplus(a_sh * (d_max_eff[ch] - v1)) / a_sh;
    }

    // Print Saturation: uniform k around the achromatic mean of density
    // above paper base (Separation Damping makes k a function of the
    // pixel's own RMS spread).
    if (cu.print_saturation != 1.0) {
        let ve = dens - d_min_rgb;
        let m = (ve.x + ve.y + ve.z) / 3.0;
        if (cu.separation_damping > 0.0) {
            let chroma = sqrt(((ve.x - ve.y) * (ve.x - ve.y) + (ve.y - ve.z) * (ve.y - ve.z)
                + (ve.x - ve.z) * (ve.x - ve.z)) / 3.0);
            let k = separation_damping_gain(cu.print_saturation, cu.separation_damping, chroma);
            dens = d_min_rgb + vec3<f32>(m) + k * (ve - vec3<f32>(m));
        } else {
            dens = d_min_rgb + vec3<f32>(m) + cu.print_saturation * (ve - vec3<f32>(m));
        }
    }
    var transmittance = pow(vec3<f32>(10.0), -dens);
    if (cu.true_black != 0.0) {
        transmittance = (transmittance - bpc_black) / (1.0 - bpc_black);
    }
    let outv = clamp(transmittance, vec3<f32>(0.0), vec3<f32>(1.0));
    dst[i] = outv.x;
    dst[i + 1u] = outv.y;
    dst[i + 2u] = outv.z;
}

// Hue Trim → Colour Mixer → vibrance → saturation (+ Skin Protection) in
// CIELAB on the linear print (mirrors negkit::lab::ColorPop::apply_pixel).
@compute @workgroup_size(256)
fn color_pop(@builtin(global_invocation_id) gid3: vec3<u32>,
             @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let i = gid * 3u;
    var result = vec3<f32>(src[i], src[i + 1u], src[i + 2u]);

    // Hue Trim: rotate about the neutral axis in a*b*; runs FIRST.
    if (cu.hue_trim != 0.0) {
        var lab = rgb_to_lab(result);
        let c = cos(cu.hue_trim);
        let s = sin(cu.hue_trim);
        lab = vec3<f32>(lab.x, lab.y * c - lab.z * s, lab.y * s + lab.z * c);
        result = clamp(lab_to_rgb(lab), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    // Colour mixer: chroma-gated hue-targeted bands, R/Y/G/B. All weights
    // read the ORIGINAL hue and compose jointly.
    if (any(cu.band_hues != vec4<f32>(0.0)) || any(cu.band_saturations != vec4<f32>(1.0))) {
        var lab = rgb_to_lab(result);
        let chroma = length(lab.yz);
        let gate = smoothstep(6.0, 16.0, chroma);
        if (gate > 0.0) {
            let hue_deg = atan2(lab.z, lab.y) * (180.0 / PI);
            let w_r = band_weight(hue_deg, gate, 25.0, 45.0);
            let w_y = band_weight(hue_deg, gate, 90.0, 45.0);
            let w_g = band_weight(hue_deg, gate, 135.0, 50.0);
            let w_b = band_weight(hue_deg, gate, 235.0, 65.0);
            var delta_deg = 0.0;
            var chroma_scale = 1.0;
            delta_deg += cu.band_hues.x * 30.0 * w_r;
            chroma_scale *= 1.0 + (cu.band_saturations.x - 1.0) * w_r;
            delta_deg += cu.band_hues.y * 30.0 * w_y;
            chroma_scale *= 1.0 + (cu.band_saturations.y - 1.0) * w_y;
            delta_deg += cu.band_hues.z * 30.0 * w_g;
            chroma_scale *= 1.0 + (cu.band_saturations.z - 1.0) * w_g;
            delta_deg += cu.band_hues.w * 30.0 * w_b;
            chroma_scale *= 1.0 + (cu.band_saturations.w - 1.0) * w_b;
            if (delta_deg != 0.0 || chroma_scale != 1.0) {
                let new_hue = (hue_deg + delta_deg) * (PI / 180.0);
                let new_chroma = chroma * max(chroma_scale, 0.0);
                lab.y = new_chroma * cos(new_hue);
                lab.z = new_chroma * sin(new_hue);
                result = clamp(lab_to_rgb(lab), vec3<f32>(0.0), vec3<f32>(1.0));
            }
        }
    }

    if (cu.vibrance != 1.0) {
        var lab = rgb_to_lab(result);
        let chroma = length(lab.yz);
        let muted = clamp(1.0 - chroma / 60.0, 0.0, 1.0);
        let boost = 1.0 + (cu.vibrance - 1.0) * muted;
        lab.y *= boost;
        lab.z *= boost;
        result = clamp(lab_to_rgb(lab), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if (cu.saturation != 1.0 || cu.skin_protection > 0.0) {
        var lab = rgb_to_lab(result);
        if (cu.saturation != 1.0) {
            // Boosts are gamut-aware; desaturation stays flat.
            var eff = cu.saturation;
            if (cu.saturation > 1.0) {
                eff = gamut_aware_boost(lab, cu.saturation);
            }
            lab.y *= eff;
            lab.z *= eff;
        }
        // Skin Protection: after the scale, independent of it, reduce-only.
        lab = skin_chroma_rein(lab, cu.skin_protection);
        result = clamp(lab_to_rgb(lab), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    dst[i] = result.x;
    dst[i + 1u] = result.y;
    dst[i + 2u] = result.z;
}

// 4×256 bins (R, G, B, Rec.709 luma): reads the LINEAR content and encodes
// internally — levels remap included, so bins are DISPLAYED values.
@compute @workgroup_size(256)
fn histogram(@builtin(global_invocation_id) gid3: vec3<u32>,
             @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let i = gid * 3u;
    let color = vec3<f32>(
        levels_remap(oetf_encode(src[i]), 0u),
        levels_remap(oetf_encode(src[i + 1u]), 1u),
        levels_remap(oetf_encode(src[i + 2u]), 2u));
    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));

    let bin_r = u32(clamp(color.r * 255.0, 0.0, 255.0));
    let bin_g = u32(clamp(color.g * 255.0, 0.0, 255.0));
    let bin_b = u32(clamp(color.b * 255.0, 0.0, 255.0));
    let bin_l = u32(clamp(luma * 255.0, 0.0, 255.0));

    atomicAdd(&bins[bin_r], 1u);
    atomicAdd(&bins[256u + bin_g], 1u);
    atomicAdd(&bins[512u + bin_b], 1u);
    atomicAdd(&bins[768u + bin_l], 1u);
}

// Working-space OETF encode + levels remap, float output (export/tests);
// flags bit 0 = the sRGB export leg.
@compute @workgroup_size(256)
fn encode_f(@builtin(global_invocation_id) gid3: vec3<u32>,
            @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let i = gid * 3u;
    let e = encode_pixel(i);
    dst[i] = e.x;
    dst[i + 1u] = e.y;
    dst[i + 2u] = e.z;
}

// Display fast path: the same encode, packed RGBA8 (alpha 255) — the GPU
// quantizes and readback is 4× smaller. `push.row_pitch` (pixels, 0 =
// tight) lets the frame land directly in a texture's padded row layout.
@compute @workgroup_size(256)
fn encode_u8(@builtin(global_invocation_id) gid3: vec3<u32>,
             @builtin(num_workgroups) nwg: vec3<u32>) {
    let gid = pixel_index(gid3, nwg);
    if (gid >= push.n) {
        return;
    }
    let e = encode_pixel(gid * 3u);
    let packed = pack4x8unorm(vec4<f32>(e, 1.0));
    if (push.row_pitch == 0u) {
        dst_u8[gid] = packed;
    } else {
        let w = max(push.width, 1u);
        let y = gid / w;
        let x = gid - y * w;
        dst_u8[y * push.row_pitch + x] = packed;
    }
}

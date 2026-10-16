//! CPU reference render chain — port of `ReferenceCurve.swift` (NegPy
//! `_normalize_log_image_jit` + `_apply_print_curve_kernel` + `working_oetf_encode`;
//! d4dc3e49 quadratic vertex hold, 8532dd92 Highlight Hold burn / paper-S retune, 0.36 True
//! Black, 7bc8bdc Print Saturation, d86a5aa Separation Damping, 515c1f5 Contrast Mask).
//!
//! This is the numerical source of truth every GPU kernel is pinned against (the Metal and
//! Vulkan mirrors in Swift, the WGSL twin in `neggpu`): mean < 0.01, max < 0.04 vs this chain.
//! It is also the no-GPU renderer (`negsession`'s `CpuRenderer`), so it runs `rayon` over rows
//! behind the crate feature — every pixel is independent and summed in a fixed order, so the
//! parallel split is bit-for-bit the serial result.
//!
//! Passes, in the GPU's order (CLAUDE.md §"GPU render"):
//! 1. [`normalize`] — `log10(max(c, 1e-6))` (lower clip only) → per-channel stretch by the
//!    final bounds, unclamped.
//! 2. [`apply_print_curve`] — pre-saturation → per channel: cmy offsets (+ Contrast Mask plane
//!    sample × scale) → [`quadratic_core`] → paper-S tanh → tone controls → 3-band colour →
//!    Highlight Hold burn → shoulder/toe softplus knees → Print Saturation → `10^−D` →
//!    True Black → clamp → linear reflectance; then the colour-pop stage
//!    ([`crate::lab::ColorPop`]) on the linear print when any of its controls is active — the
//!    Swift `applyPrintCurve` folds the pop into the same per-pixel loop, and so does this port
//!    (the GPU runs it as its own `color_pop` pass on the same buffer).
//! 3. [`encode_output`] — working-space OETF (Adobe RGB 1998 TRC) → [`levels_remap`] →
//!    optionally the kernels' flags-bit sRGB re-expression (decode the TRC → gamut matrix →
//!    piecewise sRGB OETF, AFTER the levels remap, as `encode_f`/`encode_u8` do).
//! 4. [`histogram_256`] — the `histogram` kernel's 4 × 256 raw counts over the encoded output.
//!
//! Precision follows the Swift lane for lane: pixels are `f32`; `normalize` and the encode run
//! their math in `f32` (Swift `Float`, same as the kernels); the print curve widens each pixel
//! to `f64` (Swift `Double`), runs the whole per-pixel chain there and narrows once at the write;
//! `levels_remap` is `f32` (the shader mirror) and `levels_inverse_remap` is `f64` (the
//! interactive histogram's grab math). The sRGB leg has no Swift twin (the Mac tags a CGImage
//! and lets ColorSync convert); it mirrors the GLSL in `f32`.

use crate::constants::{K, WorkingOetf};
use crate::curve_logic::{self as cl};
use crate::derive::RenderParams;
use crate::image::for_each_row_mut;
use crate::lab::{self, ColorPop};
use crate::{LogNegativeBounds, RgbImage, V2, V3};

/// A Contrast Mask plane the print curve can sample — the hook `contrast_mask` plugs into
/// (`ContrastMask.sample(plane, x:y:outWidth:outHeight:)` in Swift: a half-pixel bilinear
/// sample of the plane mapped over the frame being rendered, so proxy/HQ/export renders sample
/// one plane identically). The curve multiplies the sample by `RenderParams::mask_val_scale`
/// and adds it alongside the cmy offsets.
pub trait MaskSampler: Sync {
    /// The plane value under output pixel `(x, y)` of a `out_width × out_height` render.
    fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32;
}

impl<F> MaskSampler for F
where
    F: Fn(usize, usize, usize, usize) -> f32 + Sync,
{
    fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        self(x, y, out_width, out_height)
    }
}

/// Per-render options of [`render`].
#[derive(Clone, Copy, Default)]
pub struct RenderOptions<'a> {
    /// Re-express the encoded output as sRGB (the kernels' flags bit 0: sRGB display on an
    /// unmanaged canvas / the sRGB export leg). Off = Adobe-RGB-encoded working space, which
    /// is what every parity fixture holds.
    pub srgb_encode: bool,
    /// The Contrast Mask plane, when the mask is on (`mask_val_scale` non-zero). `None` with a
    /// non-zero scale renders without the mask, exactly as the Swift does with a nil plane.
    pub mask_plane: Option<&'a dyn MaskSampler>,
}

impl std::fmt::Debug for RenderOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderOptions")
            .field("srgb_encode", &self.srgb_encode)
            .field("mask_plane", &self.mask_plane.is_some())
            .finish()
    }
}

/// The histogram kernel's bins: 4 × 256 raw counts, `[R | G | B | Rec.709 luma]`.
pub type Histogram256 = [u32; 1024];

/// sRGB (linear, D65) from XYZ, row-major: `rgb[i] = dot(SRGB_FROM_XYZ[i], xyz)` — the
/// kernels' `SRGB_FROM_XYZ` (declared there column-wise). Composes with [`lab::TO_XYZ`] for
/// the Adobe → sRGB gamut leg.
pub const SRGB_FROM_XYZ: [V3; 3] = [
    [3.2404542, -1.5371385, -0.4985314],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0556434, -0.2040259, 1.0572252],
];

// ───────────────────────────── 1. normalize ─────────────────────────────

/// `normalize_log_image`: `log10` (lower-clip only, no upper clamp — matches the kernels) →
/// per-channel stretch `(log − floor) / (ceil − floor)`, unclamped. NaN reads as the clip
/// floor, ±∞ as 1 / the floor. `f32` throughout, as the Swift.
pub fn normalize(image: &RgbImage, bounds: &LogNegativeBounds) -> RgbImage {
    let mut out = image.clone();
    normalize_in_place(&mut out, bounds);
    out
}

/// [`normalize`] on a buffer the caller already owns (the big-buffer discipline).
pub fn normalize_in_place(image: &mut RgbImage, bounds: &LogNegativeBounds) {
    let eps: f32 = 1e-6;
    let mut floors = [0.0f32; 3];
    let mut inv_d = [0.0f32; 3];
    for ch in 0..3 {
        // Boundary casts: the Swift narrows the Double bounds to Float here too.
        floors[ch] = bounds.floors[ch] as f32;
        let mut denom = bounds.ceils[ch] as f32 - floors[ch];
        if denom.abs() < eps {
            denom = if denom >= 0.0 { eps } else { -eps };
        }
        inv_d[ch] = 1.0 / denom;
    }
    let row_len = image.width * 3;
    for_each_row_mut(&mut image.data, row_len, |_, row| {
        for px in row.chunks_exact_mut(3) {
            for (ch, lane) in px.iter_mut().enumerate() {
                let mut v = *lane;
                if v.is_nan() {
                    v = eps;
                }
                if v.is_infinite() {
                    v = if v > 0.0 { 1.0 } else { eps };
                }
                let lg = v.max(eps).log10();
                *lane = (lg - floors[ch]) * inv_d[ch];
            }
        }
    });
}

// ───────────────────────────── 2. print curve ─────────────────────────────

/// The per-channel quadratic core `k(u − pivot) + curv·u²`, held at its vertex on the side
/// where it would fold back (NegPy d4dc3e49, `quadratic_core`). The cast-removal clamp bounds
/// the curvature only near the frame's range; input far outside it — holder edges and dust
/// (u ≪ 0), bare light (u > 1) — would otherwise reverse direction in R/B alone and print
/// coloured instead of paper white/black. Mirrored as `quadratic_core` in every kernel.
#[inline(always)]
pub fn quadratic_core(slope: f64, pivot: f64, curvature: f64, u_in: f64) -> f64 {
    let mut u = u_in;
    if curvature != 0.0 {
        let vertex = -slope / (2.0 * curvature);
        if (curvature > 0.0 && u < vertex) || (curvature < 0.0 && u > vertex) {
            u = vertex;
        }
    }
    slope * (u - pivot) + curvature * u * u
}

/// Everything the print curve folds once per render from [`RenderParams`] (the scalars the
/// kernels compute per pixel from their uniforms: knee sharpnesses, effective paper bounds,
/// the BPC black, the tone-control amplitudes and the uniform gates). Built by
/// [`PrintCurve::new`]; [`PrintCurve::print_pixel`] is the per-pixel kernel without the
/// colour-pop stage.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintCurve {
    /// Slopes, pivots, curvatures and pre-curve offsets, copied from the params.
    pub slopes: V3,
    pub pivots: V3,
    pub curvatures: V3,
    pub cmy_offsets: V3,
    /// Shoulder softplus sharpness `a_hl`.
    pub a_hl: f64,
    /// Toe softplus sharpness `a_sh` (steepened by a negative toe).
    pub a_sh: f64,
    /// Uniform paper-base density (the `dMinRGB` lanes; neutral paper).
    pub d_min: f64,
    /// Effective paper white per channel (base + shoulder lift, ≥ 0).
    pub d_min_eff: V3,
    /// Effective paper black per channel (toe-lowered, ≥ white + 0.1).
    pub d_max_eff: V3,
    /// True Black's clip point, `10^−bpcDb` against the PHYSICAL d_max.
    pub bpc_black: f64,
    pub true_black: bool,
    pub v_star: f64,
    pub pre_saturation: f64,
    /// Any tone control off zero (gates the sigmoid masks).
    pub has_tone: bool,
    /// Any band CMY off zero.
    pub has_band_cmy: bool,
    /// Shadows lift in density (`shadows × shadowsMaxLift`).
    pub sh_lift: f64,
    /// Dark Shadows lift in density.
    pub ds_lift: f64,
    /// Shadow contrast gain, floored at the monotone limit.
    pub sh_contrast: f64,
    /// Highlights shift in density.
    pub hi_shift: f64,
    /// Highlight contrast gain.
    pub hi_contrast: f64,
    pub shadow_cmy: V3,
    pub mid_cmy: V3,
    pub highlight_cmy: V3,
    pub auto_highlight: f64,
    pub print_saturation: f64,
    pub separation_damping: f64,
    /// Contrast Mask per-channel scale; all-zero gates the plane sample off.
    pub mask_val_scale: V3,
}

impl PrintCurve {
    /// Fold the per-render scalars (the Swift `applyPrintCurve` prologue).
    pub fn new(params: &RenderParams) -> Self {
        let eps = 1e-6;
        let toe = params.toe_eff * K::TOE_SHOULDER_STRENGTH;
        let shoulder = params.shoulder_eff * K::TOE_SHOULDER_STRENGTH;

        let a_hl =
            K::SHOULDER_SHARPNESS_BASE * K::TOE_SHOULDER_WIDTH_REF / params.shoulder_width.max(eps);
        let mut a_sh =
            K::TOE_SHARPNESS_BASE * K::TOE_SHOULDER_WIDTH_REF / params.toe_width.max(eps);
        let mut d_max_base = K::D_MAX;
        if toe >= 0.0 {
            d_max_base = K::D_MAX - toe * K::TOE_HEIGHT;
        } else {
            a_sh *= 1.0 - toe * 4.0;
        }

        // Neutral paper: d_min_rgb is uniform.
        let d_min = params.d_min.max(0.0);
        let mut d_min_eff = [0.0; 3];
        let mut d_max_eff = [0.0; 3];
        for ch in 0..3 {
            let mut dmn = d_min + shoulder * K::SHOULDER_HEIGHT;
            if dmn < 0.0 {
                dmn = 0.0;
            }
            let mut dmx = d_max_base;
            if dmx < dmn + 0.1 {
                dmx = dmn + 0.1;
            }
            d_min_eff[ch] = dmn;
            d_max_eff[ch] = dmx;
        }
        // True Black (BPC) references the PHYSICAL d_max (not d_max_eff) so toe lifts survive;
        // a negative toe raises the clip point — the softplus bound reaches d_max only
        // asymptotically (mirrors NegPy 0.36).
        let mut bpc_db = K::D_MAX;
        if toe < 0.0 {
            bpc_db = K::D_MAX + toe * K::TOE_HEIGHT;
        }
        let bpc_black = 10f64.powf(-bpc_db);

        let has_tone = params.shadows != 0.0
            || params.shadow_contrast != 0.0
            || params.dark_shadows != 0.0
            || params.highlights != 0.0
            || params.highlight_contrast != 0.0;
        let has_band_cmy = params.shadow_cmy != [0.0; 3]
            || params.mid_cmy != [0.0; 3]
            || params.highlight_cmy != [0.0; 3];

        Self {
            slopes: params.slopes,
            pivots: params.pivots,
            curvatures: params.curvatures,
            cmy_offsets: params.cmy_offsets,
            a_hl,
            a_sh,
            d_min,
            d_min_eff,
            d_max_eff,
            bpc_black,
            true_black: params.true_black,
            v_star: params.v_star,
            pre_saturation: params.pre_saturation,
            has_tone,
            has_band_cmy,
            sh_lift: params.shadows * K::SHADOWS_MAX_LIFT,
            ds_lift: params.dark_shadows * K::SHADOWS_MAX_LIFT,
            sh_contrast: (params.shadow_contrast * K::SHADOW_CONTRAST_MAX)
                .max(K::SHADOW_CONTRAST_NEG_FLOOR),
            hi_shift: params.highlights * K::HIGHLIGHTS_MAX_SHIFT,
            hi_contrast: params.highlight_contrast * K::HIGHLIGHT_CONTRAST_MAX,
            shadow_cmy: params.shadow_cmy,
            mid_cmy: params.mid_cmy,
            highlight_cmy: params.highlight_cmy,
            auto_highlight: params.auto_highlight,
            print_saturation: params.print_saturation,
            separation_damping: params.separation_damping,
            mask_val_scale: params.mask_val_scale,
        }
    }

    /// Whether a plane sample would change anything (`mask_val_scale` non-zero).
    #[inline]
    pub fn mask_active(&self) -> bool {
        self.mask_val_scale != [0.0; 3]
    }

    /// One normalized-log pixel → linear reflectance in `[0, 1]`, the kernel's per-pixel body
    /// up to (not including) the colour-pop stage. `mask_sample` is the Contrast Mask plane
    /// value under this pixel (0 when the mask is off). Math in `f64`; narrows at the return.
    #[inline]
    pub fn print_pixel(&self, rgb: [f32; 3], mask_sample: f32) -> [f32; 3] {
        let mut val = [f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2])];
        // Pre-saturation: scale density deviations from the per-pixel neutral (channel mean)
        // before any print decision. The Swift narrows back to Float between this and the
        // curve; mirrored so the two stay bit-identical.
        if self.pre_saturation != 1.0 {
            let m = (val[0] + val[1] + val[2]) / 3.0;
            for lane in &mut val {
                *lane = f64::from((m + self.pre_saturation * (*lane - m)) as f32);
            }
        }
        let mut mask_add = [0.0f64; 3];
        if self.mask_active() {
            let s = f64::from(mask_sample);
            for (add, scale) in mask_add.iter_mut().zip(&self.mask_val_scale) {
                *add = scale * s;
            }
        }

        let midtone_gamma = K::PAPER_MIDTONE_GAMMA;
        let gamma_width = K::PAPER_GAMMA_WIDTH;
        let mut dens = [0.0f64; 3];
        for ch in 0..3 {
            let u = val[ch] + self.cmy_offsets[ch] + mask_add[ch];
            let mut v = quadratic_core(self.slopes[ch], self.pivots[ch], self.curvatures[ch], u);
            if midtone_gamma != 0.0 {
                v += midtone_gamma * gamma_width * ((v - self.v_star) / gamma_width).tanh();
            }

            // Regional tone: sigmoid-masked density shifts + anchor-pivoted contrast, parallel
            // form (both masks on the incoming v).
            if self.has_tone {
                let w_s = cl::sigmoid(K::TONE_REGION_SHARPNESS * (v - K::SHADOW_TONE_ANCHOR));
                let w_h = cl::sigmoid(K::TONE_REGION_SHARPNESS * (K::HIGHLIGHT_TONE_ANCHOR - v));
                let w_ds = cl::sigmoid(K::TONE_REGION_SHARPNESS * (v - K::DARK_SHADOW_TONE_ANCHOR));
                // ONE expression on the incoming v, like both GPU kernels (GLSL/MSL). The Swift
                // CPU reference applied the three terms sequentially, so its highlight contrast
                // read a v the shadow terms had already moved — up to 0.025 D off the GPU with
                // shadow and highlight controls on together (found in the 2026-10-15 review; no
                // fixture combines them). The GPUs are what users saw: the parallel form is the
                // oracle here.
                v += (-self.sh_lift + self.sh_contrast * (v - K::SHADOW_TONE_ANCHOR)) * w_s
                    - self.ds_lift * w_ds
                    + (-self.hi_shift + self.hi_contrast * (v - K::HIGHLIGHT_TONE_ANCHOR)) * w_h;
            }

            // 3-band colour balance on the same tone-region masks (mids = whatever the
            // shadow/highlight bands don't claim).
            if self.has_band_cmy {
                let w_s = cl::sigmoid(K::TONE_REGION_SHARPNESS * (v - K::SHADOW_TONE_ANCHOR));
                let w_h = cl::sigmoid(K::TONE_REGION_SHARPNESS * (K::HIGHLIGHT_TONE_ANCHOR - v));
                let w_m = (1.0 - w_s - w_h).max(0.0);
                v += self.shadow_cmy[ch] * w_s
                    + self.mid_cmy[ch] * w_m
                    + self.highlight_cmy[ch] * w_h;
            }

            // Highlight Hold's automatic burn: upstream's Zone Density highlight term
            // (8532dd92 rides it), the last density op before the knees — their placement.
            if self.auto_highlight != 0.0 {
                let z_hi = K::ANCHOR_TARGET_DENSITY + K::ZONE_DENSITY_HIGHLIGHT_OFFSET;
                v += self.auto_highlight
                    * (1.0 - cl::sigmoid(K::ZONE_DENSITY_SHARPNESS * (v - z_hi)));
            }

            let v1 =
                self.d_min_eff[ch] + cl::softplus(self.a_hl * (v - self.d_min_eff[ch])) / self.a_hl;
            dens[ch] = self.d_max_eff[ch]
                - cl::softplus(self.a_sh * (self.d_max_eff[ch] - v1)) / self.a_sh;
        }

        // Print Saturation: uniform k around the achromatic mean of density above paper base —
        // the global reduction of NegPy's saturation matrix (identity dye matrix here; NegPy
        // 7bc8bdc).
        if self.print_saturation != 1.0 {
            let ve = [
                dens[0] - self.d_min,
                dens[1] - self.d_min,
                dens[2] - self.d_min,
            ];
            let m = (ve[0] + ve[1] + ve[2]) / 3.0;
            let k = if self.separation_damping > 0.0 {
                // Separation Damping (NegPy d86a5aa): k becomes a function of the pixel's own
                // RMS spread, so the push lands on muted colour and reverses on extreme colour.
                let chroma = (((ve[0] - ve[1]) * (ve[0] - ve[1])
                    + (ve[1] - ve[2]) * (ve[1] - ve[2])
                    + (ve[0] - ve[2]) * (ve[0] - ve[2]))
                    / 3.0)
                    .sqrt();
                cl::separation_damping_gain(self.print_saturation, self.separation_damping, chroma)
            } else {
                self.print_saturation
            };
            for ch in 0..3 {
                dens[ch] = self.d_min + m + k * (ve[ch] - m);
            }
        }

        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let mut t = 10f64.powf(-dens[ch]);
            if self.true_black {
                t = (t - self.bpc_black) / (1.0 - self.bpc_black);
            }
            // Narrowing at the pixel boundary, as the Swift `Float(min(max(t, 0), 1))`.
            out[ch] = t.clamp(0.0, 1.0) as f32;
        }
        out
    }
}

/// `_apply_print_curve_kernel` (C-41 path: no B&W collapse, no dye mix, no EV map; NegPy's
/// 2-band regional CMY generalized to the 3-band tone-mask colour balance), followed by the
/// colour-pop stage on the linear print when any of its controls is active (the Swift folds
/// it into the same loop; the GPU dispatches `color_pop` on the same buffer). Input normalized
/// log; output linear reflectance in `[0, 1]`.
///
/// `mask_plane` is the Contrast Mask plane; it is only sampled when
/// `params.mask_val_scale` is non-zero, and a `None` plane with the mask on renders unmasked
/// (the Swift's nil plane).
pub fn apply_print_curve(
    img: &RgbImage,
    params: &RenderParams,
    mask_plane: Option<&dyn MaskSampler>,
) -> RgbImage {
    let mut out = img.clone();
    apply_print_curve_in_place(&mut out, params, mask_plane);
    out
}

/// [`apply_print_curve`] on a buffer the caller already owns.
pub fn apply_print_curve_in_place(
    img: &mut RgbImage,
    params: &RenderParams,
    mask_plane: Option<&dyn MaskSampler>,
) {
    let curve = PrintCurve::new(params);
    let pop = ColorPop::from_render_params(params);
    let pop_active = pop.is_active();
    let plane = if curve.mask_active() {
        mask_plane
    } else {
        None
    };
    let (width, height) = (img.width, img.height);
    let row_len = width * 3;
    for_each_row_mut(&mut img.data, row_len, |y, row| {
        for (x, px) in row.chunks_exact_mut(3).enumerate() {
            let sample = plane.map_or(0.0, |p| p.sample(x, y, width, height));
            let lin = curve.print_pixel([px[0], px[1], px[2]], sample);
            if pop_active {
                // Colour pop on the linear print (hue trim, mixer bands, then NegPy's lab
                // stage: vibrance, saturation, skin protection), identity at defaults.
                let res =
                    pop.apply_pixel([f64::from(lin[0]), f64::from(lin[1]), f64::from(lin[2])]);
                px[0] = res[0] as f32;
                px[1] = res[1] as f32;
                px[2] = res[2] as f32;
            } else {
                px.copy_from_slice(&lin);
            }
        }
    });
}

// ───────────────────────────── 3. encode ─────────────────────────────

/// Piecewise-linear levels remap through the channel's anchors (endpoints `(0,0)`/`(1,1)`
/// implicit) — mirrors the kernels' `levels_remap` exactly, same float arithmetic order.
/// Anchors must be sanitized (sorted, monotone; [`crate::derive::sanitize_levels`]).
#[inline]
pub fn levels_remap(e: f32, points: &[[f32; 2]]) -> f32 {
    let (mut x0, mut y0) = (0.0f32, 0.0f32);
    for p in points {
        if e <= p[0] {
            return y0 + (e - x0) * (p[1] - y0) / (p[0] - x0).max(1e-4);
        }
        x0 = p[0];
        y0 = p[1];
    }
    y0 + (e - x0) * (1.0 - y0) / (1.0 - x0).max(1e-4)
}

/// Piecewise inverse of [`levels_remap`] (the map is monotone, so this is well-defined):
/// displayed position → input tone under the anchors in effect. The interactive histogram
/// grabs through this — if it drifts from the forward map, planting an anchor makes the tone
/// jump under the cursor. `f64`, as the Swift.
pub fn levels_inverse_remap(y: f64, points: &[V2]) -> f64 {
    let (mut x0, mut y0) = (0.0f64, 0.0f64);
    for p in points {
        if y <= p[1] {
            let dy = p[1] - y0;
            return if dy < 1e-6 {
                p[0]
            } else {
                x0 + (y - y0) * (p[0] - x0) / dy
            };
        }
        x0 = p[0];
        y0 = p[1];
    }
    let dy = 1.0 - y0;
    if dy < 1e-6 {
        1.0
    } else {
        x0 + (y - y0) * (1.0 - x0) / dy
    }
}

/// The kernels' `srgb_encode`: the piecewise sRGB OETF on a linear value in `[0, 1]`.
#[inline]
pub fn srgb_oetf(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The flags-bit sRGB leg of `encode_f` / `encode_u8`, on one Adobe-RGB-encoded (levels
/// included) triplet: decode the pure-power TRC, gamut matrix to linear sRGB, clamp
/// (per-channel clip is fine for a canvas), piecewise sRGB OETF. `f32`, as the GLSL.
#[inline]
pub fn srgb_from_encoded(e: [f32; 3]) -> [f32; 3] {
    let lin = [
        e[0].max(0.0).powf(WorkingOetf::GAMMA_F32),
        e[1].max(0.0).powf(WorkingOetf::GAMMA_F32),
        e[2].max(0.0).powf(WorkingOetf::GAMMA_F32),
    ];
    let mut xyz = [0.0f32; 3];
    for (o, row) in xyz.iter_mut().zip(&lab::TO_XYZ) {
        // Boundary casts: the kernels hold these matrices as f32 literals.
        *o = row[0] as f32 * lin[0] + row[1] as f32 * lin[1] + row[2] as f32 * lin[2];
    }
    let mut out = [0.0f32; 3];
    for (o, row) in out.iter_mut().zip(&SRGB_FROM_XYZ) {
        let s = row[0] as f32 * xyz[0] + row[1] as f32 * xyz[1] + row[2] as f32 * xyz[2];
        *o = srgb_oetf(s.clamp(0.0, 1.0));
    }
    out
}

/// Linear sRGB (D65) → XYZ, row-major — the PUBLISHED inverse of [`SRGB_FROM_XYZ`]
/// (IEC 61966-2-1), agreeing with it to ~1e-7 per entry; [`encoded_from_srgb_f64`] inverts the
/// forward matrix numerically instead, so its round trip is exact.
pub const XYZ_FROM_SRGB: [V3; 3] = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];

/// Inverse of the piecewise sRGB OETF ([`srgb_oetf`]), in `f64` (see [`encoded_from_srgb`]).
#[inline]
pub fn srgb_eotf(e: f64) -> f64 {
    if e <= 0.04045 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`srgb_from_encoded`]: an sRGB-encoded display triplet back to the Adobe-RGB
/// working encode — inverse piecewise OETF → XYZ ([`XYZ_FROM_SRGB`]) → linear Adobe RGB
/// ([`lab::TO_RGB`]) → the pure-power TRC. For a front end that shows sRGB bytes (no
/// ColorSync) and wants to read density off them. The math runs in `f64` and narrows once at
/// the end, so against an f64 forward the round trip is exact to ≤ 1e-6; against the kernels'
/// own `f32` forward it holds to ≤ 1e-5 — the residual is the forward's f32 cancellation in the
/// gamut matrix on a dark channel beside a bright one, amplified by the inverse TRC's slope
/// near black (≈ ×16 at e = 0.05); on 8-bit display bytes it is invisible. In-gamut colours only
/// (the forward clamp admits nothing else back); negative linear values clamp at 0 before the
/// TRC.
#[inline]
pub fn encoded_from_srgb(s: [f32; 3]) -> [f32; 3] {
    let w = encoded_from_srgb_f64([f64::from(s[0]), f64::from(s[1]), f64::from(s[2])]);
    // Boundary narrowing: the display path is f32.
    [w[0] as f32, w[1] as f32, w[2] as f32]
}

/// The `f64` core of [`encoded_from_srgb`]. Exact to ≤ 1e-6 against an `f64` forward because
/// it inverts the FORWARD's own matrices numerically ([`inverse3`]) rather than using the
/// published rounded inverses ([`XYZ_FROM_SRGB`], [`lab::TO_RGB`]): those agree with the
/// forward only to ~1e-7 per entry, which a bright channel turns into ~1e-7 absolute in a dark
/// channel's tiny linear value — 2.8e-6 after the inverse TRC. The `f32` interface's own
/// rounding costs about as much again, hence the ≤ 1e-5 bound on [`encoded_from_srgb`].
#[inline]
pub fn encoded_from_srgb_f64(s: [f64; 3]) -> [f64; 3] {
    let xyz_from_srgb = inverse3(&SRGB_FROM_XYZ);
    let rgb_from_xyz = inverse3(&lab::TO_XYZ);
    let lin = [srgb_eotf(s[0]), srgb_eotf(s[1]), srgb_eotf(s[2])];
    let mut xyz = [0.0f64; 3];
    for (o, row) in xyz.iter_mut().zip(&xyz_from_srgb) {
        *o = row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2];
    }
    let mut out = [0.0f64; 3];
    for (o, row) in out.iter_mut().zip(&rgb_from_xyz) {
        let a = row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2];
        *o = a.max(0.0).powf(1.0 / WorkingOetf::GAMMA);
    }
    out
}

/// Exact inverse of a 3×3 row-major matrix (adjugate ÷ determinant). The colour matrices are
/// well conditioned; a singular input (never these constants) returns the identity rather than
/// NaNs.
pub fn inverse3(m: &[V3; 3]) -> [V3; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det == 0.0 || !det.is_finite() {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let inv = 1.0 / det;
    [
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ]
}

/// Working-space OETF encode over a whole buffer (final engine step), with the optional
/// per-channel display-domain levels anchors (interactive histogram). Each channel's anchors
/// narrow to `f32` once, as the Swift. The output is Adobe-RGB-encoded working space — the
/// fixtures' `output.bin`.
pub fn encode_output(img: &RgbImage, levels: &[Vec<V2>; 3]) -> RgbImage {
    encode_output_with(img, levels, false)
}

/// [`encode_output`] with the kernels' flags bit: `srgb` re-expresses the result as sRGB
/// after the levels remap (the remap is a display-domain op on the working encode).
pub fn encode_output_with(img: &RgbImage, levels: &[Vec<V2>; 3], srgb: bool) -> RgbImage {
    let mut out = img.clone();
    encode_output_in_place(&mut out, levels, srgb);
    out
}

/// [`encode_output_with`] on a buffer the caller already owns.
pub fn encode_output_in_place(img: &mut RgbImage, levels: &[Vec<V2>; 3], srgb: bool) {
    let row_len = img.width * 3;
    if levels.iter().all(|l| l.is_empty()) && !srgb {
        for_each_row_mut(&mut img.data, row_len, |_, row| {
            for lane in row {
                *lane = WorkingOetf::encode(*lane);
            }
        });
        return;
    }
    let pts: [Vec<[f32; 2]>; 3] = [
        narrow_points(&levels[0]),
        narrow_points(&levels[1]),
        narrow_points(&levels[2]),
    ];
    for_each_row_mut(&mut img.data, row_len, |_, row| {
        for px in row.chunks_exact_mut(3) {
            let mut e = [0.0f32; 3];
            for ch in 0..3 {
                let enc = WorkingOetf::encode(px[ch]);
                e[ch] = if pts[ch].is_empty() {
                    enc
                } else {
                    levels_remap(enc, &pts[ch])
                };
            }
            if srgb {
                e = srgb_from_encoded(e);
            }
            px.copy_from_slice(&e);
        }
    });
}

#[inline]
fn narrow_points(points: &[V2]) -> Vec<[f32; 2]> {
    // Boundary cast: the kernels read the anchors as f32 uniforms.
    points.iter().map(|p| [p[0] as f32, p[1] as f32]).collect()
}

// ───────────────────────────── 4. histogram ─────────────────────────────

/// CPU twin of the `histogram` kernel: 4 × 256 raw counts (R, G, B, Rec.709 luma) over the
/// ENCODED output (working-space OETF + levels remap — i.e. what [`encode_output`] returns
/// without the sRGB flag; the kernel encodes the linear content internally to the same
/// values). Bin = `clamp(v·255, 0, 255)` truncated, luma on the encoded triplet in `f32`.
/// Counts are raw: consumers normalize to the peak bin and compress with a FIXED constant
/// (the HistogramView shape-only rule). Rows bin in parallel under `rayon`; the integer
/// reduction is order-independent, so the result is identical either way.
pub fn histogram_256(encoded: &RgbImage) -> Histogram256 {
    let row_len = encoded.width * 3;
    if row_len == 0 {
        return [0; 1024];
    }
    let bin_row = |row: &[f32]| -> Histogram256 {
        let mut bins = [0u32; 1024];
        for px in row.chunks_exact(3) {
            let luma = px[0] * 0.2126 + px[1] * 0.7152 + px[2] * 0.0722;
            for (c, v) in [px[0], px[1], px[2], luma].into_iter().enumerate() {
                // Truncating cast on a value already clamped to [0, 255] — the kernels' uint().
                let bin = (v * 255.0).clamp(0.0, 255.0) as usize;
                if let Some(b) = bins.get_mut(c * 256 + bin) {
                    *b += 1;
                }
            }
        }
        bins
    };
    let merge = |mut a: Histogram256, b: Histogram256| -> Histogram256 {
        for (x, y) in a.iter_mut().zip(b) {
            *x += y;
        }
        a
    };
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        encoded
            .data
            .par_chunks_exact(row_len)
            .map(bin_row)
            .reduce(|| [0; 1024], merge)
    }
    #[cfg(not(feature = "rayon"))]
    {
        encoded
            .data
            .chunks_exact(row_len)
            .map(bin_row)
            .fold([0; 1024], merge)
    }
}

// ───────────────────────────── 5. render ─────────────────────────────

/// Full CPU render: normalize → print curve (+ colour pop) → encode, one allocation. The
/// Swift `render` also builds the Contrast Mask plane from the render input when the mask is
/// on; here the plane arrives through [`RenderOptions::mask_plane`] (built by
/// `contrast_mask`, cached by the session) so this module stays plane-format-agnostic.
pub fn render(
    linear_image: &RgbImage,
    params: &RenderParams,
    options: RenderOptions<'_>,
) -> RgbImage {
    let mut out = linear_image.clone();
    render_in_place(&mut out, params, options);
    out
}

/// [`render`] on a buffer the caller already owns (the linear input is overwritten).
pub fn render_in_place(image: &mut RgbImage, params: &RenderParams, options: RenderOptions<'_>) {
    normalize_in_place(image, &params.final_bounds);
    apply_print_curve_in_place(image, params, options.mask_plane);
    encode_output_in_place(image, &params.levels_points, options.srgb_encode);
}

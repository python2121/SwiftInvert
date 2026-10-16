//! CIELAB colour operations on the linear print — port of `LabColor.swift`.
//!
//! CIELAB in the working space (linear Adobe RGB (1998), D65 — NegPy `b3490eb`, which moved the
//! pipeline off ProPhoto because the chain ASSIGNS primaries to sensor-native data at output and
//! ProPhoto's imaginary primaries inflated chroma and skewed hues) and the colour-pop operators
//! that run on it, in the order the `color_pop` kernel applies them:
//!
//! 1. **Hue Trim** (NegPy `7a07f5c`) — a rotation of a*/b* about the neutral axis. Corrects a
//!    scanning light that ROTATES hues rather than casting them; the origin is a fixed point, so
//!    neutrals cannot move and it can never fight the cast removal. Runs FIRST so every control
//!    below acts on corrected hues (upstream rides it on the exposure pass ahead of their Lab
//!    stage).
//! 2. **Colour Mixer** — SwiftInvert-only (no NegPy equivalent): chroma-gated, hue-targeted
//!    R/Y/G/B bands.
//! 3. **Vibrance** — muted-chroma boost (NegPy's `/60`; upstream DELETED theirs in `de79e13`, ours
//!    is SwiftInvert-maintained and the `lab_color` vibrance fixture is frozen at the `0369b10`
//!    dump).
//! 4. **Saturation** — a flat a*/b* scale, **gamut-aware on boosts** (NegPy `1b900ab`: a soft
//!    knee toward each pixel's bisected in-gamut headroom instead of the per-channel clamp that
//!    shifted hue; the in-boost skin term was removed in `bfcd90a`). Desaturation stays flat.
//! 5. **Skin Protection** (NegPy `bfcd90a` + `fb94aed`) — a one-directional soft chroma ceiling
//!    inside the measured CIELAB skin locus, after the scale and independent of it.
//!
//! Precision: every per-pixel function works on `[f64; 3]`, exactly like the Swift's
//! `SIMD3<Double>`; the only `f32` touch points are the whole-image entry points, which widen
//! each lane on read and narrow on write (the Swift `apply` does the same). The GPU mirrors
//! (`NegPipeline.metal` `colorPop`; the WGSL port) run the identical formulas in `f32` and read
//! their literals from the `pub const`s below — keep them in sync.

use crate::{RenderParams, RgbImage, V3, V4};

// ── Working space ↔ XYZ (Adobe RGB (1998) primaries, D65 white; upstream `_WORKING_TO_XYZ`) ──

/// Linear Adobe RGB (1998) → XYZ (D65), row-major: `xyz[i] = dot(TO_XYZ[i], rgb)`.
pub const TO_XYZ: [V3; 3] = [
    [0.5767309, 0.1855540, 0.1881852],
    [0.2973769, 0.6273491, 0.0752741],
    [0.0270343, 0.0706872, 0.9911085],
];
/// XYZ (D65) → linear Adobe RGB (1998), row-major: `rgb[i] = dot(TO_RGB[i], xyz)`.
pub const TO_RGB: [V3; 3] = [
    [2.0413690, -0.5649464, -0.3446944],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0134474, -0.1183897, 1.0154096],
];
/// The D65 reference white XYZ the Lab encode normalizes against.
pub const WORKING_WHITE: V3 = [0.95047, 1.00000, 1.08883];
/// CIELAB linear-segment threshold (OpenCV float-scale constants: L 0–100).
pub const LAB_EPS: f64 = 0.008856;
/// CIELAB linear-segment slope.
pub const LAB_KAPPA: f64 = 7.787;
/// Chroma below which vibrance considers a colour "muted" (NegPy: `/60`).
pub const VIBRANCE_CHROMA_RANGE: f64 = 60.0;

// ── Colour mixer bands (chroma-gated, hue-targeted) ──────────────────────────────────────────
// Membership = raised-cosine hue window around each band centre × a chroma ramp that reaches
// zero at the neutral axis — whites/grays and faint casts are untouched by construction (the
// complement of the WB sliders, which move every pixel). SwiftInvert-only; NegPy has no
// equivalent. Band order everywhere: Red, Yellow, Green, Blue. Tuned empirically on real
// inverted scans, not colorimetric primaries: colourful blues land at Lab hue ~200–250 in this
// pipeline (sky/cyan-blue), and typical colourful content sits at chroma 12–30 (p90 of whole
// frames ~15–30), so the gate must saturate well below the ~60-chroma of an sRGB primary.

/// Band hue centres in degrees, order R/Y/G/B.
pub const BAND_CENTERS_DEG: V4 = [25.0, 90.0, 135.0, 235.0];
/// Band raised-cosine half-widths in degrees, order R/Y/G/B.
pub const BAND_HALF_WIDTHS_DEG: V4 = [45.0, 45.0, 50.0, 65.0];
/// Chroma below which every band fully protects the pixel (the shared gate's floor).
pub const BAND_CHROMA_GATE_LOW: f64 = 6.0;
/// Chroma above which band membership is at full strength (the shared gate's ceiling).
pub const BAND_CHROMA_GATE_HIGH: f64 = 16.0;
/// Hue rotation at slider ±1 (degrees; + rotates toward the next band counterclockwise:
/// red→orange, yellow→green, green→teal, blue→purple).
pub const BAND_MAX_HUE_SHIFT_DEG: f64 = 30.0;

// ── Gamut-aware chroma boost (NegPy 1b900ab, skin term removed in bfcd90a) ───────────────────
// A flat a*/b* scale preserves hue, but overshooting pixels get hard-clamped per RGB channel
// afterward — clamping only the channel(s) that overshot changes the R:G:B ratio, shifting the
// visible hue. The boost path therefore soft-knees toward each pixel's true in-gamut headroom
// (bisection against the real RGB cube) instead of clamping; in-gamut pixels get the exact flat
// scale. Desaturation stays flat.

/// Bisection steps when searching a boosted pixel's in-gamut headroom.
pub const GAMUT_ITERATIONS: usize = 10;
/// Slack on the `[0, 1]` cube for the in-gamut decision, and the floor of the knee's headroom.
pub const GAMUT_TOLERANCE: f64 = 1e-4;

// ── Skin Protection (NegPy bfcd90a + fb94aed) ────────────────────────────────────────────────
// A one-directional soft chroma ceiling inside the measured CIELAB skin locus, applied after the
// saturation scale and independent of it — it also reins skin that arrived over-chromatic from
// the print curve. Chroma is only ever reduced; a* and b* scale together, so hue and L* never
// move. The mask: hue Gaussian around 52° (σ 20), a one-sided chroma window (full ≤35, zero ≥60
// — skin runs C*≈12–40; pure red ≈104 drops out entirely), and a lightness rolloff (L* 15…95).

/// Centre of the skin hue Gaussian (Lab hue, degrees).
pub const SKIN_HUE_CENTER_DEG: f64 = 52.0;
/// σ of the skin hue Gaussian (degrees).
pub const SKIN_HUE_SIGMA_DEG: f64 = 20.0;
/// Chroma up to which the skin window is fully open.
pub const SKIN_CHROMA_FULL: f64 = 35.0;
/// Chroma from which the skin window is fully closed.
pub const SKIN_CHROMA_ZERO: f64 = 60.0;
/// L* at which the lightness rolloff reaches full weight (smoothstep 0…15).
pub const SKIN_LIGHT_LO: f64 = 15.0;
/// L* from which the lightness rolloff starts falling (smoothstep 95…100).
pub const SKIN_LIGHT_HI: f64 = 95.0;
/// Near-neutral pixels (chroma below this) gate to 0: their hue angle is noise.
pub const SKIN_CHROMA_GATE: f64 = 2.0;
/// Chroma ceiling at full strength; the rein pulls toward `ceiling / strength`.
pub const SKIN_CEILING_AT_FULL: f64 = 22.0;
/// The softplus knee starts at this fraction of the ceiling.
pub const SKIN_KNEE_START_FRACTION: f64 = 0.6;

#[inline]
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn clamp01(v: V3) -> V3 {
    [
        v[0].clamp(0.0, 1.0),
        v[1].clamp(0.0, 1.0),
        v[2].clamp(0.0, 1.0),
    ]
}

#[inline]
fn chroma_of(lab: V3) -> f64 {
    (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
}

/// Hermite smoothstep: 0 below `e0`, 1 above `e1` (the mixer's chroma gate and the skin
/// mask's windows).
#[inline]
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear working RGB → CIELAB (D65), OpenCV float scale (L 0–100). Negative input lanes are
/// clamped to 0 first.
pub fn rgb_to_lab(rgb: V3) -> V3 {
    let lin = [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)];
    let mut f = [0.0; 3];
    for (i, lane) in f.iter_mut().enumerate() {
        let x = dot(TO_XYZ[i], lin) / WORKING_WHITE[i];
        *lane = if x > LAB_EPS {
            x.cbrt()
        } else {
            LAB_KAPPA * x + 16.0 / 116.0
        };
    }
    [
        116.0 * f[1] - 16.0,
        500.0 * (f[0] - f[1]),
        200.0 * (f[1] - f[2]),
    ]
}

/// Lab → XYZ (D65) with no clamping: shared by the decode and the gamut decision.
#[inline]
fn lab_to_xyz(lab: V3) -> V3 {
    let fy = (lab[0] + 16.0) / 116.0;
    let f = [lab[1] / 500.0 + fy, fy, fy - lab[2] / 200.0];
    let mut xyz = [0.0; 3];
    for (i, lane) in xyz.iter_mut().enumerate() {
        let f3 = f[i] * f[i] * f[i];
        *lane = if f3 > LAB_EPS {
            f3
        } else {
            (f[i] - 16.0 / 116.0) / LAB_KAPPA
        } * WORKING_WHITE[i];
    }
    xyz
}

/// CIELAB (D65) → linear working RGB (lower-clamped at 0, no upper clip).
pub fn lab_to_rgb(lab: V3) -> V3 {
    let xyz = lab_to_xyz(lab);
    [
        dot(TO_RGB[0], xyz).max(0.0),
        dot(TO_RGB[1], xyz).max(0.0),
        dot(TO_RGB[2], xyz).max(0.0),
    ]
}

/// Hue Trim (NegPy 7a07f5c): rotate one linear pixel's colour about the neutral axis by
/// `radians` in the working CIELAB a*b* plane. L* is untouched and the origin is a fixed point,
/// so neutrals cannot move — which is why this can never fight the cast removal in analysis.
///
/// Corrects a scanning light that ROTATES hues (narrowband or odd phosphor) rather than casting
/// them; white balance cannot, because no grey is wrong. Runs FIRST in the colour-pop stage, so
/// the mixer, vibrance and saturation all see corrected hues — matching upstream, where it rides
/// the exposure pass ahead of their Lab look stage. Exactly identity (the input is returned
/// untouched, not round-tripped) at 0 radians.
pub fn apply_hue_trim(rgb: V3, radians: f64) -> V3 {
    if radians == 0.0 {
        return rgb;
    }
    let mut lab = rgb_to_lab(rgb);
    let (s, c) = radians.sin_cos();
    let (a, b) = (lab[1], lab[2]);
    lab[1] = a * c - b * s;
    lab[2] = a * s + b * c;
    clamp01(lab_to_rgb(lab))
}

/// Per-band hue rotation + chroma scale on one linear pixel (band order R/Y/G/B; `hues` in
/// slider units ±1 → ±[`BAND_MAX_HUE_SHIFT_DEG`], `saturations` with 1 = identity). All band
/// weights read the ORIGINAL hue and compose jointly, so overlapping feathers are
/// order-independent; the shared chroma gate keeps neutrals and faint casts untouched. Exactly
/// identity at the defaults and for pixels below the gate.
pub fn apply_color_mixer(rgb: V3, hues: V4, saturations: V4) -> V3 {
    if hues == [0.0; 4] && saturations == [1.0; 4] {
        return rgb;
    }
    let mut lab = rgb_to_lab(rgb);
    let chroma = chroma_of(lab);
    let gate = smoothstep(BAND_CHROMA_GATE_LOW, BAND_CHROMA_GATE_HIGH, chroma);
    if gate <= 0.0 {
        return rgb;
    }
    let hue_deg = lab[2].atan2(lab[1]).to_degrees();
    let mut delta_deg = 0.0;
    let mut chroma_scale = 1.0;
    for i in 0..4 {
        // `%` on f64 is fmod — the Swift `truncatingRemainder`.
        let dh = (hue_deg - BAND_CENTERS_DEG[i] + 540.0) % 360.0 - 180.0;
        let t = (dh.abs() / BAND_HALF_WIDTHS_DEG[i]).min(1.0);
        let w = gate * 0.5 * (1.0 + (std::f64::consts::PI * t).cos());
        delta_deg += hues[i] * BAND_MAX_HUE_SHIFT_DEG * w;
        chroma_scale *= 1.0 + (saturations[i] - 1.0) * w;
    }
    if delta_deg == 0.0 && chroma_scale == 1.0 {
        return rgb;
    }
    let new_hue = (hue_deg + delta_deg).to_radians();
    let new_chroma = chroma * chroma_scale.max(0.0);
    lab[1] = new_chroma * new_hue.cos();
    lab[2] = new_chroma * new_hue.sin();
    clamp01(lab_to_rgb(lab))
}

/// Skin-locus membership, 0 (not skin) … 1 (dead centre): hue Gaussian × one-sided chroma
/// window × lightness rolloff. Near-neutral pixels (chroma below [`SKIN_CHROMA_GATE`]) gate to
/// 0 — their hue angle is noise.
pub fn skin_weight(l: f64, a: f64, b: f64) -> f64 {
    let chroma = (a * a + b * b).sqrt();
    if chroma < SKIN_CHROMA_GATE {
        return 0.0;
    }
    let hue_deg = b.atan2(a).to_degrees();
    let mut dist = hue_deg - SKIN_HUE_CENTER_DEG;
    dist -= 360.0 * (dist / 360.0).round();
    let x = dist / SKIN_HUE_SIGMA_DEG;
    let w_hue = (-0.5 * x * x).exp();
    let w_chroma = 1.0 - smoothstep(SKIN_CHROMA_FULL, SKIN_CHROMA_ZERO, chroma);
    let w_light = smoothstep(0.0, SKIN_LIGHT_LO, l) * (1.0 - smoothstep(SKIN_LIGHT_HI, 100.0, l));
    w_hue * w_chroma * w_light
}

/// The Skin Protection rein on a Lab triplet: a soft chroma ceiling (`22 / strength`, softplus
/// knee from 0.6 × ceiling) blended by the skin weight; a* and b* scale together so hue and L*
/// never move, and chroma only ever falls. Identity at strength ≤ 0, below the knee start, and
/// outside the skin locus.
pub fn skin_chroma_rein(lab: V3, strength: f64) -> V3 {
    if strength <= 0.0 {
        return lab;
    }
    let ceiling = SKIN_CEILING_AT_FULL / strength;
    let start = SKIN_KNEE_START_FRACTION * ceiling;
    let span = ceiling - start;
    let chroma = chroma_of(lab);
    if chroma <= start {
        return lab;
    }
    let w = skin_weight(lab[0], lab[1], lab[2]);
    if w <= 0.0 {
        return lab;
    }
    let knee = start + span * (1.0 - (-(chroma - start) / span).exp());
    let scale = (chroma + w * (knee - chroma)) / chroma;
    [lab[0], lab[1] * scale, lab[2] * scale]
}

/// Whether `(L, a, b)` decodes to linear working RGB within `[0, 1]` on all channels
/// (± [`GAMUT_TOLERANCE`]). Unlike [`lab_to_rgb`], no lower clamp — the raw values ARE the
/// gamut decision.
pub fn in_gamut_lab(l: f64, a: f64, b: f64) -> bool {
    let xyz = lab_to_xyz([l, a, b]);
    let lo = -GAMUT_TOLERANCE;
    let hi = 1.0 + GAMUT_TOLERANCE;
    TO_RGB.iter().all(|row| {
        let v = dot(*row, xyz);
        v >= lo && v <= hi
    })
}

/// The effective boost for one pixel: the full push if it already lands in gamut, else a
/// softplus-style knee toward the bisected in-gamut headroom. Pure gamut math — the skin term
/// was removed upstream (bfcd90a); Skin Protection is its own operator now. Caller ensures
/// `saturation > 1`.
pub fn gamut_aware_boost(lab: V3, saturation: f64) -> f64 {
    if in_gamut_lab(lab[0], lab[1] * saturation, lab[2] * saturation) {
        // Full push already fits — use it directly (bisecting here would misread the search's
        // own upper bound as a constraint and throttle pixels that were never going to clip).
        return saturation;
    }
    let (mut lo, mut hi) = (1.0, saturation);
    let still_ok = in_gamut_lab(lab[0], lab[1], lab[2]);
    for _ in 0..GAMUT_ITERATIONS {
        let mid = (lo + hi) / 2.0;
        if still_ok && in_gamut_lab(lab[0], lab[1] * mid, lab[2] * mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s_max = lo.max(1.0 + GAMUT_TOLERANCE);
    let knee = s_max - 1.0;
    1.0 + knee * (1.0 - (-(saturation - 1.0) / knee).exp())
}

/// Vibrance, then saturation (+ Skin Protection) on one linear pixel — the NegPy LabProcessor
/// order. Vibrance boosts muted chroma toward the strength; saturation scales all chroma
/// (gamut-aware on boosts, flat on desaturation); the skin rein runs after the scale and
/// independent of it (reduce-only, so it can't overshoot the gamut the knee fitted). Each stage
/// returns clipped to `[0, 1]` like NegPy's per-op clip. Exactly identity when
/// `vibrance == 1 && saturation == 1 && skin_protection <= 0`.
pub fn apply_vibrance_saturation(
    rgb: V3,
    vibrance: f64,
    saturation: f64,
    skin_protection: f64,
) -> V3 {
    let mut color = rgb;
    if vibrance != 1.0 {
        let mut lab = rgb_to_lab(color);
        let muted = (1.0 - chroma_of(lab) / VIBRANCE_CHROMA_RANGE).clamp(0.0, 1.0);
        let boost = 1.0 + (vibrance - 1.0) * muted;
        lab[1] *= boost;
        lab[2] *= boost;
        color = clamp01(lab_to_rgb(lab));
    }
    if saturation != 1.0 || skin_protection > 0.0 {
        let mut lab = rgb_to_lab(color);
        if saturation != 1.0 {
            // Boosts are gamut-aware (1b900ab); desaturation never overshoots, so it stays
            // the exact flat scale.
            let eff = if saturation > 1.0 {
                gamut_aware_boost(lab, saturation)
            } else {
                saturation
            };
            lab[1] *= eff;
            lab[2] *= eff;
        }
        lab = skin_chroma_rein(lab, skin_protection);
        color = clamp01(lab_to_rgb(lab));
    }
    color
}

/// The colour-pop stage's parameters, in the order the kernel applies them. `Default` is the
/// identity (every NegPy parity fixture runs with it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPop {
    /// Hue Trim in radians (0 = off).
    pub hue_trim: f64,
    /// Colour-mixer hue shifts, R/Y/G/B lanes, slider units ±1.
    pub band_hues: V4,
    /// Colour-mixer chroma scales, R/Y/G/B lanes (1 = identity).
    pub band_saturations: V4,
    /// Vibrance (1 = identity).
    pub vibrance: f64,
    /// Saturation (1 = identity; > 1 gamut-aware).
    pub saturation: f64,
    /// Skin Protection strength (0 = off; the app default is 0.5).
    pub skin_protection: f64,
}

impl Default for ColorPop {
    fn default() -> Self {
        Self {
            hue_trim: 0.0,
            band_hues: [0.0; 4],
            band_saturations: [1.0; 4],
            vibrance: 1.0,
            saturation: 1.0,
            skin_protection: 0.0,
        }
    }
}

impl ColorPop {
    /// The stage's slice of a derived [`RenderParams`].
    pub fn from_render_params(p: &RenderParams) -> Self {
        Self {
            hue_trim: p.hue_trim,
            band_hues: p.band_hues,
            band_saturations: p.band_saturations,
            vibrance: p.vibrance,
            saturation: p.saturation,
            skin_protection: p.skin_protection,
        }
    }

    /// Whether any operator is off its identity — the `ReferenceCurve` gate (the GPU dispatches
    /// the pass under the same condition).
    pub fn is_active(&self) -> bool {
        self.hue_trim != 0.0
            || self.band_hues != [0.0; 4]
            || self.band_saturations != [1.0; 4]
            || self.vibrance != 1.0
            || self.saturation != 1.0
            || self.skin_protection > 0.0
    }

    /// One linear pixel through the whole stage in the Swift / kernel order: Hue Trim → Colour
    /// Mixer → vibrance → saturation → Skin Protection. Returns the input untouched when
    /// nothing is active.
    pub fn apply_pixel(&self, rgb: V3) -> V3 {
        if !self.is_active() {
            return rgb;
        }
        let rgb = apply_hue_trim(rgb, self.hue_trim);
        let rgb = apply_color_mixer(rgb, self.band_hues, self.band_saturations);
        apply_vibrance_saturation(rgb, self.vibrance, self.saturation, self.skin_protection)
    }

    /// Whole-buffer application in place (the CPU reference path). Lanes widen to `f64` for the
    /// math and narrow back — the Swift `apply`'s precision.
    pub fn apply_in_place(&self, img: &mut RgbImage) {
        if !self.is_active() {
            return;
        }
        let row_len = img.width * 3;
        if row_len == 0 {
            return;
        }
        let per_row = |row: &mut [f32]| {
            for px in row.chunks_exact_mut(3) {
                let rgb = [f64::from(px[0]), f64::from(px[1]), f64::from(px[2])];
                let out = self.apply_pixel(rgb);
                // Narrowing at the image boundary, as the Swift `Float(res.x)`.
                px[0] = out[0] as f32;
                px[1] = out[1] as f32;
                px[2] = out[2] as f32;
            }
        };
        #[cfg(feature = "rayon")]
        {
            use rayon::prelude::*;
            img.data.par_chunks_mut(row_len).for_each(per_row);
        }
        #[cfg(not(feature = "rayon"))]
        {
            img.data.chunks_mut(row_len).for_each(per_row);
        }
    }
}

/// Whole-buffer vibrance + saturation (+ Skin Protection) — the Swift `LabColor.apply`, which
/// the `lab_color` fixtures exercise. Returns the input unchanged (a clone) when every control
/// is at identity.
pub fn apply(img: &RgbImage, vibrance: f64, saturation: f64, skin_protection: f64) -> RgbImage {
    let mut out = img.clone();
    ColorPop {
        vibrance,
        saturation,
        skin_protection,
        ..ColorPop::default()
    }
    .apply_in_place(&mut out);
    out
}

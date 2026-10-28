//! Auto-exposure meters — port of `Meters.swift`, all consuming the shared prefiltered log
//! grid. Ports of the `*_from_log` meters in `negpy/features/exposure/normalization.py`:
//! the textured-cell activity gate (NegPy 8532dd92, Boyack & Juenger US 5,724,456), the
//! anchor / shadow-point / highlight-point trio, the textural range, the shadow refs and the
//! two-pass neutral axis (NegPy 0.43 / 127bcd7) measured against the PRE-trim base bounds
//! (NegPy 2125a34).
//!
//! Precision mirrors the Swift: per-pixel values are accumulated in `f64` and stored as `f32`
//! exactly where upstream's float32 arrays sit (normalized luma, the normalized triplets the
//! neutral axis ranks, chroma), so the quantile thresholds compare the same rounded values.
//!
//! Owner: wave 2 / agent B.

use std::borrow::Cow;

use crate::bounds::{self, LogNegativeBounds, SortedChannels};
use crate::constants::K;
use crate::stats;
use crate::{RgbImage, V3};

/// Run two independent closures, in parallel under the `rayon` feature, serially otherwise.
#[inline]
fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    #[cfg(feature = "rayon")]
    {
        rayon::join(a, b)
    }
    #[cfg(not(feature = "rayon"))]
    {
        (a(), b())
    }
}

/// `f` over every interleaved RGB triple of `data`, collected in order (parallel under the
/// `rayon` feature — rayon's indexed collect preserves order, so the result is bit-identical
/// to the serial map). A trailing partial triple is ignored, as `chunks_exact` does.
#[inline]
pub(crate) fn map_pixels<U, F>(data: &[f32], f: F) -> Vec<U>
where
    U: Send,
    F: Fn(&[f32]) -> U + Sync + Send,
{
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        data.par_chunks_exact(3).map(f).collect()
    }
    #[cfg(not(feature = "rayon"))]
    {
        data.chunks_exact(3).map(f).collect()
    }
}

/// `f` over every element of `src`, collected in order (parallel under `rayon`, see
/// [`map_pixels`]).
#[inline]
fn map_slice<T, U, F>(src: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync + Send,
{
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        src.par_iter().map(f).collect()
    }
    #[cfg(not(feature = "rayon"))]
    {
        src.iter().map(f).collect()
    }
}

/// Neutral-axis measurement: per-channel median raw-log refs at three luma bands over each
/// band's lowest-chroma pixels (`measure_neutral_axis_from_log`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeutralAxisRefs {
    pub mid: V3,
    pub shadow: V3,
    /// `None` → callers fit a 2-point line.
    pub highlight: Option<V3>,
    pub confidence: f64,
}

/// The three bounds-normalized textured-luma meters (see [`textured_luma_meters`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexturedLumaMeters {
    /// `measure_anchor_from_log` — the auto-exposure key.
    pub anchor: f64,
    /// `measure_shadow_point_from_log` — P99, the tone Shadow Reach prints at paper black.
    pub shadow_point: f64,
    /// `measure_highlight_point_from_log` — P2, the tone Highlight Hold keeps off paper white.
    pub highlight_point: f64,
}

/// Floors and reciprocal spans of `bounds`, the span guarded away from zero (`±1e-6`).
#[inline]
fn floors_and_inverse_spans(bounds: &LogNegativeBounds) -> (V3, V3) {
    let eps = 1e-6;
    let mut inv_d = [0.0; 3];
    for (ch, inv) in inv_d.iter_mut().enumerate() {
        let mut denom = bounds.ceils[ch] - bounds.floors[ch];
        if denom.abs() < eps {
            denom = if denom >= 0.0 { eps } else { -eps };
        }
        *inv = 1.0 / denom;
    }
    (bounds.floors, inv_d)
}

/// Normalized luma of the grid under `bounds` (shared by the anchor trio and the neutral
/// axis): `f64` per pixel, stored as `f32` like upstream's float32 normalized image.
pub fn normalized_luma(grid: &RgbImage, bounds: &LogNegativeBounds) -> Vec<f32> {
    let (f, inv_d) = floors_and_inverse_spans(bounds);
    map_pixels(&grid.data, |p| {
        let r = (f64::from(p[0]) - f[0]) * inv_d[0];
        let g = (f64::from(p[1]) - f[1]) * inv_d[1];
        let b = (f64::from(p[2]) - f[2]) * inv_d[2];
        // Boundary cast: upstream stores the normalized image as float32.
        (K::LUMA_R * r + K::LUMA_G * g + K::LUMA_B * b) as f32
    })
}

/// `_textured_cells` (NegPy 8532dd92, Boyack & Juenger US 5,724,456): the cells of a luma
/// grid that carry detail. The grid is tiled into sectors of 2×2 blocks of
/// [`K::ACTIVITY_BLOCK`] cells; a sector votes when its four block means span more than
/// [`K::ACTIVITY_GATE_DENSITY`]. Block means average grain out, and a flat sector beside a
/// textured one stays out because only its own blocks are compared. Falls back to every cell
/// (borrowed — no copy) when the grid is smaller than one sector or too few sectors pass. The
/// gate reads whatever units `lum` is in (log D for the textural meter, normalized luma for
/// the anchor/points — upstream's own behaviour).
pub fn textured_cells(lum: &[f32], width: usize, height: usize) -> Cow<'_, [f32]> {
    let b = K::ACTIVITY_BLOCK;
    let sector = 2 * b;
    let hs = (height / sector) * sector;
    let ws = (width / sector) * sector;
    if hs == 0 || ws == 0 {
        return Cow::Borrowed(lum);
    }

    // Per-block means over the top-left hs×ws core (f64 accumulation; numpy's float32
    // pairwise mean agrees to ~1 ulp over 64 taps).
    let (by, bx) = (hs / b, ws / b);
    // Boundary cast: a constant block area.
    let block_area = (b * b) as f64;
    let mut blocks: Vec<f32> = Vec::with_capacity(by * bx);
    for j in 0..by {
        for i in 0..bx {
            let mut sum = 0.0f64;
            for y in (j * b)..((j + 1) * b) {
                let row = y * width + i * b;
                if let Some(span) = lum.get(row..row + b) {
                    for &v in span {
                        sum += f64::from(v);
                    }
                }
            }
            // Boundary cast: the block mean is stored as float32 upstream.
            blocks.push((sum / block_area) as f32);
        }
    }
    let block = |j: usize, i: usize| -> f32 { blocks.get(j * bx + i).copied().unwrap_or(0.0) };

    // Sector vote: max−min of the 2×2 block means against the gate.
    let (sy, sx) = (by / 2, bx / 2);
    let mut active: Vec<bool> = Vec::with_capacity(sy * sx);
    let mut active_count = 0usize;
    for j in 0..sy {
        for i in 0..sx {
            let a = block(2 * j, 2 * i);
            let c = block(2 * j, 2 * i + 1);
            let d = block(2 * j + 1, 2 * i);
            let e = block(2 * j + 1, 2 * i + 1);
            let span = f64::from(a.max(c).max(d.max(e)) - a.min(c).min(d.min(e)));
            let votes = span > K::ACTIVITY_GATE_DENSITY;
            active.push(votes);
            if votes {
                active_count += 1;
            }
        }
    }
    // Boundary casts: sector counts, for the fraction test.
    if (active_count as f64) / ((sy * sx) as f64) < K::ACTIVITY_MIN_FRACTION {
        return Cow::Borrowed(lum);
    }

    let mut kept: Vec<f32> = Vec::with_capacity(active_count * 4 * b * b);
    for y in 0..hs {
        let sj = (y / sector) * sx;
        for i in 0..sx {
            if active.get(sj + i).copied().unwrap_or(false) {
                let x0 = y * width + i * sector;
                if let Some(span) = lum.get(x0..x0 + sector) {
                    kept.extend_from_slice(span);
                }
            }
        }
    }
    Cow::Owned(kept)
}

/// The three bounds-normalized textured-luma meters, sharing one gate and one sort (upstream
/// re-grids per meter; the inputs are identical):
/// - `anchor` (`measure_anchor_from_log`): the mean of the P5–P95 trimmed window and its
///   midpoint, averaged, partially pulled toward [`K::ASSUMED_ANCHOR`] and clamped to
///   ±[`K::ANCHOR_METER_BAND`] — a skewed histogram is placed by its detail-bearing span, not
///   its median.
/// - `shadow_point` (`measure_shadow_point_from_log`): P99, the tone Shadow Reach prints at
///   paper black.
/// - `highlight_point` (`measure_highlight_point_from_log`): P2, the tone Highlight Hold keeps
///   off paper white.
///
/// `None` on an empty grid.
pub fn textured_luma_meters(
    grid: &RgbImage,
    bounds: &LogNegativeBounds,
) -> Option<TexturedLumaMeters> {
    let luma = normalized_luma(grid, bounds);
    let lum = textured_cells(&luma, grid.width, grid.height);
    let sorted = stats::sorted_ascending(&lum);

    let clip = K::ANCHOR_TRIM_CLIP;
    let lo = stats::percentile_of_sorted(&sorted, clip)?;
    let hi = stats::percentile_of_sorted(&sorted, 100.0 - clip)?;
    // Mean over the inclusive [lo, hi] window — the sorted array makes it a contiguous run
    // (same value set as numpy's boolean select; the f64 accumulator makes the sum
    // order-independent).
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for &v in sorted.iter() {
        let d = f64::from(v);
        if d < lo {
            continue;
        }
        if d > hi {
            break;
        }
        sum += d;
        count += 1;
    }
    let inner = if count > 0 {
        // Boundary cast: a sample count.
        sum / count as f64
    } else {
        0.5 * (lo + hi)
    };
    let measured = 0.5 * (inner + 0.5 * (lo + hi));
    let pulled = K::ASSUMED_ANCHOR + K::ANCHOR_METER_STRENGTH * (measured - K::ASSUMED_ANCHOR);
    let anchor = pulled.clamp(
        K::ASSUMED_ANCHOR - K::ANCHOR_METER_BAND,
        K::ASSUMED_ANCHOR + K::ANCHOR_METER_BAND,
    );

    Some(TexturedLumaMeters {
        anchor,
        shadow_point: stats::percentile_of_sorted(&sorted, K::SHADOW_REACH_PERCENTILE)?,
        highlight_point: stats::percentile_of_sorted(&sorted, K::HIGHLIGHT_HOLD_PERCENTILE)?,
    })
}

/// `measure_anchor_from_log` (see [`textured_luma_meters`] — kept as the narrow entry point
/// for callers that only want the exposure key).
pub fn anchor(grid: &RgbImage, bounds: &LogNegativeBounds) -> Option<f64> {
    textured_luma_meters(grid, bounds).map(|m| m.anchor)
}

/// `measure_textural_range_from_log`: |P90 − P10| of raw log luma over the textured cells
/// (8532dd92 — rebate and flat sky no longer set grade). `None` on an empty grid.
pub fn textural_range(grid: &RgbImage) -> Option<f64> {
    let lum: Vec<f32> = map_pixels(&grid.data, |p| {
        // Boundary cast: upstream's float32 luma plane.
        (K::LUMA_R * f64::from(p[0]) + K::LUMA_G * f64::from(p[1]) + K::LUMA_B * f64::from(p[2]))
            as f32
    });
    // Through `stats` so this shares the radix sort with every other meter (a bare `.sort()`
    // left this the slowest line in prepare).
    let sorted = stats::sorted_ascending(&textured_cells(&lum, grid.width, grid.height));
    let lo = stats::percentile_of_sorted(&sorted, K::TEXTURAL_RANGE_CLIP)?;
    let hi = stats::percentile_of_sorted(&sorted, 100.0 - K::TEXTURAL_RANGE_CLIP)?;
    Some((hi - lo).abs())
}

/// `measure_shadow_refs_from_log`: P98 per channel of the raw log grid. `None` on an empty grid.
pub fn shadow_refs(grid: &RgbImage) -> Option<V3> {
    shadow_refs_sorted(&bounds::sorted_channels(grid))
}

/// Same refs from pre-sorted channels (bit-identical: the percentile is deterministic on the
/// sorted array, which is a pure function of the grid — shared with `bounds` so `prepare`
/// sorts once).
pub fn shadow_refs_sorted(channels: &SortedChannels) -> Option<V3> {
    let p = K::SHADOW_NEUTRAL_PERCENTILE;
    Some([
        stats::percentile_of_sorted(&channels[0], p)?,
        stats::percentile_of_sorted(&channels[1], p)?,
        stats::percentile_of_sorted(&channels[2], p)?,
    ])
}

/// Pairwise-RMS distance from the neutral axis (rotation-symmetric around grey, so the
/// near-neutral ranking is hue-uniform — max−min scores an opposed R/B split double a
/// same-side deviation). `_rms_chroma`.
#[inline]
pub fn rms_chroma(r: f64, g: f64, b: f64) -> f64 {
    (((r - g) * (r - g) + (g - b) * (g - b) + (r - b) * (r - b)) / 3.0).sqrt()
}

/// One band's result inside [`neutral_axis`]: refs, median kept chroma, kept count.
struct BandRefs {
    refs: V3,
    median_chroma: f64,
    count: usize,
}

/// `measure_neutral_axis_from_log`. `bounds` is the PRE-trim base bounds (NegPy 2125a34: the
/// film's cast is a source property; user WP/BP trims don't perturb it — their GPU always
/// measured pre-trim, and the CPU side was standardized to match).
///
/// Two passes (NegPy 0.43): pass 1 selects through the residual cast under a loose cap (a
/// strong but correctable cast must not collapse the axis; saturated content still fails
/// it), then the affine R/B→G correction implied by its mid+shadow refs re-ranks chroma so
/// pass 2 selects true neutrals under the strict cap. Confidence combines the grey sets'
/// corrected tightness, the midtone sample size and mid↔shadow deviation agreement.
/// `None` when either the mid or the shadow band has no trustworthy neutrals (either pass).
pub fn neutral_axis(grid: &RgbImage, bounds: &LogNegativeBounds) -> Option<NeutralAxisRefs> {
    let n = grid.pixel_count();
    let eps = 1e-6;
    let (f, inv_d) = floors_and_inverse_spans(bounds);

    // Normalized triplets (f32, mirroring NegPy's float32 normalize_log_image output), then
    // luma and pass-1 RMS chroma read from the ROUNDED norm values — upstream computes them
    // from the float32 normalized image.
    let norm: Vec<[f32; 3]> = map_pixels(&grid.data, |p| {
        // Boundary casts: the float32 normalized image.
        [
            ((f64::from(p[0]) - f[0]) * inv_d[0]) as f32,
            ((f64::from(p[1]) - f[1]) * inv_d[1]) as f32,
            ((f64::from(p[2]) - f[2]) * inv_d[2]) as f32,
        ]
    });
    let luma: Vec<f32> = map_slice(&norm, |p| {
        (K::LUMA_R * f64::from(p[0]) + K::LUMA_G * f64::from(p[1]) + K::LUMA_B * f64::from(p[2]))
            as f32
    });
    let chroma: Vec<f32> = map_slice(&norm, |p| {
        rms_chroma(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])) as f32
    });

    // Band membership depends only on the (immutable) luma — compute each band's indices
    // ONCE (one pass for all three) and reuse across both passes.
    let (mb, sb, hb) = (
        K::NEUTRAL_AXIS_MID_BAND,
        K::NEUTRAL_AXIS_SHADOW_BAND,
        K::NEUTRAL_AXIS_HIGHLIGHT_BAND,
    );
    let (mut mb_idx, mut sb_idx, mut hb_idx) = (
        Vec::with_capacity(n / 4),
        Vec::with_capacity(n / 4),
        Vec::with_capacity(n / 4),
    );
    {
        // Boundary casts: the band edges compare against float32 luma.
        let edges = |band: (f64, f64)| (band.0 as f32, band.1 as f32);
        let (mb_f, sb_f, hb_f) = (edges(mb), edges(sb), edges(hb));
        for (i, &l) in luma.iter().enumerate() {
            if l >= mb_f.0 && l <= mb_f.1 {
                mb_idx.push(i);
            }
            if l >= sb_f.0 && l <= sb_f.1 {
                sb_idx.push(i);
            }
            if l >= hb_f.0 && l <= hb_f.1 {
                hb_idx.push(i);
            }
        }
    }

    let band_refs = |indices: &[usize], chroma_vals: &[f32], cap: f64| -> Option<BandRefs> {
        if indices.len() < K::NEUTRAL_AXIS_MIN_PIXELS {
            return None;
        }
        let band_chroma: Vec<f32> = indices
            .iter()
            .map(|&i| chroma_vals.get(i).copied().unwrap_or(0.0))
            .collect();
        // Boundary cast: the quantile threshold is compared at float32 like upstream.
        let thr = stats::quantile(&band_chroma, K::NEUTRAL_AXIS_CHROMA_QUANTILE)? as f32;
        // Order-preserving subset (matches np.nonzero(band)[0][band_chroma <= thr]); gather
        // the kept chroma and channel values in the same pass.
        let mut kept_chroma: Vec<f32> = Vec::with_capacity(indices.len() / 2);
        let mut kept: [Vec<f32>; 3] = [
            Vec::with_capacity(indices.len() / 2),
            Vec::with_capacity(indices.len() / 2),
            Vec::with_capacity(indices.len() / 2),
        ];
        for (&i, &c) in indices.iter().zip(&band_chroma) {
            if c <= thr {
                let Some(p) = grid.data.get(i * 3..i * 3 + 3) else {
                    continue;
                };
                kept_chroma.push(c);
                kept[0].push(p[0]);
                kept[1].push(p[1]);
                kept[2].push(p[2]);
            }
        }
        let near_neutral_chroma = if kept_chroma.is_empty() {
            cap
        } else {
            stats::median(&kept_chroma)?
        };
        if kept_chroma.len() < K::NEUTRAL_AXIS_MIN_PIXELS || near_neutral_chroma > cap {
            return None;
        }
        // Three independent sorts — side by side under `rayon`.
        let (r, (g, b)) = join(
            || stats::median(&kept[0]),
            || join(|| stats::median(&kept[1]), || stats::median(&kept[2])),
        );
        Some(BandRefs {
            refs: [r?, g?, b?],
            median_chroma: near_neutral_chroma,
            count: kept_chroma.len(),
        })
    };

    // Raw-log refs → normalized space under the same bounds (`_norm_ref`).
    let norm_ref = |refs: V3| -> V3 {
        let mut out = [0.0; 3];
        for ch in 0..3 {
            let mut denom = bounds.ceils[ch] - bounds.floors[ch];
            if denom.abs() < eps {
                denom = if denom >= 0.0 { eps } else { -eps };
            }
            out[ch] = (refs[ch] - bounds.floors[ch]) / denom;
        }
        out
    };

    // The band measurements are independent (each sorts its own subset), so the pairs run
    // side by side under `rayon`; the results are bit-identical either way.
    let (mid1, sh1) = join(
        || band_refs(&mb_idx, &chroma, K::NEUTRAL_AXIS_FIRST_PASS_CAP),
        || band_refs(&sb_idx, &chroma, K::NEUTRAL_AXIS_FIRST_PASS_CAP),
    );
    let (mid1, sh1) = (mid1?, sh1?);

    // Affine R/B→G correction implied by the pass-1 mid+shadow refs, then re-ranked chroma
    // (corrected channels stored at f32, matching the float32 array upstream writes into).
    let (nm, ns) = (norm_ref(mid1.refs), norm_ref(sh1.refs));
    let mut a_c = [1.0; 3];
    let mut b_c = [0.0; 3];
    for ch in [0usize, 2] {
        let du = nm[ch] - ns[ch];
        if du.abs() < eps {
            a_c[ch] = 1.0;
            b_c[ch] = nm[1] - nm[ch];
        } else {
            a_c[ch] = (nm[1] - ns[1]) / du;
            b_c[ch] = nm[1] - a_c[ch] * nm[ch];
        }
    }
    let chroma2: Vec<f32> = map_slice(&norm, |p| {
        // Boundary casts: the corrected float32 channels.
        let r = (a_c[0] * f64::from(p[0]) + b_c[0]) as f32;
        let g = p[1];
        let b = (a_c[2] * f64::from(p[2]) + b_c[2]) as f32;
        rms_chroma(f64::from(r), f64::from(g), f64::from(b)) as f32
    });

    let cap = K::NEUTRAL_AXIS_CHROMA_CAP;
    let (mid, (shadow, highlight)) = join(
        || band_refs(&mb_idx, &chroma2, cap),
        || {
            join(
                || band_refs(&sb_idx, &chroma2, cap),
                || band_refs(&hb_idx, &chroma2, cap),
            )
        },
    );
    let (mid, shadow) = (mid?, shadow?);

    // Confidence: corrected tightness of the grey sets × midtone sample size × mid↔shadow
    // deviation agreement (a dead zone passes plausible crossover).
    let tight = (1.0 - mid.median_chroma.max(shadow.median_chroma) / cap).clamp(0.0, 1.0);
    // Boundary cast: a sample count.
    let size_term = mid.count as f64 / (mid.count as f64 + K::NEUTRAL_AXIS_CONFIDENCE_N0);
    let (dm, ds) = (norm_ref(mid.refs), norm_ref(shadow.refs));
    let spread = ((dm[0] - dm[1]) - (ds[0] - ds[1]))
        .abs()
        .max(((dm[2] - dm[1]) - (ds[2] - ds[1])).abs());
    let agree = 1.0
        - ((spread - K::NEUTRAL_AXIS_AGREEMENT_DEADZONE).max(0.0)
            / K::NEUTRAL_AXIS_AGREEMENT_SCALE)
            .min(1.0);
    let confidence = (tight * size_term * agree).clamp(0.0, 1.0);
    Some(NeutralAxisRefs {
        mid: mid.refs,
        shadow: shadow.refs,
        highlight: highlight.map(|h| h.refs),
        confidence,
    })
}

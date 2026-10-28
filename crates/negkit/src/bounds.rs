//! Log-exposure bounds analysis — port of `BoundsAnalysis.swift` (NegPy
//! `analyze_log_exposure_bounds_from_log`, C-41 branch only — no E6 reversal, no margin-mode
//! negative sliders in v1, though the margin path is kept since it's part of
//! `_sample_log_bounds`; 127bcd7 / NegPy 0.43 same-pixel colour floors).
//!
//! Two independent percentile axes on the prefiltered log grid, one sort per channel:
//! the **luma axis** (base clip 0.01 %) sets the mean centre + span (dynamic range); the
//! **colour axis** (base clip 1.0 %) sets per-channel cast offsets relative to the median
//! channel. The colour axis's dense end (floors = print whites) prefers the same-pixel
//! chroma-gated band refs ([`same_pixel_color_floor_refs`], float64 end to end like upstream),
//! falling back to the percentile pass when the band has no trustworthy neutrals.
//!
//! Owner: wave 2 / agent B. `LogNegativeBounds` is the shared contract (also read from the
//! fixtures' `analysis.json`).

use serde::{Deserialize, Serialize};

use crate::constants::K;
use crate::meters::rms_chroma;
use crate::stats;
use crate::{RgbImage, V3};

/// Per-channel log-density floors (dense end / print whites) and ceils (thin end), floors < ceils.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogNegativeBounds {
    pub floors: V3,
    pub ceils: V3,
}

impl LogNegativeBounds {
    /// Creative white/black-point trims fold in here at derive time only (floors + wp, ceils + bp).
    pub fn applying_offsets(self, white_point: f64, black_point: f64) -> Self {
        Self {
            floors: [
                self.floors[0] + white_point,
                self.floors[1] + white_point,
                self.floors[2] + white_point,
            ],
            ceils: [
                self.ceils[0] + black_point,
                self.ceils[1] + black_point,
                self.ceils[2] + black_point,
            ],
        }
    }

    /// Rec.709-weighted luminance density range (`luminanceDensityRange`): the grade's
    /// `lumRange` and the Contrast Mask's scale read this from the PRE-offset base bounds.
    pub fn luminance_density_range(&self) -> f64 {
        crate::constants::K::LUMA_R * (self.ceils[0] - self.floors[0]).abs()
            + crate::constants::K::LUMA_G * (self.ceils[1] - self.floors[1]).abs()
            + crate::constants::K::LUMA_B * (self.ceils[2] - self.floors[2]).abs()
    }

    /// Per-channel range `ceil − floor`.
    pub fn ranges(&self) -> V3 {
        [
            self.ceils[0] - self.floors[0],
            self.ceils[1] - self.floors[1],
            self.ceils[2] - self.floors[2],
        ]
    }
}

/// The grid's three channels, each sorted ascending — the output of [`sorted_channels`],
/// shared by every percentile consumer (`prepare` sorts once).
pub type SortedChannels = [Vec<f32>; 3];

/// Per-channel (floors, ceils) at one clip level — the output of [`sample_log_bounds`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampledBounds {
    pub floors: V3,
    pub ceils: V3,
}

/// `_sample_log_bounds`: per-channel (floors, ceils) at one clip level. A non-negative
/// `percentile_clip` is added to `base` and clamped to `[1e-5, 50]`; a negative one switches
/// to margin mode — percentiles at the base clip, the (positive) margin added outward.
/// `None` when a channel is empty.
pub fn sample_log_bounds(
    channels: &SortedChannels,
    percentile_clip: f64,
    base: f64,
) -> Option<SampledBounds> {
    let (clip, margin) = if percentile_clip >= 0.0 {
        ((percentile_clip + base).clamp(0.00001, 50.0), 0.0)
    } else {
        (base, -percentile_clip)
    };
    let (p_low, p_high) = (clip, 100.0 - clip);

    let mut floors = [0.0; 3];
    let mut ceils = [0.0; 3];
    for (ch, sorted) in channels.iter().enumerate() {
        let (Some(f), Some(c)) = (floors.get_mut(ch), ceils.get_mut(ch)) else {
            continue;
        };
        *f = stats::percentile_of_sorted(sorted, p_low)?;
        *c = stats::percentile_of_sorted(sorted, p_high)?;
    }
    if margin > 0.0 {
        for (f, c) in floors.iter_mut().zip(ceils.iter_mut()) {
            if *c >= *f {
                *f -= margin;
                *c += margin;
            } else {
                *f += margin;
                *c -= margin;
            }
        }
    }
    Some(SampledBounds { floors, ceils })
}

/// One ascending sort per channel — the expensive half of the percentile sampling. Public so
/// `prepare` can share the sorts with the other percentile meters (`shadow_refs`): the sorted
/// arrays are a pure function of the grid, so every consumer sees identical bytes.
///
/// The three channels are wholly independent — each deinterleaves and sorts its own buffer —
/// so running them concurrently (`rayon` feature) cannot reorder anything within a channel
/// (6.2 → 2.5 ms in Swift). Each lane is collected straight from the strided iterator: no
/// zero-fill that the sort then overwrites.
pub fn sorted_channels(grid: &RgbImage) -> SortedChannels {
    let sort = |c: usize| -> Vec<f32> {
        let lane: Vec<f32> = grid.data.iter().skip(c).step_by(3).copied().collect();
        stats::sorted_ascending(&lane)
    };
    #[cfg(feature = "rayon")]
    {
        let (r, (g, b)) = rayon::join(|| sort(0), || rayon::join(|| sort(1), || sort(2)));
        [r, g, b]
    }
    #[cfg(not(feature = "rayon"))]
    {
        [sort(0), sort(1), sort(2)]
    }
}

/// `_same_pixel_color_floor_refs`: dense-end (print-white) colour refs from one shared pixel
/// set — the luma-extreme band's lowest-chroma subset, chroma measured base-anchored (offsets
/// from the thin-end refs, per-channel span as provisional gamma, refined once from the band
/// medians). Independent per-channel percentiles read a different scene object per channel,
/// so coloured highlight content masquerades as film cast; a shared chroma-gated set cannot.
/// The thin end needs no such treatment — density on real film is bounded below by base,
/// anchoring per-channel ceils. `None` when the band's neutral set is too small or too
/// chromatic (the caller falls back to the percentile pass).
///
/// Runs in float64 end to end (the one analysis path that does upstream — it casts the grid
/// to float64 on entry), so this is bit-comparable to the numpy reference.
pub fn same_pixel_color_floor_refs(
    grid: &RgbImage,
    luma_floors: V3,
    luma_ceils: V3,
    base_refs: V3,
    color_clip: f64,
) -> Option<V3> {
    let eps = 1e-6;
    let n = grid.pixel_count();
    let min_px = K::NEUTRAL_AXIS_MIN_PIXELS;
    let width = K::COLOR_BOUNDS_BAND_WIDTH;

    let mut denoms = [0.0; 3];
    for ch in 0..3 {
        let mut denom = luma_ceils[ch] - luma_floors[ch];
        if denom.abs() < eps {
            denom = if denom >= 0.0 { eps } else { -eps };
        }
        denoms[ch] = denom;
    }
    let luma: Vec<f64> = crate::meters::map_pixels(&grid.data, |p| {
        let r = (f64::from(p[0]) - luma_floors[0]) / denoms[0];
        let g = (f64::from(p[1]) - luma_floors[1]) / denoms[1];
        let b = (f64::from(p[2]) - luma_floors[2]) / denoms[2];
        K::LUMA_R * r + K::LUMA_G * g + K::LUMA_B * b
    });

    let clip = (50.0 - width).min(color_clip).max(0.00001);
    let sorted_luma = stats::sorted_ascending(&luma);
    let lo = stats::percentile_of_sorted(&sorted_luma, clip)?;
    let hi = stats::percentile_of_sorted(&sorted_luma, clip + width)?;

    // Band offsets from the thin-end base refs, order-preserving triplets.
    let mut d: Vec<f64> = Vec::with_capacity(n / 8 * 3);
    for (p, &l) in grid.data.chunks_exact(3).zip(&luma) {
        if l >= lo && l <= hi {
            d.push(f64::from(p[0]) - base_refs[0]);
            d.push(f64::from(p[1]) - base_refs[1]);
            d.push(f64::from(p[2]) - base_refs[2]);
        }
    }
    let m = d.len() / 3;
    if m < min_px {
        return None;
    }

    // Lowest-RMS-chroma subset of the band under a provisional gamma: (kept row indices,
    // median chroma of the kept set).
    let select = |gamma: V3| -> Option<(Vec<usize>, f64)> {
        let g = gamma.map(|v| if v.abs() < eps { eps } else { v });
        let chroma: Vec<f64> = d
            .chunks_exact(3)
            .map(|t| rms_chroma(t[0] / g[0], t[1] / g[1], t[2] / g[2]))
            .collect();
        let thr = stats::quantile(&chroma, K::NEUTRAL_AXIS_CHROMA_QUANTILE)?;
        let mut kept: Vec<usize> = Vec::with_capacity(m / 2);
        let mut kept_chroma: Vec<f64> = Vec::with_capacity(m / 2);
        for (j, &c) in chroma.iter().enumerate() {
            if c <= thr {
                kept.push(j);
                kept_chroma.push(c);
            }
        }
        if kept.len() < min_px {
            return None;
        }
        Some((kept, stats::median(&kept_chroma)?))
    };

    let channel_medians = |rows: &[usize]| -> Option<V3> {
        let mut scratch: Vec<f64> = Vec::with_capacity(rows.len());
        let mut out = [0.0; 3];
        for (ch, slot) in out.iter_mut().enumerate() {
            scratch.clear();
            for &j in rows {
                scratch.push(d.get(j * 3 + ch).copied()?);
            }
            *slot = stats::median(&scratch)?;
        }
        Some(out)
    };

    let spans = [
        luma_floors[0] - base_refs[0],
        luma_floors[1] - base_refs[1],
        luma_floors[2] - base_refs[2],
    ];
    // Pass-1 loose cap: a homogeneous coloured cluster would otherwise be self-normalized to
    // zero chroma by pass 2 and read as neutral.
    let (first_kept, first_chroma) = select(spans)?;
    if first_chroma > K::NEUTRAL_AXIS_FIRST_PASS_CAP {
        return None;
    }
    let provisional = channel_medians(&first_kept)?;
    if !provisional.iter().all(|v| v.abs() >= eps) {
        return None;
    }
    let (second_kept, second_chroma) = select(provisional)?;
    if second_chroma > K::NEUTRAL_AXIS_CHROMA_CAP {
        return None;
    }
    let med = channel_medians(&second_kept)?;
    Some([
        base_refs[0] + med[0],
        base_refs[1] + med[1],
        base_refs[2] + med[2],
    ])
}

/// The middle of three values (Swift's `sorted()[1]` on a triple).
#[inline]
fn median3(v: V3) -> f64 {
    v[0].min(v[1]).max(v[0].max(v[1]).min(v[2]))
}

/// Two-axis recombination at the default clips (luma 0, colour [`K::BASE_COLOR_CLIP`]): sorts
/// the channels itself. See [`analyze_sorted`]. `None` on an empty grid.
pub fn analyze(grid: &RgbImage) -> Option<LogNegativeBounds> {
    analyze_sorted(grid, &sorted_channels(grid), 0.0, K::BASE_COLOR_CLIP)
}

/// Two-axis recombination with the caller's pre-sorted channels (shared with the other
/// percentile meters — one sort pass per prepare, not two): the luma clip drives the mean
/// centre + span, the colour clip the per-channel cast (offset from the median channel).
/// Colour floors (dense end, scene content) prefer the same-pixel chroma-gated band refs
/// (NegPy 0.43), falling back to the percentile pass when the band holds no trustworthy
/// neutrals (and always for margin-mode clips). `None` on an empty grid.
pub fn analyze_sorted(
    grid: &RgbImage,
    channels: &SortedChannels,
    luma_range_clip: f64,
    color_range_clip: f64,
) -> Option<LogNegativeBounds> {
    let luma = sample_log_bounds(channels, luma_range_clip, K::BASE_LUMA_CLIP)?;
    let colour = sample_log_bounds(channels, color_range_clip, 0.0)?;
    let mut c_floors = colour.floors;
    let c_ceils = colour.ceils;
    if color_range_clip >= 0.0
        && let Some(sp) =
            same_pixel_color_floor_refs(grid, luma.floors, luma.ceils, c_ceils, color_range_clip)
    {
        c_floors = sp;
    }

    let mean_lf = (luma.floors[0] + luma.floors[1] + luma.floors[2]) / 3.0;
    let mean_lc = (luma.ceils[0] + luma.ceils[1] + luma.ceils[2]) / 3.0;
    let med_cf = median3(c_floors);
    let med_cc = median3(c_ceils);

    Some(LogNegativeBounds {
        floors: [
            mean_lf + (c_floors[0] - med_cf),
            mean_lf + (c_floors[1] - med_cf),
            mean_lf + (c_floors[2] - med_cf),
        ],
        ceils: [
            mean_lc + (c_ceils[0] - med_cc),
            mean_lc + (c_ceils[1] - med_cc),
            mean_lc + (c_ceils[2] - med_cc),
        ],
    })
}

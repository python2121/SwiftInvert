//! `prepare` / `finalize` / `analyze` — port of `ExposureKernel`'s analysis half in
//! `ExposureSettings.swift` (NegPy 2125a34 offset-independent split).
//!
//! Full per-image analysis: prefilter + bounds + all meters. Both halves are offset-independent
//! since the 2125a34 port: [`prepare`] (≈17 ms once per image/crop in Swift) holds the
//! prefiltered grid plus every meter that reads the grid or the base bounds; [`finalize`]
//! (≈7 ms, cached with it) measures the neutral axis against the PRE-trim base bounds —
//! white/black-point drags re-run NO analysis at all; the offsets fold into `finalBounds` at
//! derive time only. Cache a `Prepared` per (image, crop_rect, analysis_rect).
//!
//! Region priority mirrors NegPy's `resolve_analysis_region` (see
//! [`crate::prefilter::resolve_analysis_region`]): a freehand `analysis_rect` wins and disables
//! the centered buffer inset; otherwise the output `crop_rect` scopes the meters (so borders
//! outside the crop can't throw off the inversion) with the buffer applied inside it. Default
//! centered inset: [`DEFAULT_ANALYSIS_BUFFER`] = 10 % per side = the middle 80 % of the frame
//! (SwiftInvert default; NegPy ships 5 %, which the parity tests pass explicitly).
//!
//! Owner: wave 2 / agent B. The two result structs are the shared contract: `Prepared` is the
//! offset-independent cache (per rect state), `ExposureAnalysis` what `derive` consumes and what
//! the fixtures' `analysis.json` serializes (keep the Swift Codable keys).

use serde::{Deserialize, Serialize};

use crate::constants::K;
use crate::rect::NormalizedRect;
use crate::{LogNegativeBounds, NegError, RgbImage, V3, bounds, meters, prefilter};

pub use crate::prefilter::DEFAULT_ANALYSIS_BUFFER;

/// Everything `prepare` computes once per image/crop (≈17 ms on the 1536 px proxy in Swift).
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    /// The prefiltered log grid (block medians of −D), the input of every meter.
    pub grid: RgbImage,
    pub base_bounds: LogNegativeBounds,
    pub anchor: f64,
    pub textural_range: f64,
    pub shadow_point: f64,
    pub highlight_point: f64,
    pub shadow_refs: V3,
}

/// The analysis `derive_render_params` consumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureAnalysis {
    pub base_bounds: LogNegativeBounds,
    pub anchor: f64,
    pub textural_range: f64,
    #[serde(default)]
    pub shadow_point: Option<f64>,
    #[serde(default)]
    pub highlight_point: Option<f64>,
    pub shadow_refs: V3,
    #[serde(default)]
    pub neutral_mid: Option<V3>,
    #[serde(default)]
    pub neutral_shadow: Option<V3>,
    #[serde(default)]
    pub neutral_highlight: Option<V3>,
    #[serde(default)]
    pub neutral_confidence: Option<f64>,
}

/// The expensive, offset-independent half of the analysis: the prefiltered grid plus every
/// meter that doesn't read the final (offset) bounds. Cache this per
/// (image, `crop_rect`, `analysis_rect`); white/black-point drags only need [`finalize`].
///
/// Pass [`DEFAULT_ANALYSIS_BUFFER`] for the Swift default. `Err(NegError::Empty)` when the
/// metered region prefilters to nothing (an empty input image).
pub fn prepare(
    linear_image: &RgbImage,
    crop_rect: Option<&NormalizedRect>,
    analysis_rect: Option<&NormalizedRect>,
    analysis_buffer: f64,
) -> Result<Prepared, NegError> {
    let (metered, buffer) =
        prefilter::resolve_analysis_region(linear_image, crop_rect, analysis_rect, analysis_buffer);
    let grid = prefilter::prefilter_log_grid(&metered, buffer);
    let n = grid.pixel_count();
    if n == 0 {
        return Err(NegError::Empty);
    }
    if grid.data.len() != n * 3 {
        return Err(NegError::Shape {
            width: grid.width,
            height: grid.height,
            len: grid.data.len(),
        });
    }
    // One sort pass per channel, shared by the bounds percentiles and the shadow refs
    // (identical inputs → identical bytes; the sort was the biggest single line item in
    // prepare).
    let channels = bounds::sorted_channels(&grid);
    let base =
        bounds::analyze_sorted(&grid, &channels, 0.0, K::BASE_COLOR_CLIP).ok_or(NegError::Empty)?;
    // Anchor + reach/hold points read against the per-frame base bounds (luma_source_bounds),
    // sharing one textured gate and one sort; the textural range gates its own raw-log luma.
    // The two are independent, so they run side by side under `rayon`.
    let (textured, textural_range) = {
        #[cfg(feature = "rayon")]
        {
            rayon::join(
                || meters::textured_luma_meters(&grid, &base),
                || meters::textural_range(&grid),
            )
        }
        #[cfg(not(feature = "rayon"))]
        {
            (
                meters::textured_luma_meters(&grid, &base),
                meters::textural_range(&grid),
            )
        }
    };
    let textured = textured.ok_or(NegError::Empty)?;
    let textural_range = textural_range.ok_or(NegError::Empty)?;
    let shadow_refs = meters::shadow_refs_sorted(&channels).ok_or(NegError::Empty)?;
    Ok(Prepared {
        grid,
        base_bounds: base,
        anchor: textured.anchor,
        textural_range,
        shadow_point: textured.shadow_point,
        highlight_point: textured.highlight_point,
        shadow_refs,
    })
}

/// Assemble the analysis from a [`Prepared`]. The neutral axis is measured against the
/// PRE-trim base bounds (NegPy 2125a34): the film's inherent cast is a source property, and
/// creative white/black-point trims must not perturb its measurement — so the whole analysis
/// is offset-independent (offsets fold into `finalBounds` at derive time only, and wp/bp
/// drags re-run no analysis at all). Pure: never mutates its input.
pub fn finalize(prepared: &Prepared) -> ExposureAnalysis {
    let neutral = meters::neutral_axis(&prepared.grid, &prepared.base_bounds);
    ExposureAnalysis {
        base_bounds: prepared.base_bounds,
        anchor: prepared.anchor,
        textural_range: prepared.textural_range,
        shadow_point: Some(prepared.shadow_point),
        highlight_point: Some(prepared.highlight_point),
        shadow_refs: prepared.shadow_refs,
        neutral_mid: neutral.map(|n| n.mid),
        neutral_shadow: neutral.map(|n| n.shadow),
        neutral_highlight: neutral.and_then(|n| n.highlight),
        neutral_confidence: neutral.map(|n| n.confidence),
    }
}

/// [`prepare`] then [`finalize`] — the full per-image analysis.
pub fn analyze(
    linear_image: &RgbImage,
    crop_rect: Option<&NormalizedRect>,
    analysis_rect: Option<&NormalizedRect>,
    analysis_buffer: f64,
) -> Result<ExposureAnalysis, NegError> {
    Ok(finalize(&prepare(
        linear_image,
        crop_rect,
        analysis_rect,
        analysis_buffer,
    )?))
}

//! `prepare` / `finalize` / `analyze` — port of `ExposureKernel`'s analysis half in
//! `ExposureSettings.swift` (NegPy 2125a34 offset-independent split).
//!
//! Owner: wave 2 / agent B. The two result structs are the shared contract: `Prepared` is the
//! offset-independent cache (per rect state), `ExposureAnalysis` what `derive` consumes and what
//! the fixtures' `analysis.json` serializes (keep the Swift Codable keys).

use serde::{Deserialize, Serialize};

use crate::{LogNegativeBounds, RgbImage, V3};

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

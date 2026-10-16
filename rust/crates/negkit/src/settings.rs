//! `ExposureSettings` — the sidecar schema. Port of the struct in `ExposureSettings.swift`.
//!
//! Owner: wave 1 / agent C — who must (1) verify every default and JSON key against the Swift
//! struct and its custom `init(from:)` (retired keys such as the old dye-separation field are
//! ignored; keyless sidecars decode to defaults), (2) port `SidecarCodecTests` + the
//! `HistoryLabelTests` field sweep as `tests/sidecar.rs` pinning the stored-property count (54).
//!
//! JSON keys are the Swift Codable keys (camelCase). `[f64; 3]` / `[f64; 2]` encode as JSON arrays
//! like Swift's `SIMD3`/`SIMD2`; `NormalizedRect` as `{x, y, width, height}`.

use serde::{Deserialize, Serialize};

use crate::{NormalizedRect, V2, V3};

/// Every user-facing control plus geometry. Missing keys decode to the defaults below
/// (sidecar back-compat); unknown keys are ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExposureSettings {
    /// Brightness slider stores `density` (UI shows 2 − density: right = brighter).
    pub density: f64,
    /// Paper grade as ISO R (50–180; 115 ≈ grade 2).
    pub grade: f64,
    pub wb_cyan: f64,
    pub wb_magenta: f64,
    pub wb_yellow: f64,
    pub auto_exposure: bool,
    pub auto_normalize_contrast: bool,
    pub cast_removal_strength: f64,
    pub white_point_offset: f64,
    pub black_point_offset: f64,
    pub toe: f64,
    pub toe_width: f64,
    pub shoulder: f64,
    pub shoulder_width: f64,
    pub paper_dmin: bool,
    pub true_black: bool,
    pub exposure_stops: f64,
    pub shadows: f64,
    pub shadow_contrast: f64,
    pub dark_shadows: f64,
    pub highlights: f64,
    pub highlight_contrast: f64,
    pub overall_contrast: f64,
    pub vibrance: f64,
    pub saturation: f64,
    pub skin_protection: f64,
    pub red_hue: f64,
    pub red_saturation: f64,
    pub yellow_hue: f64,
    pub yellow_saturation: f64,
    pub green_hue: f64,
    pub green_saturation: f64,
    pub blue_hue: f64,
    pub blue_saturation: f64,
    pub print_saturation: f64,
    pub separation_damping: f64,
    pub contrast_mask: f64,
    pub mask_spacer: f64,
    pub hue_trim: f64,
    pub pre_saturation: f64,
    pub temp: f64,
    pub tint: f64,
    pub color_shadows: V3,
    pub color_mids: V3,
    pub color_highs: V3,
    pub levels_red: Vec<V2>,
    pub levels_green: Vec<V2>,
    pub levels_blue: Vec<V2>,
    /// Clockwise degrees: 0 / 90 / 180 / 270.
    pub rotation: i32,
    pub flip_horizontal: bool,
    pub fine_rotation: f64,
    pub analysis_rect: Option<NormalizedRect>,
    pub analysis_rect_fine_rotation: f64,
    pub crop_rect: Option<NormalizedRect>,
}

impl Default for ExposureSettings {
    fn default() -> Self {
        Self {
            density: 1.0,
            grade: 115.0,
            wb_cyan: 0.0,
            wb_magenta: 0.0,
            wb_yellow: 0.0,
            auto_exposure: true,
            auto_normalize_contrast: true,
            cast_removal_strength: 0.5,
            white_point_offset: 0.0,
            black_point_offset: 0.0,
            toe: 0.0,
            toe_width: 2.5,
            shoulder: 0.0,
            shoulder_width: 2.5,
            paper_dmin: false,
            true_black: true,
            exposure_stops: 0.0,
            shadows: 0.0,
            shadow_contrast: 0.0,
            dark_shadows: 0.0,
            highlights: 0.0,
            highlight_contrast: 0.0,
            overall_contrast: 0.0,
            vibrance: 1.0,
            saturation: 1.0,
            skin_protection: 0.5,
            red_hue: 0.0,
            red_saturation: 1.0,
            yellow_hue: 0.0,
            yellow_saturation: 1.0,
            green_hue: 0.0,
            green_saturation: 1.0,
            blue_hue: 0.0,
            blue_saturation: 1.0,
            print_saturation: 1.0,
            separation_damping: 0.0,
            contrast_mask: 0.0,
            mask_spacer: 4.0,
            hue_trim: 0.0,
            pre_saturation: 1.15,
            temp: 0.0,
            tint: 0.0,
            color_shadows: [0.0; 3],
            color_mids: [0.0; 3],
            color_highs: [0.0; 3],
            levels_red: Vec::new(),
            levels_green: Vec::new(),
            levels_blue: Vec::new(),
            rotation: 0,
            flip_horizontal: false,
            fine_rotation: 0.0,
            analysis_rect: None,
            analysis_rect_fine_rotation: 0.0,
            crop_rect: None,
        }
    }
}

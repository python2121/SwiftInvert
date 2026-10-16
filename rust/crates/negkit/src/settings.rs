//! `ExposureSettings` — the sidecar schema. Port of the struct in `ExposureSettings.swift`
//! (field defaults mirror NegPy's ExposureConfig / ProcessConfig, C-41 subset) including its
//! custom `init(from:)`.
//!
//! JSON keys are the Swift Codable keys (camelCase). `[f64; 3]` / `[f64; 2]` encode as JSON
//! arrays like Swift's `SIMD3`/`SIMD2`; `NormalizedRect` as `{x, y, width, height}`; the two
//! optional rects are OMITTED when absent (Swift's synthesized `encodeIfPresent`), so a
//! default encodes 52 keys and a fully-populated value all 54.
//!
//! Decoding mirrors the Swift decoder's leniency field by field: a missing key, a `null`, or
//! a value of the wrong type decodes to that FIELD's default (Swift wraps every `decode` in
//! `try?`), unknown keys are ignored (forward compat), the levels arrays accept the brief
//! single-point era's scalar `[x, y]` pair (one anchor; identity pairs drop), and retired keys
//! (e.g. the old `dyeSeparation`) are simply unknown. A sidecar can therefore never fail to
//! decode as a whole — only a non-object payload is an error.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{NormalizedRect, V2, V3};

/// Every user-facing control plus geometry. Missing keys decode to the defaults in
/// [`Default`] (sidecar back-compat); unknown keys are ignored.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureSettings {
    /// Print exposure around the calibrated default (NegPy: 1.0 = neutral). The Brightness
    /// slider shows `2 − density` (right = brighter).
    pub density: f64,
    /// Contrast as ISO R paper exposure range (50 hard … 180 soft; 115 ≈ grade 2).
    pub grade: f64,
    /// Global white balance (CMY filtration, ±1 slider = ±20cc).
    pub wb_cyan: f64,
    /// Magenta filtration.
    pub wb_magenta: f64,
    /// Yellow filtration.
    pub wb_yellow: f64,
    /// Auto density: use the per-frame metered anchor.
    pub auto_exposure: bool,
    /// Auto grade: adapt slope to the measured textural range.
    pub auto_normalize_contrast: bool,
    /// C-41 gray balance (orange-mask cancel). Always confidence-scaled (NegPy 0.36 removed
    /// the auto toggle).
    pub cast_removal_strength: f64,
    /// Histogram edge handle: log-density offset on the analyzed floors.
    pub white_point_offset: f64,
    /// Histogram edge handle: log-density offset on the analyzed ceils.
    pub black_point_offset: f64,
    /// Toe slider.
    pub toe: f64,
    /// Toe knee width.
    pub toe_width: f64,
    /// Shoulder slider.
    pub shoulder: f64,
    /// Shoulder knee width.
    pub shoulder_width: f64,
    /// Paper white floor (NegPy paper_dmin, d_min = 0.06). Default OFF since NegPy 0.38.
    pub paper_dmin: bool,
    /// True Black: black point compensation — paper Dmax maps to display black. Default ON
    /// (NegPy 0.38).
    pub true_black: bool,
    /// Global print exposure in stops (+ = brighter, histogram right).
    pub exposure_stops: f64,
    /// Shadows lift (+1 lifts).
    pub shadows: f64,
    /// Shadow-region contrast (−3…+3; the negative side is remapped at derive).
    pub shadow_contrast: f64,
    /// Same lift as `shadows`, anchored deeper (`K::DARK_SHADOW_TONE_ANCHOR`).
    pub dark_shadows: f64,
    /// Highlights (−1 brings highlights down).
    pub highlights: f64,
    /// Highlight-region contrast.
    pub highlight_contrast: f64,
    /// Overall contrast: rotates the whole curve around the reference tone.
    pub overall_contrast: f64,
    /// Lab vibrance (1.0 = neutral; SwiftInvert-maintained).
    pub vibrance: f64,
    /// Lab saturation (1.0 = neutral).
    pub saturation: f64,
    /// Skin Protection (NegPy bfcd90a/fb94aed). DEFAULT ON at 0.5 — keyless sidecars adopt it.
    pub skin_protection: f64,
    /// Colour-mixer red band hue nudge. (Defaulted +0.5 through the ROMM era; neutral again
    /// since the b3490eb Adobe RGB port, 2026-07-20.)
    pub red_hue: f64,
    /// Colour-mixer red band saturation.
    pub red_saturation: f64,
    /// Colour-mixer yellow band hue nudge.
    pub yellow_hue: f64,
    /// Colour-mixer yellow band saturation.
    pub yellow_saturation: f64,
    /// Colour-mixer green band hue nudge.
    pub green_hue: f64,
    /// Colour-mixer green band saturation.
    pub green_saturation: f64,
    /// Colour-mixer blue band hue nudge.
    pub blue_hue: f64,
    /// Colour-mixer blue band saturation.
    pub blue_saturation: f64,
    /// Print Saturation (NegPy 0.45, their surviving density-space control, renamed
    /// dye_separation in 3fb5ca8). 1.0 = off.
    pub print_saturation: f64,
    /// Separation Damping (NegPy d86a5aa). Inert at printSaturation 1.0. 0 = off.
    pub separation_damping: f64,
    /// Contrast Mask (NegPy 515c1f5): unsharp-mask gamma, ±0.5. 0 = off.
    pub contrast_mask: f64,
    /// The mask's spacer: σ of the plane blur as % of the grid's short side, 2–6.
    pub mask_spacer: f64,
    /// Hue Trim (NegPy 7a07f5c): degrees of a*b* rotation about the neutral axis. 0 = off.
    /// An ordinary sidecar setting here (upstream makes theirs sticky across frames).
    pub hue_trim: f64,
    /// Pre-saturation (Negative Lab Pro concept): density-deviation gain before the print
    /// curve. Default 1.15 — NegPy has no equivalent; parity fixtures pin 1.0.
    pub pre_saturation: f64,
    /// Temp: blue↔yellow along the Planckian direction, + = warmer.
    pub temp: f64,
    /// Tint: green↔magenta, + = magenta.
    pub tint: f64,
    /// Per-band colour balance, shadows: x = R↔C, y = G↔M, z = B↔Y (+ = toward C/M/Y).
    pub color_shadows: V3,
    /// Per-band colour balance, mids.
    pub color_mids: V3,
    /// Per-band colour balance, highlights.
    pub color_highs: V3,
    /// Interactive-histogram levels anchors `(input, output)`, red. Empty = identity.
    pub levels_red: Vec<V2>,
    /// Levels anchors, green.
    pub levels_green: Vec<V2>,
    /// Levels anchors, blue.
    pub levels_blue: Vec<V2>,
    /// Clockwise degrees: 0 / 90 / 180 / 270 (baked into the pixels after decode).
    pub rotation: i32,
    /// Horizontal flip (baked with the rotation).
    pub flip_horizontal: bool,
    /// Straighten: degrees, clockwise-positive, auto-cropped to the inscribed rect.
    pub fine_rotation: f64,
    /// Scopes ONLY the metering (buffer disabled inside it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_rect: Option<NormalizedRect>,
    /// The fineRotation at the moment `analysis_rect` was drawn.
    pub analysis_rect_fine_rotation: f64,
    /// Crops the output AND scopes the metering (buffer applied inside).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop_rect: Option<NormalizedRect>,
}

impl ExposureSettings {
    /// The 54 stored properties' JSON keys, in declaration order. The sidecar drift-catcher
    /// pins this list against the serialized keys; a new field needs a line here, in
    /// `Default`, in `Deserialize`, and in the tests' mutation list.
    pub const STORED_KEYS: [&'static str; 54] = [
        "density",
        "grade",
        "wbCyan",
        "wbMagenta",
        "wbYellow",
        "autoExposure",
        "autoNormalizeContrast",
        "castRemovalStrength",
        "whitePointOffset",
        "blackPointOffset",
        "toe",
        "toeWidth",
        "shoulder",
        "shoulderWidth",
        "paperDmin",
        "trueBlack",
        "exposureStops",
        "shadows",
        "shadowContrast",
        "darkShadows",
        "highlights",
        "highlightContrast",
        "overallContrast",
        "vibrance",
        "saturation",
        "skinProtection",
        "redHue",
        "redSaturation",
        "yellowHue",
        "yellowSaturation",
        "greenHue",
        "greenSaturation",
        "blueHue",
        "blueSaturation",
        "printSaturation",
        "separationDamping",
        "contrastMask",
        "maskSpacer",
        "hueTrim",
        "preSaturation",
        "temp",
        "tint",
        "colorShadows",
        "colorMids",
        "colorHighs",
        "levelsRed",
        "levelsGreen",
        "levelsBlue",
        "rotation",
        "flipHorizontal",
        "fineRotation",
        "analysisRect",
        "analysisRectFineRotation",
        "cropRect",
    ];

    /// Number of stored properties (the Swift `Mirror` count the drift-catchers pin).
    pub const STORED_PROPERTY_COUNT: usize = Self::STORED_KEYS.len();
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

/// The lenient per-field reader behind `Deserialize` (one `try? decode ?? default` each).
struct Fields<'a>(&'a BTreeMap<String, Value>);

impl Fields<'_> {
    fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.0.get(key).and_then(|v| T::deserialize(v).ok())
    }

    fn f(&self, key: &str, default: f64) -> f64 {
        self.get(key).unwrap_or(default)
    }

    fn b(&self, key: &str, default: bool) -> bool {
        self.get(key).unwrap_or(default)
    }

    /// Swift's `Int` decode: a JSON integer, or a float that is exactly integral (`90.0`),
    /// within `i32`; anything else is the default.
    fn int(&self, key: &str, default: i32) -> i32 {
        match self.0.get(key) {
            Some(Value::Number(n)) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                .and_then(|i| i32::try_from(i).ok())
                .unwrap_or(default),
            _ => default,
        }
    }

    fn v3(&self, key: &str) -> V3 {
        self.get(key).unwrap_or([0.0; 3])
    }

    /// Anchor arrays; sidecars from the brief single-point era decode their one point
    /// (identity pairs drop), older sidecars to identity.
    fn levels(&self, key: &str) -> Vec<V2> {
        if let Some(arr) = self.get::<Vec<V2>>(key) {
            return arr;
        }
        match self.get::<V2>(key) {
            Some(single) if single[0] != single[1] => vec![single],
            _ => Vec::new(),
        }
    }
}

impl<'de> Deserialize<'de> for ExposureSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, Value>::deserialize(deserializer)?;
        let c = Fields(&map);
        let d = Self::default();
        Ok(Self {
            density: c.f("density", d.density),
            grade: c.f("grade", d.grade),
            wb_cyan: c.f("wbCyan", d.wb_cyan),
            wb_magenta: c.f("wbMagenta", d.wb_magenta),
            wb_yellow: c.f("wbYellow", d.wb_yellow),
            auto_exposure: c.b("autoExposure", d.auto_exposure),
            auto_normalize_contrast: c.b("autoNormalizeContrast", d.auto_normalize_contrast),
            cast_removal_strength: c.f("castRemovalStrength", d.cast_removal_strength),
            white_point_offset: c.f("whitePointOffset", d.white_point_offset),
            black_point_offset: c.f("blackPointOffset", d.black_point_offset),
            toe: c.f("toe", d.toe),
            toe_width: c.f("toeWidth", d.toe_width),
            shoulder: c.f("shoulder", d.shoulder),
            shoulder_width: c.f("shoulderWidth", d.shoulder_width),
            paper_dmin: c.b("paperDmin", d.paper_dmin),
            true_black: c.b("trueBlack", d.true_black),
            exposure_stops: c.f("exposureStops", d.exposure_stops),
            shadows: c.f("shadows", d.shadows),
            shadow_contrast: c.f("shadowContrast", d.shadow_contrast),
            dark_shadows: c.f("darkShadows", d.dark_shadows),
            highlights: c.f("highlights", d.highlights),
            highlight_contrast: c.f("highlightContrast", d.highlight_contrast),
            overall_contrast: c.f("overallContrast", d.overall_contrast),
            vibrance: c.f("vibrance", d.vibrance),
            saturation: c.f("saturation", d.saturation),
            skin_protection: c.f("skinProtection", d.skin_protection),
            red_hue: c.f("redHue", d.red_hue),
            red_saturation: c.f("redSaturation", d.red_saturation),
            yellow_hue: c.f("yellowHue", d.yellow_hue),
            yellow_saturation: c.f("yellowSaturation", d.yellow_saturation),
            green_hue: c.f("greenHue", d.green_hue),
            green_saturation: c.f("greenSaturation", d.green_saturation),
            blue_hue: c.f("blueHue", d.blue_hue),
            blue_saturation: c.f("blueSaturation", d.blue_saturation),
            print_saturation: c.f("printSaturation", d.print_saturation),
            separation_damping: c.f("separationDamping", d.separation_damping),
            contrast_mask: c.f("contrastMask", d.contrast_mask),
            mask_spacer: c.f("maskSpacer", d.mask_spacer),
            hue_trim: c.f("hueTrim", d.hue_trim),
            pre_saturation: c.f("preSaturation", d.pre_saturation),
            temp: c.f("temp", d.temp),
            tint: c.f("tint", d.tint),
            color_shadows: c.v3("colorShadows"),
            color_mids: c.v3("colorMids"),
            color_highs: c.v3("colorHighs"),
            levels_red: c.levels("levelsRed"),
            levels_green: c.levels("levelsGreen"),
            levels_blue: c.levels("levelsBlue"),
            rotation: c.int("rotation", d.rotation),
            flip_horizontal: c.b("flipHorizontal", d.flip_horizontal),
            fine_rotation: c.f("fineRotation", d.fine_rotation),
            analysis_rect: c.get("analysisRect"),
            analysis_rect_fine_rotation: c
                .f("analysisRectFineRotation", d.analysis_rect_fine_rotation),
            crop_rect: c.get("cropRect"),
        })
    }
}

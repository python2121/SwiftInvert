//! Port of `SidecarCodecTests.swift` (+ the `HistoryLabelTests` field sweep, the sidecar
//! halves of `LevelsRemapTests` and `DensityChromaTests`): the sidecar codec's standing
//! contracts, exhaustively — the drift-catcher for the struct as a whole, so "added a field,
//! forgot a list" fails loudly. Exact gates (the Swift suite compares with `==`).

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use std::collections::BTreeSet;

use negkit::{ExposureSettings, NormalizedRect};
use serde_json::{Map, Value, json};

/// Every stored property set to a non-default value (the Swift `fullyMutated()`).
fn fully_mutated() -> ExposureSettings {
    let mut s = ExposureSettings::default();
    s.density = 0.8;
    s.grade = 90.0;
    s.wb_cyan = 0.1;
    s.wb_magenta = -0.2;
    s.wb_yellow = 0.3;
    s.auto_exposure = false;
    s.auto_normalize_contrast = false;
    s.cast_removal_strength = 0.9;
    s.white_point_offset = 0.05;
    s.black_point_offset = -0.04;
    s.toe = 0.4;
    s.toe_width = 3.0;
    s.shoulder = -0.3;
    s.shoulder_width = 2.0;
    s.paper_dmin = true;
    s.true_black = false;
    s.exposure_stops = 0.5;
    s.shadows = 0.7;
    s.shadow_contrast = -0.6;
    s.dark_shadows = 0.2;
    s.highlights = -0.8;
    s.highlight_contrast = 0.4;
    s.overall_contrast = 0.3;
    s.vibrance = 1.2;
    s.skin_protection = 0.8;
    s.print_saturation = 1.3;
    s.separation_damping = 0.6;
    s.contrast_mask = 0.35;
    s.mask_spacer = 5.5;
    s.hue_trim = -12.5;
    s.saturation = 0.9;
    s.red_hue = -0.25;
    s.red_saturation = 1.1;
    s.yellow_hue = 0.6;
    s.yellow_saturation = 0.8;
    s.green_hue = -0.4;
    s.green_saturation = 1.3;
    s.blue_hue = 0.7;
    s.blue_saturation = 0.6;
    s.pre_saturation = 1.0;
    s.temp = 0.35;
    s.tint = -0.15;
    s.color_shadows = [0.1, -0.2, 0.3];
    s.color_mids = [-0.1, 0.2, -0.3];
    s.color_highs = [0.05, 0.15, -0.25];
    s.levels_red = vec![[0.2, 0.35], [0.7, 0.75]];
    s.levels_green = vec![[0.6, 0.5]];
    s.levels_blue = vec![[0.4, 0.45]];
    s.rotation = 90;
    s.flip_horizontal = true;
    s.fine_rotation = 1.5;
    s.analysis_rect = Some(NormalizedRect::new(0.1, 0.2, 0.5, 0.4));
    s.analysis_rect_fine_rotation = 2.5;
    s.crop_rect = Some(NormalizedRect::new(0.05, 0.05, 0.9, 0.85));
    s
}

/// One single-field mutation per stored property (the Swift `HistoryLabelTests.mutations`).
fn mutations() -> Vec<(&'static str, fn(&mut ExposureSettings))> {
    vec![
        ("density", |s| s.density = 0.8),
        ("grade", |s| s.grade = 90.0),
        ("wbCyan", |s| s.wb_cyan = 0.1),
        ("wbMagenta", |s| s.wb_magenta = -0.2),
        ("wbYellow", |s| s.wb_yellow = 0.3),
        ("autoExposure", |s| s.auto_exposure = false),
        ("autoNormalizeContrast", |s| {
            s.auto_normalize_contrast = false
        }),
        ("castRemovalStrength", |s| s.cast_removal_strength = 0.9),
        ("whitePointOffset", |s| s.white_point_offset = 0.05),
        ("blackPointOffset", |s| s.black_point_offset = -0.04),
        ("toe", |s| s.toe = 0.4),
        ("toeWidth", |s| s.toe_width = 3.0),
        ("shoulder", |s| s.shoulder = -0.3),
        ("shoulderWidth", |s| s.shoulder_width = 2.0),
        ("paperDmin", |s| s.paper_dmin = true),
        ("trueBlack", |s| s.true_black = false),
        ("exposureStops", |s| s.exposure_stops = 0.5),
        ("shadows", |s| s.shadows = 0.7),
        ("shadowContrast", |s| s.shadow_contrast = -0.6),
        ("darkShadows", |s| s.dark_shadows = 0.2),
        ("highlights", |s| s.highlights = -0.8),
        ("highlightContrast", |s| s.highlight_contrast = 0.4),
        ("overallContrast", |s| s.overall_contrast = 0.3),
        ("vibrance", |s| s.vibrance = 1.2),
        ("skinProtection", |s| s.skin_protection = 0.8),
        ("printSaturation", |s| s.print_saturation = 1.3),
        ("separationDamping", |s| s.separation_damping = 0.6),
        ("contrastMask", |s| s.contrast_mask = 0.35),
        ("maskSpacer", |s| s.mask_spacer = 5.5),
        ("hueTrim", |s| s.hue_trim = -12.5),
        ("saturation", |s| s.saturation = 0.9),
        ("redHue", |s| s.red_hue = -0.25),
        ("redSaturation", |s| s.red_saturation = 1.1),
        ("yellowHue", |s| s.yellow_hue = 0.6),
        ("yellowSaturation", |s| s.yellow_saturation = 0.8),
        ("greenHue", |s| s.green_hue = -0.4),
        ("greenSaturation", |s| s.green_saturation = 1.3),
        ("blueHue", |s| s.blue_hue = 0.7),
        ("blueSaturation", |s| s.blue_saturation = 0.6),
        ("preSaturation", |s| s.pre_saturation = 1.0),
        ("temp", |s| s.temp = 0.35),
        ("tint", |s| s.tint = -0.15),
        ("colorShadows", |s| s.color_shadows = [0.1, -0.2, 0.3]),
        ("colorMids", |s| s.color_mids = [-0.1, 0.2, -0.3]),
        ("colorHighs", |s| s.color_highs = [0.05, 0.15, -0.25]),
        ("levelsRed", |s| s.levels_red = vec![[0.2, 0.35]]),
        ("levelsGreen", |s| s.levels_green = vec![[0.6, 0.5]]),
        ("levelsBlue", |s| s.levels_blue = vec![[0.4, 0.45]]),
        ("rotation", |s| s.rotation = 90),
        ("flipHorizontal", |s| s.flip_horizontal = true),
        ("fineRotation", |s| s.fine_rotation = 1.5),
        ("analysisRect", |s| {
            s.analysis_rect = Some(NormalizedRect::new(0.1, 0.2, 0.5, 0.4))
        }),
        ("analysisRectFineRotation", |s| {
            s.analysis_rect_fine_rotation = 2.5
        }),
        ("cropRect", |s| {
            s.crop_rect = Some(NormalizedRect::new(0.05, 0.05, 0.9, 0.85))
        }),
    ]
}

fn json_map(s: &ExposureSettings) -> Map<String, Value> {
    match serde_json::to_value(s).expect("encode") {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

fn decode(text: &str) -> ExposureSettings {
    serde_json::from_str(text).expect("decode")
}

fn round_trip(s: &ExposureSettings) -> ExposureSettings {
    decode(&serde_json::to_string(s).expect("encode"))
}

/// The tripwire. A new stored property means FOUR hand-maintained lists need a line: the
/// decoder, `STORED_KEYS`, `fully_mutated()` and `mutations()`. Update them all, then this count.
#[test]
fn stored_field_count_is_pinned() {
    assert_eq!(ExposureSettings::STORED_PROPERTY_COUNT, 54);
    assert_eq!(ExposureSettings::STORED_KEYS.len(), 54);
    let keys: BTreeSet<&str> = ExposureSettings::STORED_KEYS.iter().copied().collect();
    assert_eq!(keys.len(), 54, "STORED_KEYS has a duplicate");
    // The serialized key set of a fully-populated value IS the stored key list — in the
    // Swift declaration order (serde emits fields in declaration order).
    let mutated = json_map(&fully_mutated());
    let serialized: Vec<&str> = mutated.keys().map(String::as_str).collect();
    let mut expected: Vec<&str> = ExposureSettings::STORED_KEYS.to_vec();
    expected.sort_unstable();
    let mut got = serialized.clone();
    got.sort_unstable();
    assert_eq!(got, expected);
}

#[test]
fn fully_mutated_actually_mutates_every_field() {
    // Compare the two encodings key by key: any key whose value matches the default's means
    // fully_mutated() missed that field. Optionals (rects) are absent from the default
    // encoding, so presence alone proves mutation there.
    let defaults = json_map(&ExposureSettings::default());
    let mutated = json_map(&fully_mutated());
    assert_eq!(mutated.len(), 54);
    assert_eq!(
        defaults.len(),
        52,
        "a default encodes without the two nil rects"
    );
    for (key, value) in &mutated {
        assert_ne!(
            defaults.get(key),
            Some(value),
            "fully_mutated() left {key} at its default"
        );
    }
}

#[test]
fn every_field_survives_the_round_trip() {
    let original = fully_mutated();
    assert_eq!(round_trip(&original), original);
}

/// The HistoryLabelTests sweep, expressed on the codec: every stored property, mutated
/// alone, changes the struct, changes EXACTLY its own JSON key, and round-trips.
#[test]
fn mutation_list_covers_every_stored_property() {
    let muts = mutations();
    assert_eq!(muts.len(), ExposureSettings::STORED_PROPERTY_COUNT);
    let names: BTreeSet<&str> = muts.iter().map(|(n, _)| *n).collect();
    let keys: BTreeSet<&str> = ExposureSettings::STORED_KEYS.iter().copied().collect();
    assert_eq!(names, keys);
}

#[test]
fn every_field_change_is_visible_in_the_json() {
    let base = ExposureSettings::default();
    let base_json = json_map(&base);
    for (field, mutate) in mutations() {
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(changed, base, "{field} mutation is a no-op");
        let changed_json = json_map(&changed);
        let differing: Vec<&String> = changed_json
            .iter()
            .filter(|(k, v)| base_json.get(*k) != Some(*v))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            differing,
            vec![field],
            "{field} must change exactly its own key"
        );
        assert_eq!(round_trip(&changed), changed, "{field} round trip");
    }
}

/// The back-compat contract: sidecars written before a control existed omit its key, and
/// every missing key must decode to the field default — for ALL fields at once.
#[test]
fn empty_sidecar_decodes_to_all_defaults() {
    assert_eq!(decode("{}"), ExposureSettings::default());
}

#[test]
fn partial_sidecar_keeps_unmentioned_defaults() {
    let decoded = decode(r#"{"density": 0.7, "grade": 140, "rotation": 180}"#);
    let mut expected = ExposureSettings::default();
    expected.density = 0.7;
    expected.grade = 140.0;
    expected.rotation = 180;
    assert_eq!(decoded, expected);
}

/// A future field's key must not break today's decoder (forward compat), and neither may a
/// retired one (the old dye-separation control's key is just unknown now).
#[test]
fn unknown_and_retired_keys_are_ignored() {
    let decoded = decode(r#"{"density": 0.9, "someFutureControl": 42, "dyeSeparation": 0.4}"#);
    let mut expected = ExposureSettings::default();
    expected.density = 0.9;
    assert_eq!(decoded, expected);
}

/// Swift wraps every field decode in `try?`: a wrong-typed or null value falls back to THAT
/// field's default while the rest of the sidecar still decodes.
#[test]
fn wrong_typed_and_null_values_fall_back_per_field() {
    let decoded = decode(
        r#"{"density": "bright", "grade": null, "autoExposure": "yes", "colorMids": [1, 2],
            "levelsRed": "none", "rotation": 1.5, "cropRect": {"x": 0.1}, "toe": 0.4}"#,
    );
    let mut expected = ExposureSettings::default();
    expected.toe = 0.4;
    assert_eq!(decoded, expected);
    // Integers decode into Double fields (Swift's JSONDecoder does the same).
    assert_eq!(decode(r#"{"grade": 140}"#).grade, 140.0);
    // Only a non-object payload is an error.
    assert!(serde_json::from_str::<ExposureSettings>("[1, 2]").is_err());
}

/// The exact wire format: camelCase keys, SIMD3 as a 3-array, SIMD2 anchors as 2-arrays,
/// NormalizedRect as `{x, y, width, height}`, nil rects omitted.
#[test]
fn wire_format_matches_swift_codable() {
    let mut s = ExposureSettings::default();
    s.color_shadows = [0.1, -0.2, 0.3];
    s.levels_green = vec![[0.6, 0.5]];
    s.crop_rect = Some(NormalizedRect::new(0.05, 0.05, 0.9, 0.85));
    let m = json_map(&s);
    assert_eq!(m["colorShadows"], json!([0.1, -0.2, 0.3]));
    assert_eq!(m["levelsGreen"], json!([[0.6, 0.5]]));
    assert_eq!(m["levelsRed"], json!([]));
    assert_eq!(
        m["cropRect"],
        json!({"x": 0.05, "y": 0.05, "width": 0.9, "height": 0.85})
    );
    assert!(!m.contains_key("analysisRect"));
    assert_eq!(m["autoExposure"], json!(true));
    assert_eq!(m["rotation"], json!(0));
    assert_eq!(m["preSaturation"], json!(1.15));
    assert_eq!(m["skinProtection"], json!(0.5));
    assert_eq!(m["trueBlack"], json!(true));
    assert_eq!(m["paperDmin"], json!(false));
    // A Swift-written sidecar body (JSONEncoder spells 1.0 as 1) decodes identically.
    let swift = r#"{"density":1,"grade":115,"colorShadows":[0.1,-0.2,0.3],"levelsGreen":[[0.6,0.5]],
                    "cropRect":{"x":0.05,"y":0.05,"width":0.9,"height":0.85}}"#;
    assert_eq!(decode(swift), s);
}

/// Never asserted anywhere before the Swift suite (audit 2026-07-15): the angle an analysis
/// region was drawn at must survive the sidecar.
#[test]
fn analysis_rect_fine_rotation_round_trips() {
    let mut s = ExposureSettings::default();
    s.analysis_rect = Some(NormalizedRect::new(0.2, 0.2, 0.4, 0.4));
    s.analysis_rect_fine_rotation = -1.75;
    assert_eq!(round_trip(&s).analysis_rect_fine_rotation, -1.75);
    // And pre-straighten sidecars default it to 0.
    assert_eq!(decode("{}").analysis_rect_fine_rotation, 0.0);
}

/// `LevelsRemapTests.sidecarRoundTripAndLegacyDecodes`.
#[test]
fn levels_sidecar_round_trip_and_legacy_decodes() {
    let mut s = ExposureSettings::default();
    s.levels_red = vec![[0.25, 0.4], [0.6, 0.7]];
    s.levels_blue = vec![[0.7, 0.6]];
    assert_eq!(round_trip(&s), s);
    // Pre-levels sidecars decode to identity.
    let legacy = decode("{}");
    assert!(
        legacy.levels_red.is_empty()
            && legacy.levels_green.is_empty()
            && legacy.levels_blue.is_empty()
    );
    // The brief single-point era ([x, y] scalar pair) decodes as one anchor; identity pairs drop.
    let migrated = decode(r#"{"levelsRed": [0.2, 0.45], "levelsGreen": [0.5, 0.5]}"#);
    assert_eq!(migrated.levels_red, vec![[0.2, 0.45]]);
    assert!(migrated.levels_green.is_empty());
}

/// `DensityChromaTests.sidecarRoundTripAndLegacyDefaults` (the rest of that suite drives the
/// CPU reference chain — wave 2).
#[test]
fn print_saturation_sidecar_round_trip_and_legacy_default() {
    let mut s = ExposureSettings::default();
    s.print_saturation = 1.4;
    assert_eq!(round_trip(&s), s);
    assert_eq!(decode("{}").print_saturation, 1.0);
}

/// The documented defaults (PLAN.md §3.2 / CLAUDE.md), spelled out so a default change is a
/// deliberate edit here too.
#[test]
fn defaults_match_the_swift_struct() {
    let d = ExposureSettings::default();
    assert_eq!(d.density, 1.0);
    assert_eq!(d.grade, 115.0);
    assert!(d.auto_exposure && d.auto_normalize_contrast);
    assert_eq!(d.cast_removal_strength, 0.5);
    assert_eq!((d.toe_width, d.shoulder_width), (2.5, 2.5));
    assert!(!d.paper_dmin && d.true_black);
    assert_eq!(
        (d.vibrance, d.saturation, d.skin_protection),
        (1.0, 1.0, 0.5)
    );
    assert_eq!(
        d.red_hue, 0.0,
        "redHue's +0.5 divergence was retired 2026-07-20"
    );
    assert_eq!(
        (
            d.red_saturation,
            d.yellow_saturation,
            d.green_saturation,
            d.blue_saturation
        ),
        (1.0, 1.0, 1.0, 1.0)
    );
    assert_eq!((d.print_saturation, d.separation_damping), (1.0, 0.0));
    assert_eq!(
        (d.contrast_mask, d.mask_spacer, d.hue_trim),
        (0.0, 4.0, 0.0)
    );
    assert_eq!(
        d.pre_saturation, 1.15,
        "SwiftInvert-only default; parity tests pin 1.0"
    );
    assert_eq!(
        (d.rotation, d.flip_horizontal, d.fine_rotation),
        (0, false, 0.0)
    );
    assert!(d.analysis_rect.is_none() && d.crop_rect.is_none());
}

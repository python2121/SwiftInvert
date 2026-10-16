//! `HistoryLabelTests` (Tests/SwiftInvertTests/HistoryLabelTests.swift — the field sweep that
//! makes a missed label loud) + the undo/redo rules of session-spec §10.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

use std::time::{Duration, Instant};

use negkit::{ExposureSettings, NormalizedRect};
use negsession::history::{PendingCommit, labels};
use negsession::{History, history_label};

type Mutation = (&'static str, fn(&mut ExposureSettings));

/// One single-field mutation per stored property of ExposureSettings.
fn mutations() -> Vec<Mutation> {
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

/// The tripwire's other half: a new field must get a mutation here (the count is pinned to
/// negkit's stored-property count), which then forces a `history_label` line.
#[test]
fn mutation_list_covers_every_stored_property() {
    let m = mutations();
    assert_eq!(m.len(), ExposureSettings::STORED_PROPERTY_COUNT);
    assert_eq!(m.len(), 54);
    let names: Vec<&str> = m.iter().map(|(n, _)| *n).collect();
    assert_eq!(names.len(), ExposureSettings::STORED_KEYS.len());
    for key in ExposureSettings::STORED_KEYS {
        assert!(names.contains(&key), "mutation list misses {key}");
    }
}

#[test]
fn every_field_change_gets_a_real_label() {
    for (field, mutate) in mutations() {
        let base = ExposureSettings::default();
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(changed, base, "{field} mutation is a no-op");
        assert_ne!(
            history_label(&base, &changed),
            "Edit",
            "history_label misses {field}"
        );
    }
}

#[test]
fn label_formatting() {
    let stock = ExposureSettings::default();
    let mut s = stock.clone();
    s.density = 0.8; // Brightness shows the user-facing 2 − density.
    assert_eq!(history_label(&stock, &s), "Brightness 1.20");

    let mut two = stock.clone();
    two.grade = 90.0;
    two.true_black = false;
    assert_eq!(history_label(&stock, &two), "Grade 90, True black off");

    let mut many = two.clone();
    many.shadows = 1.0;
    assert_eq!(history_label(&stock, &many), "Multiple changes");
    assert_eq!(history_label(&stock, &stock), "Edit");

    // The per-field formats: %+.2f, %.3f, %.1f°, on/off, value-less names.
    let mut s = stock.clone();
    s.exposure_stops = 0.5;
    assert_eq!(history_label(&stock, &s), "Exposure +0.50");
    let mut s = stock.clone();
    s.red_hue = -0.25;
    assert_eq!(history_label(&stock, &s), "Red hue -0.25");
    let mut s = stock.clone();
    s.white_point_offset = 0.05;
    assert_eq!(history_label(&stock, &s), "White point 0.050");
    let mut s = stock.clone();
    s.fine_rotation = 1.5;
    assert_eq!(history_label(&stock, &s), "Fine rotation 1.5°");
    let mut s = stock.clone();
    s.auto_exposure = false;
    assert_eq!(history_label(&stock, &s), "Auto exposure off");
    let mut s = stock.clone();
    s.wb_cyan = 0.1;
    s.wb_yellow = 0.3;
    assert_eq!(history_label(&stock, &s), "WB trim");
    let mut s = stock.clone();
    s.levels_green = vec![[0.6, 0.5]];
    s.rotation = 270;
    assert_eq!(history_label(&stock, &s), "Green levels, Rotate");
}

#[test]
fn clearing_labels_read_as_clears() {
    let stock = ExposureSettings::default();
    let mut with_crop = stock.clone();
    with_crop.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.8, 0.8));
    assert_eq!(history_label(&stock, &with_crop), "Crop");
    assert_eq!(history_label(&with_crop, &stock), "Crop cleared");

    let mut with_region = stock.clone();
    with_region.analysis_rect = Some(NormalizedRect::new(0.2, 0.2, 0.5, 0.5));
    assert_eq!(history_label(&stock, &with_region), "Analysis region");
    assert_eq!(
        history_label(&with_region, &stock),
        "Analysis region cleared"
    );
}

/// The audit-found bug (2026-07-15): a change to only the region's drawn angle used to fall
/// through to "Edit".
#[test]
fn region_angle_change_is_a_region_edit() {
    let mut a = ExposureSettings::default();
    a.analysis_rect = Some(NormalizedRect::new(0.2, 0.2, 0.5, 0.5));
    let mut b = a.clone();
    b.analysis_rect_fine_rotation = 1.5;
    assert_eq!(history_label(&a, &b), "Analysis region");
}

// ───────────────────────────── the history state machine ─────────────────────────────

#[test]
fn seeded_with_the_original_conversion() {
    let h = History::new(ExposureSettings::default());
    assert_eq!(h.entries().len(), 1);
    assert_eq!(h.entries()[0].label, "Original conversion");
    assert_eq!(h.index(), 0);
    assert!(!h.can_undo() && !h.can_redo());
    assert!(!h.has_uncommitted_edit(&ExposureSettings::default()));
}

#[test]
fn commit_diffs_truncates_and_ignores_no_ops() {
    let stock = ExposureSettings::default();
    let mut h = History::new(stock.clone());
    // No change → no entry, and an armed label is disarmed.
    h.set_pending_label(Some("Rotate 90° CW"));
    assert!(!h.commit(&stock));
    assert_eq!(h.pending_label(), None);

    let mut a = stock.clone();
    a.grade = 90.0;
    assert!(h.commit(&a));
    assert_eq!(h.entries()[1].label, "Grade 90");
    let mut b = a.clone();
    b.shadows = 0.5;
    assert!(h.commit(&b));
    assert_eq!(h.entries().len(), 3);

    // Undo twice, then a new edit truncates the redo tail.
    assert_eq!(h.undo(&b), Some(a.clone()));
    assert_eq!(h.undo(&a), Some(stock.clone()));
    assert!(h.can_redo());
    let mut c = stock.clone();
    c.temp = 0.3;
    assert!(h.commit(&c));
    assert_eq!(h.entries().len(), 2);
    assert_eq!(h.entries()[1].label, "Temp 0.30");
    assert!(!h.can_redo());

    // Entry ids are unique and monotonic.
    let ids: Vec<u64> = h.entries().iter().map(|e| e.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn named_labels_win_over_the_diff() {
    let stock = ExposureSettings::default();
    let mut h = History::new(stock.clone());
    let mut rotated = stock.clone();
    rotated.rotation = 90;
    h.set_pending_label(Some(labels::ROTATE_CW));
    h.commit(&rotated);
    assert_eq!(h.current().unwrap().label, "Rotate 90° CW");
    assert_eq!(h.pending_label(), None, "consumed by the commit");
    let mut reset = stock.clone();
    reset.grade = 100.0;
    assert!(h.commit_labeled(labels::RESET_ALL, &reset));
    assert_eq!(h.current().unwrap().label, "Reset all");
}

/// Undo flushes an in-flight edit so it steps back exactly one; redo with an in-flight edit
/// commits it and stops.
#[test]
fn undo_flushes_and_redo_stops_on_uncommitted_edits() {
    let stock = ExposureSettings::default();
    let mut h = History::new(stock.clone());
    let mut a = stock.clone();
    a.grade = 90.0;
    h.commit(&a);
    assert_eq!(h.undo(&a), Some(stock.clone()));
    assert!(h.can_redo());

    // An uncommitted edit at index 0: undo commits it then reverts to stock.
    let mut live = stock.clone();
    live.shadows = 0.4;
    assert!(h.has_uncommitted_edit(&live));
    assert_eq!(h.undo(&live), Some(stock.clone()));
    assert_eq!(h.entries().len(), 2, "the flush replaced the old redo tail");
    assert_eq!(h.entries()[1].label, "Shadows 0.40");
    assert_eq!(h.index(), 0);

    // Redo applies the flushed edit.
    assert_eq!(h.redo(&stock), Some(live.clone()));
    // Redo with an uncommitted edit: commits and returns None (nothing to apply).
    let mut live2 = live.clone();
    live2.tint = 0.1;
    h.jump(0);
    assert_eq!(h.redo(&live2), None);
    assert_eq!(h.current().unwrap().settings, live2);
    assert!(!h.can_redo());
    // Nothing to undo past the seed.
    let mut h2 = History::new(stock.clone());
    assert_eq!(h2.undo(&stock), None);
    assert_eq!(h2.redo(&stock), None);
}

#[test]
fn jump_disarms_pending_label_and_rejects_bad_indices() {
    let stock = ExposureSettings::default();
    let mut h = History::new(stock.clone());
    let mut a = stock.clone();
    a.grade = 90.0;
    h.commit(&a);
    let mut b = a.clone();
    b.grade = 100.0;
    h.commit(&b);
    h.set_pending_label(Some("Crop"));
    assert_eq!(h.jump(0), Some(stock.clone()));
    assert_eq!(h.pending_label(), None);
    assert_eq!(h.jump(0), None, "already there");
    assert_eq!(h.jump(9), None, "out of range");
    assert_eq!(h.jump(2), Some(b.clone()));
    assert_eq!(h.index(), 2);
}

#[test]
fn history_is_unbounded() {
    let stock = ExposureSettings::default();
    let mut h = History::new(stock.clone());
    for i in 1..=150 {
        let mut s = stock.clone();
        s.grade = 50.0 + i as f64;
        h.commit(&s);
    }
    assert_eq!(
        h.entries().len(),
        151,
        "the Mac keeps every entry (Qt capped at 100)"
    );
}

#[test]
fn coalesce_window_and_drag_hold() {
    assert_eq!(History::COALESCE_WINDOW, Duration::from_millis(700));
    let t0 = Instant::now();
    let mut p = PendingCommit::default();
    p.schedule(t0);
    assert!(p.is_armed());
    assert!(!p.is_due(t0 + Duration::from_millis(699)));
    assert!(p.is_due(t0 + Duration::from_millis(700)));
    // A later tick re-arms from that tick.
    p.schedule(t0 + Duration::from_millis(500));
    assert!(!p.is_due(t0 + Duration::from_millis(1000)));
    assert!(p.is_due(t0 + Duration::from_millis(1200)));
    // A drag cancels the armed commit and holds scheduling; the LAST release commits once.
    assert!(!p.set_control_editing(true));
    assert!(!p.is_armed());
    p.schedule(t0);
    assert!(!p.is_armed(), "no coalesced commit while a control is held");
    assert!(!p.set_control_editing(true));
    assert!(!p.set_control_editing(false));
    assert!(p.set_control_editing(false), "the last release commits");
    assert!(!p.is_dragging());
    p.cancel();
    assert!(!p.is_armed());
}

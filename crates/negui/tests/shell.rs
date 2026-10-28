//! Shell tests: the control table against `ExposureSettings` (every stored key exactly once, or
//! explicitly unexposed; defaults equal the engine's), the histogram shape-only rule, the HQ
//! controller's threshold crossing + debounce, the composed history commit timing, and the
//! selftest smoke (a real PNG from the sample RAW when it is present).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use negkit::ExposureSettings;
use negsession::history::PendingCommit;
use negsession::{History, HqMode, RenderTier};
use negui::app::HqController;
use negui::control_spec::{
    CONTROLS, Disabled, Display, Field, KEYS_WITHOUT_SLIDER, TOGGLES, control,
};
use negui::histogram::{LOG_COMPRESSION, bar_height};

/// Every stored key is exposed exactly once by a slider or toggle (a V3 field three times, one
/// per component), or is on the explicit no-UI list — never both, never neither.
#[test]
fn control_table_covers_every_settings_field_exactly_once() {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in CONTROLS {
        *counts.entry(c.field.key()).or_default() += 1;
    }
    for t in TOGGLES {
        *counts.entry(t.toggle.key()).or_default() += 1;
    }
    for key in ExposureSettings::STORED_KEYS {
        let unexposed = KEYS_WITHOUT_SLIDER.contains(&key);
        let n = counts.get(key).copied().unwrap_or(0);
        let expected = match key {
            "colorShadows" | "colorMids" | "colorHighs" => 3,
            _ => 1,
        };
        if unexposed {
            assert_eq!(n, 0, "{key} is listed as having no UI but has {n} row(s)");
        } else {
            assert_eq!(n, expected, "{key} should have {expected} row(s), has {n}");
        }
    }
    // nothing in the table refers to a key outside the schema
    for key in counts.keys() {
        assert!(
            ExposureSettings::STORED_KEYS.contains(key),
            "{key} is not a stored key"
        );
    }
    // ids are unique
    let mut ids: Vec<&str> = CONTROLS
        .iter()
        .map(|c| c.id)
        .chain(TOGGLES.iter().map(|t| t.id))
        .collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate control ids");
}

/// The table's defaults ARE the engine's defaults (through the display transform), every
/// default sits inside its range, and the reset-⨯ rule reads unchanged on stock settings.
#[test]
fn control_defaults_match_exposure_settings_defaults() {
    let stock = ExposureSettings::default();
    for c in CONTROLS {
        assert!(c.min < c.max, "{}: empty range", c.id);
        assert!(
            c.default >= c.min && c.default <= c.max,
            "{}: default outside range",
            c.id
        );
        assert!(
            (c.shown(&stock) - c.default).abs() < 1e-12,
            "{}: table default {} ≠ engine default {}",
            c.id,
            c.default,
            c.shown(&stock)
        );
        assert!(
            !c.is_changed(&stock),
            "{}: stock settings read as changed",
            c.id
        );
    }
    for t in TOGGLES {
        assert_eq!(t.toggle.get(&stock), t.default, "{}: default", t.id);
    }
}

/// Spot-checks against controls-spec §3 (range, format, direction, disabled rules).
#[test]
fn control_table_matches_the_spec_rows() {
    let b = control("print.brightness").unwrap();
    assert_eq!((b.min, b.max, b.default), (-3.0, 4.0, 1.0));
    assert_eq!(b.display, Display::Brightness);
    let mut s = ExposureSettings::default();
    b.apply(&mut s, 1.5);
    assert!((s.density - 0.5).abs() < 1e-12, "Brightness = 2 − density");
    assert_eq!(b.format(1.5), "1.50");

    let g = control("print.grade").unwrap();
    assert_eq!((g.min, g.max, g.default), (50.0, 180.0, 115.0));
    assert_eq!(g.format(115.0), "115");

    let cm = control("print.contrastMask").unwrap();
    assert_eq!((cm.min, cm.max), (-0.5, 0.5));
    assert_eq!(cm.format(0.05), "+0.05");
    let ms = control("print.maskSpacer").unwrap();
    assert_eq!((ms.min, ms.max, ms.default), (2.0, 6.0, 4.0));
    assert_eq!(ms.format(4.0), "4.0%");
    assert_eq!(ms.disabled, Disabled::WhenEquals(Field::ContrastMask, 0.0));
    assert!(ms.is_disabled(&ExposureSettings::default()));
    let on = ExposureSettings {
        contrast_mask: 0.1,
        ..Default::default()
    };
    assert!(!ms.is_disabled(&on));

    let sd = control("color.separationDamping").unwrap();
    assert!(
        sd.is_disabled(&ExposureSettings::default()),
        "disabled at printSaturation 1.0"
    );
    let hue = control("color.hueTrim").unwrap();
    assert_eq!((hue.min, hue.max), (-30.0, 30.0));
    assert_eq!(hue.format(-2.5), "-2.5°");
    let ex = control("exposure.stops").unwrap();
    assert_eq!(ex.format(0.25), "+0.25");
    let wp = control("tone.whitePoint").unwrap();
    assert_eq!((wp.min, wp.max), (-0.3, 0.3));
    assert_eq!(wp.format(0.1234), "0.123");
    let ps = control("color.preSaturation").unwrap();
    assert_eq!((ps.min, ps.max, ps.default), (0.5, 2.0, 1.15));
    let rh = control("mixer.red.hue").unwrap();
    assert_eq!((rh.min, rh.max, rh.default), (-1.5, 1.5, 0.0));
    let rs = control("mixer.red.saturation").unwrap();
    assert_eq!((rs.min, rs.max, rs.default), (0.0, 2.0, 1.0));
    let sc = control("exposure.shadowContrast").unwrap();
    assert_eq!((sc.min, sc.max), (-3.0, 6.0));
}

/// Bars are a function of the histogram's SHAPE only: scaling every count ×15 (the HQ tier
/// bins ~15× the pixels) changes no bar, in log and linear.
#[test]
fn histogram_bar_height_is_shape_only() {
    let counts = [0u32, 1, 7, 42, 1_000, 65_535, 1_234_567];
    let max = 1_234_567u32;
    for &c in &counts {
        for log in [true, false] {
            let a = bar_height(c, max, log);
            let b = bar_height(c * 15, max * 15, log);
            assert!((a - b).abs() < 1e-6, "count {c} log {log}: {a} vs {b}");
        }
    }
    // the fixed-constant form, not log1p(count)/log1p(max)
    let f = 0.5f64;
    let expect = ((f * LOG_COMPRESSION).ln_1p() / LOG_COMPRESSION.ln_1p()) as f32;
    assert!((bar_height(500, 1000, true) - expect).abs() < 1e-6);
    assert_eq!(bar_height(1000, 1000, true), 1.0);
    assert_eq!(bar_height(0, 1000, true), 0.0);
    assert_eq!(bar_height(250, 1000, false), 0.25);
    assert_eq!(bar_height(5, 0, false), 1.0, "max 0 → divide by max(max,1)");
}

/// HQ resolve + the zoom threshold crossing: auto engages at ≥ 2× after the 350 ms debounce,
/// disengages instantly below; on/off ignore the zoom; no selection never.
#[test]
fn hq_controller_threshold_crossing_and_debounce() {
    let t0 = Instant::now();
    let mut hq = HqController::default();
    assert_eq!(hq.mode, HqMode::Auto);
    assert!(!hq.active);
    assert_eq!(hq.tier(), RenderTier::Proxy);

    // below the threshold nothing happens
    assert!(!hq.on_zoom(1.9, true, false, t0));
    assert!(!hq.tick(t0 + Duration::from_secs(1), 1.9, true, false));
    assert!(!hq.active);

    // crossing upward arms the debounce; not active until it fires
    assert!(!hq.on_zoom(2.0, true, false, t0));
    assert!(hq.next_due().is_some());
    assert!(!hq.tick(t0 + Duration::from_millis(349), 2.0, true, false));
    assert!(!hq.active);
    assert!(
        hq.tick(t0 + Duration::from_millis(350), 2.0, true, false),
        "swap fires → render"
    );
    assert!(hq.active);
    assert_eq!(
        hq.tier(),
        RenderTier::MediumIfFree,
        "auto never requests the full decode"
    );

    // dropping back below is instant
    assert!(
        hq.on_zoom(1.5, true, false, t0 + Duration::from_secs(1)),
        "downward → render now"
    );
    assert!(!hq.active);

    // an armed upward swap is cancelled by dropping back before it fires
    assert!(!hq.on_zoom(3.0, true, false, t0 + Duration::from_secs(2)));
    assert!(!hq.on_zoom(
        1.0,
        true,
        false,
        t0 + Duration::from_secs(2) + Duration::from_millis(100)
    ));
    assert!(hq.next_due().is_none());
    assert!(!hq.tick(t0 + Duration::from_secs(3), 1.0, true, false));
    assert!(!hq.active);

    // the crop tool suppresses auto
    assert!(!hq.on_zoom(4.0, true, true, t0 + Duration::from_secs(4)));
    assert!(hq.next_due().is_none());

    // On → full regardless of zoom; Off → never
    hq.set_mode(HqMode::On, 1.0, true, false);
    assert!(hq.active);
    assert_eq!(hq.tier(), RenderTier::Full);
    assert!(!hq.on_zoom(5.0, true, false, t0), "on/off ignore the zoom");
    hq.set_mode(HqMode::Off, 5.0, true, false);
    assert!(!hq.active);
    assert_eq!(hq.tier(), RenderTier::Proxy);

    // no selection → inactive in every mode
    hq.set_mode(HqMode::On, 5.0, false, false);
    assert!(!hq.active);
    assert_eq!(HqMode::Off.next(), HqMode::Auto);
    assert_eq!(HqMode::On.next(), HqMode::Off);
}

/// The history commit rules as the app composes them, on a fake clock: non-drag edits coalesce
/// 700 ms after the LAST tick; a drag cancels the pending commit and commits once on release;
/// undo flushes an uncommitted edit first.
#[test]
fn history_commit_timing_rules() {
    let t0 = Instant::now();
    let mut settings = ExposureSettings::default();
    let mut history = History::new(settings.clone());
    let mut pending = PendingCommit::default();

    // two quick ticks → one entry, 700 ms after the second
    settings.exposure_stops = 0.1;
    pending.schedule(t0);
    settings.exposure_stops = 0.2;
    pending.schedule(t0 + Duration::from_millis(300));
    assert!(
        !pending.is_due(t0 + Duration::from_millis(900)),
        "re-armed by the second tick"
    );
    assert!(pending.is_due(t0 + Duration::from_millis(1000)));
    pending.cancel();
    assert!(history.commit(&settings));
    assert_eq!(history.entries().len(), 2);
    assert_eq!(history.entries()[1].label, "Exposure +0.20");

    // a drag: begin cancels any armed commit, ticks during the drag never arm one, the last
    // release commits exactly once
    settings.shadows = 0.5;
    pending.schedule(t0 + Duration::from_secs(2));
    assert!(pending.is_armed());
    assert!(!pending.set_control_editing(true));
    assert!(
        !pending.is_armed(),
        "drag begin cancelled the coalesced commit"
    );
    settings.shadows = 0.6;
    pending.schedule(t0 + Duration::from_secs(2));
    assert!(
        !pending.is_armed(),
        "no coalesced commit while a drag is held"
    );
    settings.shadows = 0.7;
    assert!(
        pending.set_control_editing(false),
        "last release → commit now"
    );
    assert!(history.commit(&settings));
    assert_eq!(history.entries().len(), 3);
    assert_eq!(history.entries()[2].label, "Shadows 0.70");

    // nested drags (two controls held) commit on the LAST release only
    assert!(!pending.set_control_editing(true));
    assert!(!pending.set_control_editing(true));
    assert!(!pending.set_control_editing(false));
    assert!(pending.set_control_editing(false));

    // undo with an uncommitted edit flushes it first, then steps back one
    settings.grade = 120.0;
    let restored = history.undo(&settings).unwrap();
    assert_eq!(
        history.entries().len(),
        4,
        "the uncommitted edit became an entry"
    );
    assert_eq!(restored.shadows, 0.7);
    assert_eq!(restored.grade, 115.0);
    assert_eq!(history.index(), 2);

    // redo walks forward; a fresh edit truncates the tail
    assert!(history.redo(&restored).is_some());
    assert_eq!(history.index(), 3);
    let mut edited = history.current().unwrap().settings.clone();
    history.jump(1);
    edited.toe = 0.3;
    assert!(history.commit(&edited));
    assert_eq!(history.entries().len(), 3, "redo tail truncated");
    assert!(!history.can_redo());
}

/// Selftest smoke: the whole shell rendered headlessly from the sample RAW's folder writes a
/// non-empty PNG of the expected size. Skipped (with a note) when the sample is absent.
#[cfg(feature = "selftest")]
#[test]
fn selftest_writes_a_screenshot() {
    let sample = std::path::Path::new("/home/deck/Pictures/IMG_0420.CR3");
    if !sample.is_file() {
        eprintln!("skip: sample RAW {} not present", sample.display());
        return;
    }
    // A private folder with a COPY of the sample (never the user's folder: the library scan is
    // read-only, but the test must not depend on what else lives there).
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("negui-selftest");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(sample, dir.join("IMG_0420.CR3")).unwrap();
    let out = dir.join("shell.png");
    let size = negui::selftest::run_selftest(&out, dir.clone(), Duration::from_secs(120)).unwrap();
    assert_eq!(size, [1440, 900]);
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.len() > 10_000, "PNG is {} bytes", bytes.len());
    assert!(dir.join("shell_controls.png").is_file());
    // no sidecar was written next to the copy (the Qt rule: selftest never writes)
    assert!(!dir.join("IMG_0420.swiftinvert.json").exists());
    let img = image::open(&out).unwrap().to_rgba8();
    // the canvas region holds a real picture, not the empty canvas colour
    let distinct: std::collections::HashSet<[u8; 3]> = (0..200)
        .map(|k| {
            let p = img.get_pixel(500 + (k % 20) * 20, 100 + (k / 20) * 60);
            [p[0], p[1], p[2]]
        })
        .collect();
    assert!(
        distinct.len() > 20,
        "canvas looks flat: {} distinct colours",
        distinct.len()
    );
}

/// The library grid never leaves whitespace beside its thumbnails: at every panel width the
/// columns plus gaps fill the available width (the cap can't bite before a column is added).
#[test]
fn library_grid_cells_fill_the_width_at_every_panel_size() {
    use negui::library::{CELL_GAP, CELL_MAX, CELL_MIN};
    let mut avail = CELL_MIN;
    while avail <= 600.0 {
        let cols = ((avail + CELL_GAP) / (CELL_MIN + CELL_GAP))
            .floor()
            .max(1.0);
        let cell = (avail - CELL_GAP * (cols - 1.0)) / cols;
        assert!(
            cell <= CELL_MAX + 1e-3,
            "at {avail} pt a {cols}-column cell ({cell}) would be capped and leave whitespace"
        );
        avail += 0.5;
    }
}

/// Both bottom panels carry a drag bar on top, and dragging EITHER one resizes History
/// (the Crop & Rotation panel keeps its fixed height and rides on top of History).
#[test]
fn both_drag_bars_resize_the_history_panel() {
    use negui::controls::{SidebarInput, SidebarState, controls_sidebar};
    use std::cell::RefCell;
    use std::rc::Rc;

    let state = Rc::new(RefCell::new(SidebarState::default()));
    let start_h = state.borrow().prefs.history_height;
    let shared = Rc::clone(&state);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(340.0, 900.0))
        .build_ui(move |ui| {
            let mut settings = negkit::ExposureSettings::default();
            let input = SidebarInput {
                has_selection: true,
                bins: None,
                density_range: 1.0,
                character: None,
                status_message: None,
                history: None,
                tool_mode: negsession::ToolMode::None,
                baseline_held: false,
                can_undo: false,
                can_redo: false,
                is_exporting: false,
                test_strip_active: false,
            };
            let mut st = shared.borrow_mut();
            let _ = controls_sidebar(ui, &mut st, &mut settings, input);
        });
    harness.run();

    // drag a bar at `y` upward by `dy` points
    let drag = |harness: &mut egui_kittest::Harness<'_>, y: f32, dy: f32| {
        let from = egui::pos2(170.0, y);
        harness.event(egui::Event::PointerMoved(from));
        harness.run();
        harness.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        for k in 1..=4 {
            harness.event(egui::Event::PointerMoved(egui::pos2(
                170.0,
                y - dy * k as f32 / 4.0,
            )));
            harness.run();
        }
        let to = egui::pos2(170.0, y - dy);
        harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
    };

    // panel geometry (the harness's root ui has an 8 pt outer margin): History = height + 7 pt
    // bar at the bottom, Crop & Rotation = its content height + 7 above it
    let bottom = 900.0 - 8.0;
    let hist_top = bottom - (start_h + 7.0);
    drag(&mut harness, hist_top + 3.5, 40.0);
    let after_history_bar = state.borrow().prefs.history_height;
    assert!(
        (after_history_bar - (start_h + 40.0)).abs() < 1.0,
        "History's own bar: {start_h} → {after_history_bar}"
    );

    let hist_top = bottom - (after_history_bar + 7.0);
    let crop_top = hist_top - (negui::controls::crop_panel_height(&state.borrow().prefs) + 7.0);
    drag(&mut harness, crop_top + 3.5, 30.0);
    let after_crop_bar = state.borrow().prefs.history_height;
    assert!(
        (after_crop_bar - (after_history_bar + 30.0)).abs() < 1.0,
        "the Crop & Rotation bar also grows History: {after_history_bar} → {after_crop_bar}"
    );
}

/// At its smallest, the History panel still shows its whole header row (title, chevron,
/// undo/redo) — only the entry list beneath gives way. The list's scroll area used to keep a
/// 64 pt minimum that overflowed a short panel and pushed the header off the top.
#[test]
fn history_header_stays_visible_at_the_minimum_height() {
    use egui_kittest::kittest::Queryable;
    use negui::controls::{HISTORY_MIN_H, SidebarInput, SidebarState, controls_sidebar};
    use std::cell::Cell;
    use std::rc::Rc;

    // a long history, so the list wants far more room than the panel has
    let mut settings = negkit::ExposureSettings::default();
    let mut history = negsession::History::new(settings.clone());
    for k in 0..40 {
        settings.density = 1.0 + k as f64 * 0.01;
        history.commit(&settings);
    }
    let height = Rc::new(Cell::new(1.0f32));
    let h = Rc::clone(&height);
    let size = egui::vec2(340.0, 520.0);
    let mut state = SidebarState::default();
    let prefs = state.prefs.clone();
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            negui::theme::install(ui.ctx()); // the app's fonts and spacing
            state.prefs.history_height = h.get();
            let mut s = negkit::ExposureSettings::default();
            let input = SidebarInput {
                has_selection: true,
                bins: None,
                density_range: 1.0,
                character: None,
                status_message: None,
                history: Some(&history),
                tool_mode: negsession::ToolMode::None,
                baseline_held: false,
                can_undo: true,
                can_redo: false,
                is_exporting: false,
                test_strip_active: false,
            };
            let _ = controls_sidebar(ui, &mut state, &mut s, input);
        });

    // from "dragged all the way down" through the heights where the list has a sliver of room
    let bottom = size.y - 8.0; // the harness's 8 pt margin
    for wanted in [1.0f32, 26.0, 30.0, 40.0, 50.0, 70.0, 90.0] {
        height.set(wanted);
        harness.run();
        let panel_h = wanted.max(HISTORY_MIN_H) + 7.0; // + the drag bar
        let panel_top = bottom - panel_h;
        for label in ["History", "↶", "↷"] {
            let r = harness.get_by_label(label).rect();
            assert!(
                r.top() >= panel_top - 0.5 && r.bottom() <= bottom + 0.5,
                "height {wanted}: {label} at {r:?} is outside the History panel [{panel_top}, {bottom}]"
            );
        }
        // History keeps its reserved space: an overflowing list used to collapse it, so the Crop &
        // Rotation panel was laid out over History and painted over its header. So Crop &
        // Rotation (fixed height + its drag bar) must sit entirely above History: its title
        // within the top of that span.
        let crop_top = panel_top - (negui::controls::crop_panel_height(&prefs) + 7.0);
        let title = harness.get_by_label("Crop & Rotation").rect();
        assert!(
            title.top() >= crop_top - 0.5 && title.top() <= crop_top + 24.0,
            "height {wanted}: Crop & Rotation's title at {title:?} isn't at its panel's top {crop_top} — it slid over History (top {panel_top})"
        );
    }
}

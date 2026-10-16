//! Darkroom UI tests (wave 4 / U3): the Interactive Histogram's grab-or-plant / move rules, the
//! zone-placement pin bookkeeping, the test strip's axis labels and pick, the densitometer
//! read-out strings and strip shading, the overlay's region edges — and, when the sample RAW is
//! present, the whole app driven headlessly: test strip build → double-click pick (one "Test
//! strip pick" entry) → ladder rotation, zone overlay + probe, placement arm/pin/solve and the
//! clear-on-edit rule.

use std::time::{Duration, Instant};

use negkit::ExposureSettings;
use negkit::densitometry::{Reading, mid_gray_encoded};
use negkit::derive::{LEVELS_CLAMP, LEVELS_MAX_POINTS};
use negkit::reference::levels_inverse_remap;
use negkit::test_strip;
use negkit::zone_grid::Grid;
use negkit::zone_placement::{MAX_PINS, Pin};
use negui::app::{AppOptions, Command, NegApp};
use negui::interactive_histogram::{GAP, GRAB_TOLERANCE, grab_or_plant, move_anchor};
use negui::teststrip::{axis_label, patch_label, picked_settings, preview_badge};
use negui::zone_placement::{insert_pin, moved_pin};
use negui::zones::{
    Readout, frame_to_bitmap, marked_cell, region_edges, sampled_zone_grid, strip_shade,
};

// ───────────────────────────── interactive histogram ─────────────────────────────

#[test]
fn plant_on_identity_puts_input_at_the_displayed_tone() {
    let (pts, i) = grab_or_plant(&[], 0.4).unwrap();
    assert_eq!(i, 0);
    assert_eq!(pts, vec![[0.4, 0.4]]);
    // endpoint clamp on the input
    let (pts, _) = grab_or_plant(&[], 0.005).unwrap();
    assert!((pts[0][0] - LEVELS_CLAMP).abs() < 1e-12);
}

#[test]
fn plant_inverts_the_remap_in_effect() {
    let existing = vec![[0.5, 0.7]];
    let x = 0.3;
    let (pts, i) = grab_or_plant(&existing, x).unwrap();
    assert_eq!(i, 0, "inserted before the existing anchor");
    let want = levels_inverse_remap(x, &existing);
    assert!((pts[0][0] - want).abs() < 1e-12);
    assert_eq!(pts[0][1], x);
    assert_eq!(pts[1], [0.5, 0.7]);
}

#[test]
fn press_near_an_anchor_line_grabs_it() {
    let existing = vec![[0.3, 0.35], [0.6, 0.62]];
    let (pts, i) = grab_or_plant(&existing, 0.62 + GRAB_TOLERANCE * 0.9).unwrap();
    assert_eq!(i, 1);
    assert_eq!(pts, existing, "a grab plants nothing");
}

#[test]
fn full_channel_grabs_the_nearest_and_no_room_refuses() {
    let full: Vec<[f64; 2]> = (0..LEVELS_MAX_POINTS)
        .map(|k| {
            let v = 0.1 + 0.1 * k as f64;
            [v, v]
        })
        .collect();
    let (pts, i) = grab_or_plant(&full, 0.33).unwrap();
    assert_eq!(pts.len(), LEVELS_MAX_POINTS);
    assert_eq!(i, 2, "nearest output to 0.33 is 0.3");
    // two anchors closer than 2·GAP on the input axis leave no room between them
    let tight = vec![[0.5, 0.2], [0.5 + GAP * 1.5, 0.8]];
    assert!(grab_or_plant(&tight, 0.5).is_none());
}

#[test]
fn move_clamps_between_neighbours_and_endpoints() {
    let pts = vec![[0.2, 0.2], [0.5, 0.5], [0.8, 0.8]];
    let moved = move_anchor(&pts, 1, 0.95).unwrap();
    assert!((moved[1][1] - (0.8 - GAP)).abs() < 1e-12);
    let moved = move_anchor(&pts, 1, 0.0).unwrap();
    assert!((moved[1][1] - (0.2 + GAP)).abs() < 1e-12);
    let moved = move_anchor(&pts, 0, 0.0).unwrap();
    assert!((moved[0][1] - LEVELS_CLAMP.max(GAP)).abs() < 1e-12);
    assert_eq!(moved[0][0], 0.2, "inputs never move");
    assert!(move_anchor(&pts, 1, 0.5).is_none(), "no-op");
    assert!(move_anchor(&pts, 9, 0.5).is_none(), "out of range");
}

// ───────────────────────────── zone placement ─────────────────────────────

fn pin(nx: f64, ny: f64, luma: f64) -> Pin {
    Pin::new(nx, ny, [luma; 3], luma, 5.0)
}

#[test]
fn pins_append_then_replace_the_nearest() {
    let mut pins = Vec::new();
    for k in 0..MAX_PINS {
        insert_pin(&mut pins, pin(0.1 + 0.3 * k as f64, 0.5, 0.5));
    }
    assert_eq!(pins.len(), MAX_PINS);
    insert_pin(&mut pins, pin(0.42, 0.5, 0.9));
    assert_eq!(pins.len(), MAX_PINS);
    assert_eq!(pins[1].nx, 0.42, "the pin nearest (0.4) was replaced");
    assert_eq!(pins[0].nx, 0.1);
    assert_eq!(pins[2].nx, 0.7);
}

#[test]
fn a_moved_pin_keeps_an_asked_zone_and_resnaps_otherwise() {
    let mut asked = pin(0.2, 0.2, 0.4);
    asked.retargeted = true;
    asked.target_zone = 7.0;
    let mut sample = pin(0.3, 0.3, 0.6);
    sample.target_zone = 4.0 + 1.0 / 3.0;
    sample.mask_val = 0.01;
    let m = moved_pin(&asked, &sample);
    assert_eq!((m.nx, m.ny, m.val_luma, m.mask_val), (0.3, 0.3, 0.6, 0.01));
    assert_eq!(m.target_zone, 7.0);
    assert!(m.retargeted);
    let free = pin(0.2, 0.2, 0.4);
    assert_eq!(moved_pin(&free, &sample).target_zone, sample.target_zone);
}

// ───────────────────────────── test strip ─────────────────────────────

#[test]
fn axis_labels_follow_the_orientation_and_accent_the_current_rung() {
    let base = ExposureSettings::default();
    // orientation 0: density across (brightness = 2 − density), grade down
    let cols: Vec<(String, bool)> = (0..5).map(|c| axis_label(c, true, 0, &base)).collect();
    assert_eq!(cols[0].0, "0.4");
    assert_eq!(cols[4].0, "1.6");
    let rows: Vec<String> = (0..5).map(|r| axis_label(r, false, 0, &base).0).collect();
    assert_eq!(rows, ["R75", "R95", "R115", "R135", "R155"]);
    let nearest = test_strip::nearest_cell(base.density, base.grade);
    assert!(cols[nearest.col].1, "the base density's column is accented");
    assert!(axis_label(nearest.row, false, 0, &base).1);
    // orientation 1: the columns carry grade
    assert!(axis_label(0, true, 1, &base).0.starts_with('R'));
    assert!(!axis_label(0, false, 1, &base).0.starts_with('R'));
}

#[test]
fn a_pick_sets_density_and_grade_only() {
    let mut current = ExposureSettings::default();
    current.exposure_stops = 0.7;
    current.auto_exposure = !current.auto_exposure;
    let picked = picked_settings(&current, 0, 4, 0).unwrap();
    let cell = test_strip::cell(0, 4, 0).unwrap();
    assert_eq!((picked.density, picked.grade), (cell.density, cell.grade));
    assert_eq!(picked.exposure_stops, 0.7);
    assert_eq!(picked.auto_exposure, current.auto_exposure);
    assert!(picked_settings(&current, 5, 0, 0).is_none());
    assert!(preview_badge(0, 4, 0).starts_with("Brightness 1.6 · Grade R75"));
}

/// The hover pill and the preview badge use the same "Grade R95" caption (review 8 #17).
#[test]
fn hover_pill_and_preview_badge_share_the_caption() {
    for (row, col) in [(0, 0), (2, 2), (4, 1)] {
        let cell = test_strip::cell(row, col, 0).unwrap();
        let pill = patch_label(&cell);
        assert!(
            pill.contains(&format!("Grade R{:.0}", cell.grade)),
            "{pill}"
        );
        assert!(preview_badge(row, col, 0).starts_with(&pill));
    }
}

// ───────────────────────────── densitometer / overlay ─────────────────────────────

#[test]
fn readout_text_forms() {
    assert_eq!(Readout::default().text(), "hover to meter");
    let r = Reading {
        rgb: [0.5; 3],
        print_density: 0.734,
        zone: 5.0,
    };
    let readout = Readout {
        reading: Some(r),
        armed: None,
    };
    assert_eq!(readout.text(), "D 0.73 · Zone V");
    let armed = Readout {
        reading: Some(r),
        armed: Some(7.0),
    };
    assert_eq!(armed.text(), "Zone VII → click photo");
    assert_eq!(marked_cell(4.9), 4);
    assert_eq!(marked_cell(-1.0), 0);
    assert_eq!(marked_cell(12.0), 10);
}

#[test]
fn strip_shades_hinge_at_mid_gray() {
    assert_eq!(strip_shade(0), 0.0);
    assert!((strip_shade(5) - mid_gray_encoded()).abs() < 1e-12);
    assert!((strip_shade(10) - 1.0).abs() < 1e-12);
    for z in 0..10 {
        assert!(strip_shade(z + 1) > strip_shade(z));
    }
}

#[test]
fn frame_uv_maps_through_the_content_window() {
    let w = negkit::NormalizedRect::new(0.1, 0.0, 0.8, 1.0);
    let (u, v) = frame_to_bitmap(0.5, 0.25, &w);
    assert!((u - 0.5).abs() < 1e-12 && (v - 0.25).abs() < 1e-12);
    let (u, _) = frame_to_bitmap(0.1, 0.0, &w);
    assert!(u.abs() < 1e-12);
}

#[test]
fn overlay_draws_edges_only_between_differing_zones() {
    // 3×2: [5 5 6]
    //      [5 5 6]
    let g = Grid {
        cols: 3,
        rows: 2,
        zones: vec![5, 5, 6, 5, 5, 6],
        labels: Vec::new(),
    };
    let e = region_edges(&g);
    assert_eq!(
        e.len(),
        2,
        "one vertical edge per row between the 5s and the 6"
    );
    assert!(e.iter().all(|s| s[0].0 == 2 && s[1].0 == 2));
}

#[test]
fn sampled_zone_grid_equals_the_full_transcode() {
    // a textured sRGB bitmap at several sizes (stride 1, 2, 7)
    for (w, h) in [(120usize, 90usize), (300, 200), (1000, 700)] {
        let mut bytes = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 7 + y * 13) % 256) as u8;
                let g = ((x * 3 + (y / 5) * 29) % 256) as u8;
                bytes.extend_from_slice(&[v, g, v / 2 + g / 2, 255]);
            }
        }
        let full = negsession::zone_grid(&bytes, w, h, negsession::Encoding::Srgb).unwrap();
        let fast = sampled_zone_grid(&bytes, w, h).unwrap();
        assert_eq!(fast, full, "{w}×{h}");
    }
}

// ───────────────────────────── the app, headless ─────────────────────────────

fn sample_folder(name: &str) -> Option<std::path::PathBuf> {
    let sample = std::path::Path::new("/home/deck/Pictures/IMG_0420.CR3");
    if !sample.is_file() {
        eprintln!("skip: sample RAW {} not present", sample.display());
        return None;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::copy(sample, dir.join("IMG_0420.CR3")).ok()?;
    Some(dir)
}

type H = egui_kittest::Harness<'static, NegApp>;

fn harness(folder: std::path::PathBuf) -> H {
    let options = AppOptions {
        initial_folder: Some(folder),
        autosave: false,
        persist: false,
        renderer: None,
        presenter: None,
        backend: None,
    };
    egui_kittest::Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_max_steps(100_000)
        // real-time steps, so a double click is a double click
        .with_step_dt(1.0 / 60.0)
        .build_eframe(|cc| NegApp::new(cc, options))
}

fn until(h: &mut H, what: &str, done: impl Fn(&NegApp) -> bool) {
    let t0 = Instant::now();
    while !done(h.state()) {
        assert!(t0.elapsed() < Duration::from_secs(60), "timed out: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frame_point(h: &H, u: f32, v: f32) -> egui::Pos2 {
    let r = h.state().canvas_frame_rect().expect("frame rect");
    egui::pos2(r.left() + u * r.width(), r.top() + v * r.height())
}

fn press_release(h: &mut H, at: egui::Pos2) {
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

#[test]
fn darkroom_layers_drive_the_real_app() {
    let Some(dir) = sample_folder("negui-darkroom") else {
        return;
    };
    let mut h = harness(dir.clone());
    until(&mut h, "first frame", |a| a.selftest_ready());
    for _ in 0..4 {
        h.step();
    }

    // ── test strip: build, double-click a patch → one "Test strip pick" ──
    h.state_mut().run(Command::TestStrip);
    assert!(
        h.state().test_strip_owns_canvas(),
        "building owns the canvas"
    );
    until(&mut h, "strip", |a| a.test_strip_up());
    let p = frame_point(&h, 0.95, 0.05); // row 0, col 4 → density 0.4, grade 75
    h.hover_at(p);
    h.step();
    press_release(&mut h, p);
    press_release(&mut h, p);
    for _ in 0..6 {
        h.step();
    }
    let cell = test_strip::cell(0, 4, 0).unwrap();
    assert_eq!(h.state().settings().density, cell.density);
    assert_eq!(h.state().settings().grade, cell.grade);
    assert!(
        !h.state().test_strip_owns_canvas(),
        "a pick clears the strip"
    );
    // the commit lands after the 0.7 s coalesce
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(900) {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        h.state().current_history_label().as_deref(),
        Some("Test strip pick")
    );

    // ── while a strip is up the rotate commands turn the LADDER ──
    let rotation = h.state().settings().rotation;
    h.state_mut().run(Command::TestStrip);
    until(&mut h, "strip again", |a| a.test_strip_up());
    h.state_mut().run(Command::RotateRight);
    assert_eq!(h.state().test_strip_orientation(), 1);
    assert_eq!(
        h.state().settings().rotation,
        rotation,
        "the image did not rotate"
    );
    until(&mut h, "rotated strip", |a| a.test_strip_up());
    h.state_mut().run(Command::Escape);
    assert!(
        !h.state().test_strip_owns_canvas(),
        "Escape clears the strip"
    );
    h.state_mut().run(Command::RotateLeft); // back to orientation-free image rotation
    assert_eq!(h.state().settings().rotation, (rotation + 270) % 360);
    h.state_mut().run(Command::RotateRight);
    until(&mut h, "re-render", |a| a.canvas_frame_rect().is_some());

    // ── zone overlay + probe ──
    h.state_mut().run(Command::ToggleZones);
    assert!(h.state().zone_overlay_on());
    until(&mut h, "zone grid", |a| a.zone_grid_ready());
    let centre = frame_point(&h, 0.5, 0.5);
    h.hover_at(centre);
    for _ in 0..3 {
        h.step();
    }
    let reading = h.state().densitometer_readout().reading;
    assert!(reading.is_some(), "the probe reads the displayed bitmap");
    h.state_mut().run(Command::ToggleZones);
    assert!(!h.state().zone_grid_ready());

    // ── zone placement: arm, pin, solve; an edit clears it ──
    h.state_mut().arm_placement_zone(5.0);
    assert_eq!(h.state().densitometer_readout().armed, Some(5.0));
    h.step();
    let p = frame_point(&h, 0.5, 0.5);
    h.hover_at(p);
    press_release(&mut h, p);
    until(&mut h, "solved", |a| {
        a.placement_status()
            .is_some_and(|(n, solved, solving)| n == 1 && solved && !solving)
    });
    assert_eq!(
        h.state().densitometer_readout().armed,
        None,
        "a pin disarms"
    );
    h.state_mut().bench_nudge();
    h.step();
    assert!(
        !h.state().zone_placement_active(),
        "any edit clears the placement"
    );

    // ── interactive histogram opens on the command ──
    h.state_mut().run(Command::InteractiveHistogram);
    assert!(h.state().interactive_histogram_open());
    h.step();
    h.state_mut().close_interactive_histogram();

    // the selftest never writes sidecars
    assert!(!dir.join("IMG_0420.swiftinvert.json").exists());
}

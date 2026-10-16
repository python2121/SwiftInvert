//! Tool & dialog tests (wave 4 / U2, U2b): the pure crop-commit math — rect → box → rect
//! round-trips, the constrain rule at ±45°, near-full ⇒ no crop, `commit_fine_rotation` against
//! `negkit::crop_geometry::remap_crop`, the analysis-region commit rule (and its equality with
//! the engine's) — plus the overlay's hit-testing and clamped drags on a fake canvas, the crop
//! overlay driven through the real `canvas()` hook in a headless egui harness, export sticky
//! options through `AppPrefs` and eframe storage, the profile picker rules, and the selftest
//! crop + export smoke (sample RAW permitting).

use eframe::Storage;
use egui_kittest::kittest::Queryable;
use negkit::crop_geometry::{RotatedBox, box_fits, box_from_rect, normalized_rect, remap_crop};
use negkit::{ExposureSettings, NormalizedRect};
use negui::tools::{
    CropCommit, commit_analysis_region, commit_crop, commit_fine_rotation, crop_fit_scale,
    default_box, effective_box, is_full_frame,
};

const FRAME: [f64; 2] = [1536.0, 1024.0];

fn rect(x: f64, y: f64, w: f64, h: f64) -> NormalizedRect {
    NormalizedRect::new(x, y, w, h)
}

fn close(a: &NormalizedRect, b: &NormalizedRect, tol: f64) -> bool {
    (a.x - b.x).abs() < tol
        && (a.y - b.y).abs() < tol
        && (a.width - b.width).abs() < tol
        && (a.height - b.height).abs() < tol
}

#[test]
fn rect_box_rect_round_trips_at_several_angles() {
    let r = rect(0.1, 0.2, 0.5, 0.6);
    for deg in [0.0f64, 3.0, -12.5, 45.0, -45.0] {
        let rad = deg.to_radians();
        let b = box_from_rect(&r, FRAME, rad);
        let back = normalized_rect(b.center, b.half_extents, FRAME, rad);
        assert!(close(&r, &back, 1e-9), "{deg}°: {r:?} → {back:?}");
    }
}

#[test]
fn effective_box_stays_inside_the_rotated_frame_at_extreme_angles() {
    // A full-frame box at 0° cannot fit at 45°: constrain must shrink it inside.
    let desired = RotatedBox {
        center: [0.0, 0.0],
        half_extents: [FRAME[0] / 2.0, FRAME[1] / 2.0],
    };
    for deg in [45.0f64, -45.0, 20.0, -7.0] {
        let rad = deg.to_radians();
        let b = effective_box(desired, FRAME, rad);
        assert!(
            box_fits(b.center, b.half_extents, rad, FRAME, 1e-6),
            "{deg}°: {b:?} does not fit"
        );
        assert!(b.half_extents[0] < FRAME[0] / 2.0);
    }
    // An off-centre box: the centre is clamped and the box still fits.
    let off = RotatedBox {
        center: [700.0, -480.0],
        half_extents: [300.0, 200.0],
    };
    let rad = 45f64.to_radians();
    let b = effective_box(off, FRAME, rad);
    assert!(box_fits(b.center, b.half_extents, rad, FRAME, 1e-6));
}

#[test]
fn near_full_box_commits_as_no_crop() {
    let mut s = ExposureSettings {
        crop_rect: Some(rect(0.2, 0.2, 0.5, 0.5)),
        ..ExposureSettings::default()
    };
    let full = default_box(None, 0.0, FRAME, 0.0);
    assert!(is_full_frame(&full, FRAME, 0.0));
    // within 1 %: still "full"
    let nearly = RotatedBox {
        center: [2.0, -1.0],
        half_extents: [FRAME[0] / 2.0 * 0.996, FRAME[1] / 2.0 * 0.995],
    };
    assert!(is_full_frame(&nearly, FRAME, 0.0));
    assert_eq!(commit_crop(&mut s, nearly, FRAME, 0.0), CropCommit::Cleared);
    assert_eq!(s.crop_rect, None);
    assert_eq!(CropCommit::Cleared.label(), Some("Crop cleared"));
    // clearing twice is a no-op (label disarmed)
    assert_eq!(
        commit_crop(&mut s, nearly, FRAME, 0.0),
        CropCommit::Unchanged
    );
    // a real crop commits its normalized rect
    let partial = RotatedBox {
        center: [100.0, 50.0],
        half_extents: [400.0, 300.0],
    };
    let out = commit_crop(&mut s, partial, FRAME, 0.0);
    let CropCommit::Crop(r) = out else {
        panic!("expected a crop, got {out:?}")
    };
    assert_eq!(s.crop_rect, Some(r));
    assert!(close(
        &r,
        &rect(
            (100.0 - 400.0) / FRAME[0] + 0.5,
            (50.0 - 300.0) / FRAME[1] + 0.5,
            800.0 / FRAME[0],
            600.0 / FRAME[1]
        ),
        1e-9
    ));
    assert_eq!(CropCommit::Crop(r).label(), Some("Crop"));
}

#[test]
fn commit_fine_rotation_remaps_like_remap_crop_and_rounds() {
    let crop = rect(0.3, 0.25, 0.4, 0.5);
    let mut s = ExposureSettings {
        fine_rotation: 2.0,
        crop_rect: Some(crop),
        ..ExposureSettings::default()
    };
    assert!(commit_fine_rotation(&mut s, 9.04, FRAME));
    assert!((s.fine_rotation - 9.0).abs() < 1e-12, "rounded to 0.1°");
    let expected = remap_crop(&crop, 2f64.to_radians(), 9f64.to_radians(), FRAME);
    assert!(close(&s.crop_rect.unwrap(), &expected, 1e-12));
    // the content under the crop centre stayed put
    let before = box_from_rect(&crop, FRAME, 2f64.to_radians());
    let after = box_from_rect(&expected, FRAME, 9f64.to_radians());
    let c0 = negkit::crop_geometry::rotate(before.center, -2f64.to_radians());
    let c1 = negkit::crop_geometry::rotate(after.center, -9f64.to_radians());
    assert!((c0[0] - c1[0]).abs() < 1e-6 && (c0[1] - c1[1]).abs() < 1e-6);
    // clamped to ±45 and a no-op when nothing changes
    assert!(commit_fine_rotation(&mut s, 80.0, FRAME));
    assert_eq!(s.fine_rotation, 45.0);
    assert!(!commit_fine_rotation(&mut s, 45.0, FRAME));
    // without a crop only the angle moves
    let mut plain = ExposureSettings::default();
    assert!(commit_fine_rotation(&mut plain, -1.26, FRAME));
    assert!((plain.fine_rotation + 1.3).abs() < 1e-12);
    assert_eq!(plain.crop_rect, None);
}

#[test]
fn analysis_region_commit_records_the_drawn_angle() {
    let mut s = ExposureSettings {
        fine_rotation: 4.5,
        ..ExposureSettings::default()
    };
    assert!(!commit_analysis_region(&mut s, rect(0.1, 0.1, 0.005, 0.5)));
    assert_eq!(s.analysis_rect, None);
    assert!(commit_analysis_region(&mut s, rect(0.1, 0.1, 0.5, 0.5)));
    assert_eq!(s.analysis_rect, Some(rect(0.1, 0.1, 0.5, 0.5)));
    assert_eq!(s.analysis_rect_fine_rotation, 4.5);
}

#[test]
fn crop_fit_scale_fits_the_rotated_bounding_box() {
    let area = [1000.0, 600.0];
    let s0 = crop_fit_scale(FRAME, 0.0, area);
    assert!((s0 - 600.0 / 1024.0).abs() < 1e-12);
    let rad = 90f64.to_radians();
    let s90 = crop_fit_scale(FRAME, rad, area);
    assert!((s90 - 600.0 / 1536.0).abs() < 1e-9);
    let s45 = crop_fit_scale(FRAME, 45f64.to_radians(), area);
    let diag = (FRAME[0] + FRAME[1]) * std::f64::consts::FRAC_1_SQRT_2;
    assert!((s45 - (600.0 / diag)).abs() < 1e-9);
}

// ───────────────────────────── U2b: overlay geometry on a fake canvas ─────────────────────────────

use negui::tools::{
    CORNER_SIGNS, CropHit, CropTool, CropView, ToolAction, drag_corner, hit_test, move_box,
    normalize_in, snap_angle,
};

fn fake_view() -> CropView {
    CropView {
        center: [500.0, 400.0],
        scale: 0.5,
    }
}

fn centred_box() -> RotatedBox {
    RotatedBox {
        center: [0.0, 0.0],
        half_extents: [400.0, 300.0],
    }
}

#[test]
fn crop_view_maps_both_ways_and_fits_the_rotated_frame() {
    let v = fake_view();
    for p in [[0.0, 0.0], [123.5, -77.25], [-768.0, 512.0]] {
        let back = v.to_rotated(v.to_screen(p));
        assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3);
    }
    let area = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
    for deg in [0.0f64, 7.5, -30.0, 45.0] {
        let rad = deg.to_radians();
        let cv = CropView::for_area(area, FRAME, rad);
        // every rotated frame corner lands inside the area above the bar
        for s in CORNER_SIGNS {
            let c =
                negkit::crop_geometry::rotate([s[0] * FRAME[0] / 2.0, s[1] * FRAME[1] / 2.0], rad);
            let p = cv.to_screen(c);
            assert!(
                p.x >= area.left() - 0.01 && p.x <= area.right() + 0.01,
                "{deg}°: {p:?}"
            );
            assert!(
                p.y >= area.top() - 0.01
                    && p.y <= area.bottom() - negui::tools::CROP_BAR_HEIGHT + 0.01,
                "{deg}°: {p:?}"
            );
        }
    }
}

#[test]
fn hit_test_prefers_handles_then_the_interior() {
    let v = fake_view();
    let b = centred_box();
    let r = v.box_rect(&b);
    assert_eq!(hit_test(&b, &v, r.left_top(), 14.0), CropHit::Corner(0));
    assert_eq!(
        hit_test(&b, &v, r.right_top() + egui::vec2(6.0, -6.0), 14.0),
        CropHit::Corner(1)
    );
    assert_eq!(hit_test(&b, &v, r.right_bottom(), 14.0), CropHit::Corner(2));
    assert_eq!(hit_test(&b, &v, r.left_bottom(), 14.0), CropHit::Corner(3));
    assert_eq!(hit_test(&b, &v, r.center(), 14.0), CropHit::Inside);
    assert_eq!(
        hit_test(&b, &v, r.right_bottom() + egui::vec2(40.0, 40.0), 14.0),
        CropHit::None
    );
    // a tiny box: both handles in range, the nearer one wins
    let tiny = RotatedBox {
        center: [0.0, 0.0],
        half_extents: [10.0, 10.0],
    };
    let tr = v.box_rect(&tiny);
    assert_eq!(
        hit_test(&tiny, &v, tr.left_top() + egui::vec2(1.0, 1.0), 14.0),
        CropHit::Corner(0)
    );
}

#[test]
fn corner_drag_keeps_the_opposite_corner_and_stays_inside() {
    let b = centred_box();
    // drag the bottom-right corner inward: the top-left stays put
    let out = drag_corner(b, 2, [200.0, 100.0], 48.0, FRAME, 0.0);
    assert!((out.center[0] - out.half_extents[0] + 400.0).abs() < 1e-9);
    assert!((out.center[1] - out.half_extents[1] + 300.0).abs() < 1e-9);
    assert!((out.center[0] + out.half_extents[0] - 200.0).abs() < 1e-9);
    // past the anchor: clamped to the minimum side, never flipped
    let min = drag_corner(b, 2, [-900.0, -900.0], 48.0, FRAME, 0.0);
    assert!((min.half_extents[0] * 2.0 - 48.0).abs() < 1e-9);
    assert!((min.half_extents[1] * 2.0 - 48.0).abs() < 1e-9);
    // far outside a rotated frame: the result fits and pushed as far as it can
    for deg in [0.0f64, 12.0, -30.0] {
        let rad = deg.to_radians();
        let start = effective_box(b, FRAME, rad);
        let out = drag_corner(start, 2, [5000.0, 5000.0], 48.0, FRAME, rad);
        assert!(
            box_fits(out.center, out.half_extents, rad, FRAME, 1e-6),
            "{deg}°"
        );
        assert!(
            out.half_extents[0] >= start.half_extents[0] - 1e-9,
            "{deg}°: grew or held"
        );
    }
    // at 0° pulling past the frame edge pins the corner TO the edge (slides along it)
    let out = drag_corner(b, 2, [5000.0, 200.0], 48.0, FRAME, 0.0);
    assert!((out.center[0] + out.half_extents[0] - FRAME[0] / 2.0).abs() < 1e-3);
    assert!((out.center[1] + out.half_extents[1] - 200.0).abs() < 1e-6);
}

#[test]
fn move_slides_along_the_edges() {
    let b = RotatedBox {
        center: [0.0, 0.0],
        half_extents: [300.0, 200.0],
    };
    // free move
    let m = move_box(b, [50.0, -40.0], FRAME, 0.0);
    assert_eq!(m.center, [50.0, -40.0]);
    // x blocked at the right edge, y still follows
    let m = move_box(b, [5000.0, 100.0], FRAME, 0.0);
    assert!((m.center[0] + 300.0 - FRAME[0] / 2.0).abs() < 1e-3);
    assert!((m.center[1] - 100.0).abs() < 1e-9);
    assert_eq!(m.half_extents, b.half_extents);
    // rotated frame: always fits
    let rad = 20f64.to_radians();
    let start = effective_box(b, FRAME, rad);
    let m = move_box(start, [-3000.0, 3000.0], FRAME, rad);
    assert!(box_fits(m.center, m.half_extents, rad, FRAME, 1e-6));
}

#[test]
fn crop_tool_apply_and_cancel() {
    let base = ExposureSettings {
        fine_rotation: 1.0,
        crop_rect: Some(rect(0.2, 0.2, 0.6, 0.6)),
        ..ExposureSettings::default()
    };
    // no box drawn: only the angle moves; the committed crop is remapped
    let mut tool = CropTool::new(&base, FRAME);
    tool.angle = 3.04;
    let mut s = base.clone();
    assert_eq!(tool.apply(&mut s), None);
    assert!((s.fine_rotation - 3.0).abs() < 1e-12);
    let expected = remap_crop(
        &rect(0.2, 0.2, 0.6, 0.6),
        1f64.to_radians(),
        3f64.to_radians(),
        FRAME,
    );
    assert!(close(&s.crop_rect.unwrap(), &expected, 1e-12));
    // a drawn box commits at the snapped angle, labelled "Crop"
    let mut tool = CropTool::new(&base, FRAME);
    tool.angle = -2.0;
    tool.desired = Some(RotatedBox {
        center: [20.0, 10.0],
        half_extents: [200.0, 150.0],
    });
    let mut s = base.clone();
    assert_eq!(tool.apply(&mut s), Some("Crop"));
    assert_eq!(s.fine_rotation, -2.0);
    let r = s.crop_rect.unwrap();
    let back = box_from_rect(&r, FRAME, (-2f64).to_radians());
    assert!((back.center[0] - 20.0).abs() < 1e-6 && (back.half_extents[1] - 150.0).abs() < 1e-6);
    // cancel restores the snapshot whatever happened meanwhile
    let mut moved = s.clone();
    moved.crop_rect = None;
    assert!(tool.cancel(&mut moved));
    assert_eq!(moved.fine_rotation, 1.0);
    assert_eq!(moved.crop_rect, base.crop_rect);
    let mut same = base.clone();
    assert!(!tool.cancel(&mut same));
    // the drawn box is the effective (constrained) one, and a frame change drops it
    assert!(box_fits(
        tool.current_box().center,
        tool.current_box().half_extents,
        (-2f64).to_radians(),
        FRAME,
        1e-6
    ));
    tool.sync(&base, [1024.0, 1536.0]);
    assert_eq!(tool.desired, None);
    assert_eq!(snap_angle(f64::NAN), 0.0);
    assert_eq!(snap_angle(99.0), 45.0);
}

#[test]
fn analysis_commit_matches_the_engine_rule() {
    let r = rect(0.2, 0.3, 0.4, 0.25);
    let mut ui_side = ExposureSettings {
        fine_rotation: -3.2,
        ..ExposureSettings::default()
    };
    assert!(commit_analysis_region(&mut ui_side, r));
    let preview = negkit::RgbImage::from_vec(8, 6, vec![0.5; 8 * 6 * 3]).unwrap();
    let mut session = negsession::Session::synthetic(
        "synthetic",
        preview,
        None,
        std::sync::Arc::new(negsession::CpuRenderer),
    );
    session.set_settings(ExposureSettings {
        fine_rotation: -3.2,
        ..ExposureSettings::default()
    });
    session.commit_analysis_region(Some(r));
    assert_eq!(session.settings(), &ui_side);
    // the overlay's normalization clamps to the frame
    let fr = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
    assert_eq!(normalize_in(fr, egui::pos2(110.0, 70.0)), (0.5, 0.5));
    assert_eq!(normalize_in(fr, egui::pos2(-50.0, 500.0)), (0.0, 1.0));
}

/// The crop overlay through the real `canvas()` hook: a corner drag on the canvas moves the
/// box, the overlay pins zoom, and Apply in the in-canvas bar raises `ApplyCrop`.
#[test]
fn crop_overlay_drag_through_the_canvas_hook() {
    use negui::loupe::{CanvasInput, CanvasOverlay, LoupeState, PresentedFrame, canvas};
    use negui::tools::ToolState;

    let settings = ExposureSettings::default();
    let mut tools = ToolState {
        crop: Some(CropTool::new(&settings, FRAME)),
        analysis: None,
    };
    let mut loupe = LoupeState {
        frame: Some(PresentedFrame {
            texture: egui::TextureId::Managed(0),
            size: [1536, 1024],
        }),
        frame_size: (1536, 1024),
        display_aspect: 1.5,
        zoom: 3.0,
        ..LoupeState::default()
    };
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 700.0));
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui(|ui| {
            let input = CanvasInput {
                has_selection: true,
                developing: false,
                analyzing: false,
                baseline: false,
                error: None,
                show_grid: false,
                grid: Default::default(),
                canvas_color: Default::default(),
                tool_mode: negsession::ToolMode::Crop,
                frame_label: None,
            };
            let mut overlays: Vec<&mut dyn CanvasOverlay> = tools.overlays();
            let _ = canvas(ui, area, &mut loupe, &input, &mut overlays);
        });
    harness.run();
    drop(harness);
    assert_eq!(loupe.zoom, 1.0, "the crop tool pins the fit");

    // where the full-frame box's bottom-right handle sits on screen
    let inset = area.shrink(negui::loupe::CANVAS_INSET);
    let cv = CropView::for_area(inset, FRAME, 0.0);
    let start = tools.crop.as_ref().unwrap().current_box();
    let handle = cv.box_rect(&start).right_bottom() - egui::vec2(2.0, 2.0);
    let target = handle - egui::vec2(150.0, 100.0);

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui(|ui| {
            let input = CanvasInput {
                has_selection: true,
                developing: false,
                analyzing: false,
                baseline: false,
                error: None,
                show_grid: false,
                grid: Default::default(),
                canvas_color: Default::default(),
                tool_mode: negsession::ToolMode::Crop,
                frame_label: None,
            };
            let mut overlays: Vec<&mut dyn CanvasOverlay> = tools.overlays();
            let _ = canvas(ui, area, &mut loupe, &input, &mut overlays);
        });
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(egui::Event::PointerMoved(handle));
    harness.step();
    harness.event(button(handle, true));
    harness.step();
    for i in 1..=10 {
        harness.event(egui::Event::PointerMoved(
            handle + (target - handle) * (i as f32 / 10.0),
        ));
        harness.step();
    }
    harness.event(button(target, false));
    harness.step();
    harness.step();
    // Apply in the bar
    harness.get_by_label("Apply").click();
    harness.run();
    drop(harness);

    let tool = tools.crop.as_mut().unwrap();
    let b = tool.desired.expect("the drag drew a box");
    // top-left anchored, bottom-right moved by the drag in frame pixels
    assert!((b.center[0] - b.half_extents[0] + FRAME[0] / 2.0).abs() < 1.0);
    let moved = (start.half_extents[0] - b.half_extents[0]) * 2.0 * cv.scale;
    assert!((moved - 150.0).abs() < 4.0, "box shrank by {moved} pt");
    assert_eq!(tools.take_actions(), vec![ToolAction::ApplyCrop]);
}

// ───────────────────────────── U2b: export sticky options ─────────────────────────────

#[derive(Default)]
struct MemStorage(std::collections::HashMap<String, String>);

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn export_options_are_sticky_through_app_prefs() {
    use negsession::{ExportColorSpace, ExportDestination, ExportFormat};
    let fresh = negui::AppPrefs::default();
    assert!(fresh.export.resize, "first run: resize on");
    assert_eq!(fresh.export.max_long_edge, 3000);
    assert_eq!(fresh.export.jpeg_quality, 0.92);
    assert_eq!(fresh.export.format, ExportFormat::Jpeg);
    assert_eq!(fresh.export.color_space, ExportColorSpace::Srgb);

    let mut prefs = negui::AppPrefs::default();
    prefs.export.format = ExportFormat::Tiff16;
    prefs.export.jpeg_quality = 0.73; // survives the switch to TIFF
    prefs.export.resize = false;
    prefs.export.max_long_edge = 4500; // survives resize off
    prefs.export.color_space = ExportColorSpace::AdobeRgb;
    prefs
        .export
        .set_destination(ExportDestination::Folder("/tmp/exports".into()));
    prefs
        .export
        .set_destination(ExportDestination::NextToSource); // path remembered
    let mut storage = MemStorage::default();
    eframe::set_value(&mut storage, negui::app::PREFS_KEY, &prefs);
    let back: negui::AppPrefs = eframe::get_value(&storage, negui::app::PREFS_KEY).unwrap();
    assert_eq!(back.export, prefs.export);
    assert_eq!(
        back.export.custom_destination_path.as_deref(),
        Some(std::path::Path::new("/tmp/exports"))
    );
    // an old blob without the export key → the app default
    let mut old = MemStorage::default();
    old.set_string(negui::app::PREFS_KEY, "(library_visible: false)".into());
    let back: negui::AppPrefs = eframe::get_value(&old, negui::app::PREFS_KEY).unwrap();
    assert!(!back.library_visible);
    assert_eq!(
        back.export,
        negui::dialogs::export::default_export_options()
    );
    // out-of-range sticky values are clamped into the sheet's ranges
    let wild = negsession::ExportOptions {
        jpeg_quality: 7.0,
        max_long_edge: 3,
        ..Default::default()
    };
    let s = negui::dialogs::export::sanitize(wild);
    assert_eq!((s.jpeg_quality, s.max_long_edge), (1.0, 256));
    assert_eq!(
        negui::dialogs::export::finish_status(3, 3, 0, false),
        "Exported 3 image(s)"
    );
    assert_eq!(
        negui::dialogs::export::finish_status(3, 1, 2, false),
        "Exported 1 of 3 (2 failed)"
    );
    assert_eq!(
        negui::dialogs::export::finish_status(5, 2, 0, true),
        "Export cancelled (2 of 5 done)"
    );
}

// ───────────────────────────── U2b: profile picker rules ─────────────────────────────

#[test]
fn profile_picker_rules() {
    use negsession::ProfileStore;
    use negsession::profiles::{BUILTIN_PROFILE_ID, NONE_PROFILE_ID};
    use negui::dialogs::profiles::{
        UNTITLED, can_modify, create_profile, delete_profile, rename_profile, unique_name,
        update_profile,
    };
    let mut store = ProfileStore::new();
    assert!(!can_modify(NONE_PROFILE_ID) && !can_modify(BUILTIN_PROFILE_ID));
    let current = ExposureSettings {
        exposure_stops: 0.4,
        crop_rect: Some(rect(0.1, 0.1, 0.5, 0.5)),
        rotation: 90,
        ..ExposureSettings::default()
    };
    // create: geometry stripped, names unique (reserved names included)
    let a = create_profile(&mut store, "  Portra ", &current).unwrap();
    let pa = store.find(&a).unwrap();
    assert_eq!(pa.name, "Portra");
    assert_eq!(pa.settings.exposure_stops, 0.4);
    assert_eq!((pa.settings.crop_rect, pa.settings.rotation), (None, 0));
    let b = create_profile(&mut store, "portra", &current).unwrap();
    assert_eq!(store.find(&b).unwrap().name, "portra 2");
    let c = create_profile(&mut store, "", &current).unwrap();
    assert_eq!(store.find(&c).unwrap().name, UNTITLED);
    assert_eq!(unique_name(&store, "None", None), "None 2");
    // rename: built-ins refused; own name allowed; collisions suffixed
    assert!(!rename_profile(&mut store, BUILTIN_PROFILE_ID, "X"));
    assert!(rename_profile(&mut store, &a, "Portra"));
    assert_eq!(store.find(&a).unwrap().name, "Portra");
    assert!(rename_profile(&mut store, &c, "Portra"));
    assert_eq!(
        store.find(&c).unwrap().name,
        "Portra 3",
        "\"portra 2\" is taken"
    );
    // update: built-ins refused, user profile takes the new adjustments
    let brighter = ExposureSettings {
        exposure_stops: 1.0,
        ..current.clone()
    };
    assert!(!update_profile(&mut store, NONE_PROFILE_ID, &brighter));
    assert!(update_profile(&mut store, &a, &brighter));
    assert_eq!(store.find(&a).unwrap().settings.exposure_stops, 1.0);
    // the active profile feeds fresh frames / Reset All
    assert!(store.set_active(&a));
    assert_eq!(store.default_settings().exposure_stops, 1.0);
    // persisted through AppPrefs: the active id survives
    let prefs = negui::AppPrefs {
        profiles: store.to_json().unwrap(),
        ..Default::default()
    };
    let mut storage = MemStorage::default();
    eframe::set_value(&mut storage, negui::app::PREFS_KEY, &prefs);
    let back: negui::AppPrefs = eframe::get_value(&storage, negui::app::PREFS_KEY).unwrap();
    let restored = ProfileStore::from_json(&back.profiles);
    assert_eq!(restored.active_id(), a);
    assert_eq!(restored.user_profiles().len(), 3);
    // delete: built-ins refused; deleting the active one falls back to None
    assert!(!delete_profile(&mut store, BUILTIN_PROFILE_ID));
    assert!(delete_profile(&mut store, &a));
    assert_eq!(store.active_id(), NONE_PROFILE_ID);
    assert_eq!(store.default_settings(), ExposureSettings::default());
}

// ───────────────────────────── U2b: selftest crop + export smoke ─────────────────────────────

/// `--selftest --crop --dialogs --export`: crop/cropped/analysis/dialog captures, a JPEG and a
/// TIFF written into the tmp dir and ICC-tagged, no sidecar written. Runs on a shared-device
/// harness so the zero-copy presenter is the one exercised. Skipped without the sample RAW.
#[test]
fn selftest_crop_and_export_smoke() {
    let sample = std::path::Path::new("/home/deck/Pictures/IMG_0420.CR3");
    if !sample.is_file() {
        eprintln!("skip: sample RAW {} not present", sample.display());
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("negui-tools-selftest");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(sample, dir.join("IMG_0420.CR3")).unwrap();
    let exports = dir.join("exports");
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::PRIMARY;
    setup.device_descriptor = std::sync::Arc::new(|adapter: &eframe::wgpu::Adapter| {
        let have = adapter.limits();
        eframe::wgpu::DeviceDescriptor {
            label: Some("negui-test"),
            required_limits: eframe::wgpu::Limits {
                max_buffer_size: have.max_buffer_size,
                max_storage_buffer_binding_size: have.max_storage_buffer_binding_size,
                max_compute_workgroups_per_dimension: have.max_compute_workgroups_per_dimension,
                ..Default::default()
            },
            ..Default::default()
        }
    });
    let out = dir.join("tools.png");
    let extra = negui::selftest::SelftestOptions {
        crop: true,
        dialogs: true,
        export_dir: Some(exports.clone()),
        latency: false,
        wgpu_setup: Some(eframe::egui_wgpu::WgpuSetup::CreateNew(setup)),
    };
    negui::selftest::run_selftest_with(
        &out,
        dir.clone(),
        std::time::Duration::from_secs(180),
        extra,
    )
    .unwrap();
    for suffix in ["_crop", "_cropped", "_analysis", "_export", "_profiles"] {
        let p = dir.join(format!("tools{suffix}.png"));
        assert!(p.is_file(), "{} missing", p.display());
    }
    for (name, format) in [
        ("IMG_0420.jpg", negsession::ExportFormat::Jpeg),
        ("IMG_0420.tiff", negsession::ExportFormat::Tiff16),
    ] {
        let p = exports.join(name);
        let icc = negui::tools::selftest::icc_profile_len(&p, format).unwrap();
        assert!(icc > 0, "{name} not ICC-tagged");
        // resize on by default: long edge 3000
        let (w, h) = image::image_dimensions(&p).unwrap();
        assert_eq!(w.max(h), 3000, "{name}: {w}×{h}");
    }
    assert!(!dir.join("IMG_0420.swiftinvert.json").exists());
}

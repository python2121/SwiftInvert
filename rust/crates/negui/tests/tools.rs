//! Tool tests (wave 4 / U2): the pure crop-commit math — rect → box → rect round-trips, the
//! constrain rule at ±45°, near-full ⇒ no crop, `commit_fine_rotation` against
//! `negkit::crop_geometry::remap_crop`, the analysis-region commit rule. The interactive
//! overlay, the export/profile dialogs and the selftest crop/export captures are not yet built
//! (see `negui::tools` module docs).

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

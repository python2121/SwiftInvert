//! Zone placement through the session (`ZonePlacementTests`' session-facing half): pins
//! sampled from the warm tower, the forward model's zone for them, and solves that land the
//! pins on their targets within the ⅓-zone tolerance (or report the clamp honestly).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::sync::Arc;

use negkit::zone_placement::ON_TARGET_TOL;
use negsession::{CpuRenderer, RenderRequest, Session};

fn session() -> Session {
    let native = common::synthetic_negative();
    Session::synthetic("zones.CR3", native, None, Arc::new(CpuRenderer))
}

#[test]
fn one_pin_solves_brightness_for_the_target_zone() {
    let mut s = session();
    let settings = s.settings().clone();
    let pin = s.zone_pin(0.5, 0.5, 5.0).unwrap();
    assert_eq!((pin.nx, pin.ny, pin.target_zone), (0.5, 0.5, 5.0));
    assert!(!pin.retargeted);
    assert_eq!(pin.mask_val, 0.0);
    let before = s.predicted_zone(&settings, pin.val_luma, 0.0).unwrap();
    assert!((0.0..=10.0).contains(&before));

    let solution = s.solve_zone_placement(&settings, &[pin]).unwrap().unwrap();
    assert!(
        !solution.settings.auto_exposure,
        "solved against autos-off settings"
    );
    assert_eq!(solution.achieved.len(), 1);
    assert!(solution.knee_label.is_none());
    if !solution.clamped {
        assert!(
            (solution.achieved[0] - 5.0).abs() <= ON_TARGET_TOL,
            "achieved {}",
            solution.achieved[0]
        );
    }
    // The achieved zone IS the forward model at the rounded settings.
    let predicted = s
        .predicted_zone(&solution.settings, pin.val_luma, pin.mask_val)
        .unwrap();
    assert!((predicted - solution.achieved[0]).abs() < 1e-9);
    // Only density moved (rounded to the slider's 0.01).
    assert_ne!(solution.settings.density, settings.density);
    assert!((solution.settings.density * 100.0).fract().abs() < 1e-9);
    assert_eq!(solution.settings.grade, settings.grade);
    // The solve ran on the warm tower: no extra analysis.
    assert_eq!(s.stats().prepares, 1);
}

#[test]
fn two_pins_solve_brightness_and_grade() {
    let mut s = session();
    s.render(RenderRequest::default()).unwrap();
    let settings = s.settings().clone();
    // Find two spots with distinct tones (the synthetic grid ramps across the frame).
    let a = s.sample_zone_pin(0.1, 0.5, 2).unwrap();
    let b = s.sample_zone_pin(0.9, 0.5, 2).unwrap();
    assert!((a.val_luma - b.val_luma).abs() > 1e-3, "distinct tones");
    let (dark, light) = if a.val_luma > b.val_luma {
        (a.pin(0.1, 0.5, 3.0), b.pin(0.9, 0.5, 7.0))
    } else {
        (b.pin(0.9, 0.5, 3.0), a.pin(0.1, 0.5, 7.0))
    };
    let solution = s
        .solve_zone_placement(&settings, &[light, dark])
        .unwrap()
        .unwrap();
    assert_eq!(solution.achieved.len(), 2);
    assert!(!solution.settings.auto_exposure);
    assert!(!solution.settings.auto_normalize_contrast);
    for (pin, got) in [light, dark].iter().zip(&solution.achieved) {
        let predicted = s
            .predicted_zone(&solution.settings, pin.val_luma, pin.mask_val)
            .unwrap();
        assert!((predicted - got).abs() < 1e-9);
        if !solution.clamped {
            assert!(
                (got - pin.target_zone).abs() <= ON_TARGET_TOL,
                "pin → {got} (target {})",
                pin.target_zone
            );
        }
    }
    assert!(
        (solution.settings.grade.fract()).abs() < 1e-9,
        "grade rounds to 1 R"
    );
    assert_eq!(s.stats().prepares, 1);
    // No pins / too many → None, never an error.
    assert!(s.solve_zone_placement(&settings, &[]).unwrap().is_none());
    assert!(
        s.solve_zone_placement(&settings, &[light; 4])
            .unwrap()
            .is_none()
    );
}

/// With the mask on, the pin freezes the plane sample and the model adds it as a constant:
/// the prediction with the sample differs from the one without where the plane isn't zero.
#[test]
fn pins_carry_the_mask_sample_into_the_model() {
    let mut s = session();
    s.update_settings(|st| st.contrast_mask = 0.5);
    let settings = s.settings().clone();
    let pins: Vec<_> = [(0.1, 0.1), (0.5, 0.5), (0.9, 0.9)]
        .into_iter()
        .map(|(u, v)| s.zone_pin(u, v, 5.0).unwrap())
        .collect();
    let pin = pins
        .iter()
        .max_by(|a, b| a.mask_val.abs().total_cmp(&b.mask_val.abs()))
        .unwrap();
    assert_ne!(pin.mask_val, 0.0);
    let with = s
        .predicted_zone(&settings, pin.val_luma, pin.mask_val)
        .unwrap();
    let without = s.predicted_zone(&settings, pin.val_luma, 0.0).unwrap();
    assert_ne!(with, without);
}

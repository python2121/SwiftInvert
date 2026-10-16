//! `DensitometerStateTests` (Tests/SwiftInvertTests/DensitometerStateTests.swift): coordinate
//! mapping, range clamping, the adopt/clear lifecycle — everything beneath the hover gesture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

use negsession::densitometer::{
    DensitometerState, NegativeCharacter, Reading, encoded_of_zone, zone_of_encoded, zone_roman,
};

/// 2×2 quadrants: red, green / blue, white — the rgba8 layout the render output takes.
fn quadrants() -> (Vec<u8>, usize, usize) {
    (
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ],
        2,
        2,
    )
}

fn rgb(state: &DensitometerState) -> Option<[f64; 3]> {
    state.reading().map(|r| r.rgb)
}

#[test]
fn probe_maps_quadrants_correctly() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(0.1, 0.1);
    assert_eq!(rgb(&state), Some([1.0, 0.0, 0.0]));
    state.probe(0.9, 0.1);
    assert_eq!(rgb(&state), Some([0.0, 1.0, 0.0]));
    state.probe(0.1, 0.9);
    assert_eq!(rgb(&state), Some([0.0, 0.0, 1.0]));
    state.probe(0.9, 0.9);
    assert_eq!(rgb(&state), Some([1.0, 1.0, 1.0]));
}

/// u = v = 1.0 is a valid hover position (the far edge) and must clamp to the last pixel.
#[test]
fn far_edge_clamps_to_last_pixel() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(1.0, 1.0);
    assert_eq!(rgb(&state), Some([1.0, 1.0, 1.0]));
    state.probe(0.0, 0.0);
    assert_eq!(rgb(&state), Some([1.0, 0.0, 0.0]));
}

#[test]
fn out_of_range_probe_clears_the_reading() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(0.5, 0.5);
    assert!(state.reading().is_some());
    state.probe(1.2, 0.5);
    assert!(state.reading().is_none());
    state.probe(0.5, -0.1);
    assert!(state.reading().is_none());
}

#[test]
fn reading_matches_densitometry_on_the_same_triplet() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(0.9, 0.9);
    let reading = state.reading().unwrap();
    assert_eq!(reading, Reading::from_encoded([1.0, 1.0, 1.0]));
    // Paper white: density ~0, zone X.
    assert!(reading.print_density < 0.01);
    assert!(reading.zone > 9.99);
    // Black: the density cap, zone 0.
    let black = Reading::from_encoded([0.0, 0.0, 0.0]);
    assert_eq!(black.print_density, 4.0);
    assert_eq!(black.zone, 0.0);
    // 18 % gray encoded sits at zone V; the ruler inverts exactly.
    let mid = negsession::densitometer::mid_gray_encoded();
    assert!((zone_of_encoded(mid) - 5.0).abs() < 1e-12);
    for z in [0.0, 1.5, 5.0, 7.25, 10.0] {
        assert!((zone_of_encoded(encoded_of_zone(z)) - z).abs() < 1e-9);
    }
    assert_eq!(zone_roman(5.0), "V");
    assert_eq!(zone_roman(5.34), "V⅓");
    assert_eq!(zone_roman(9.9), "X");
    assert_eq!(zone_roman(0.0), "0");
}

#[test]
fn adopt_none_and_clear_both_drop_the_reading() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(0.5, 0.5);
    assert!(state.reading().is_some());
    state.clear();
    assert!(state.reading().is_none());

    state.probe(0.5, 0.5);
    assert!(state.reading().is_some());
    state.adopt(None); // image switched away
    assert!(state.reading().is_none());
    assert!(!state.has_bitmap());
    state.probe(0.5, 0.5);
    assert!(state.reading().is_none(), "no bitmap → no reading");
}

/// A new render replaces the bytes the probe reads; a malformed buffer is treated as none.
#[test]
fn adopt_swaps_the_bitmap() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()));
    state.probe(0.1, 0.1);
    assert_eq!(rgb(&state), Some([1.0, 0.0, 0.0]));
    state.adopt(Some((vec![128, 128, 128, 255], 1, 1)));
    assert!(state.reading().is_none(), "stale reading dropped on adopt");
    state.probe(0.1, 0.1);
    assert_eq!(rgb(&state), Some([128.0 / 255.0; 3]));
    state.adopt(Some((vec![0; 7], 2, 1)));
    assert!(!state.has_bitmap());
}

#[test]
fn negative_character_thresholds() {
    let normal = negkit::curve_logic::default_grade_range();
    assert_eq!(
        NegativeCharacter::of_density_range(normal),
        Some(NegativeCharacter::Normal)
    );
    assert_eq!(
        NegativeCharacter::of_density_range(normal * 0.5),
        Some(NegativeCharacter::Flat)
    );
    assert_eq!(
        NegativeCharacter::of_density_range(normal * 1.5),
        Some(NegativeCharacter::Contrasty)
    );
    assert_eq!(NegativeCharacter::of_density_range(0.0), None);
    assert_eq!(NegativeCharacter::of_density_range(f64::NAN), None);
    assert_eq!(NegativeCharacter::Normal.label(), "normal");
}

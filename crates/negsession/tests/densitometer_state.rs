//! `DensitometerStateTests` (Tests/SwiftInvertTests/DensitometerStateTests.swift): coordinate
//! mapping, range clamping, the adopt/clear lifecycle — everything beneath the hover gesture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

use negsession::densitometer::{
    DensitometerState, Encoding, NegativeCharacter, character, encoded_of_zone, read,
    zone_of_encoded, zone_roman,
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
    state.adopt(Some(quadrants()), Encoding::Working);
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
    state.adopt(Some(quadrants()), Encoding::Working);
    state.probe(1.0, 1.0);
    assert_eq!(rgb(&state), Some([1.0, 1.0, 1.0]));
    state.probe(0.0, 0.0);
    assert_eq!(rgb(&state), Some([1.0, 0.0, 0.0]));
}

#[test]
fn out_of_range_probe_clears_the_reading() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()), Encoding::Working);
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
    state.adopt(Some(quadrants()), Encoding::Working);
    state.probe(0.9, 0.9);
    let reading = state.reading().unwrap();
    assert_eq!(reading, read([1.0, 1.0, 1.0]));
    // Paper white: density ~0, zone X.
    assert!(reading.print_density < 0.01);
    assert!(reading.zone > 9.99);
    // Black: the density cap, zone 0.
    let black = read([0.0, 0.0, 0.0]);
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
    state.adopt(Some(quadrants()), Encoding::Working);
    state.probe(0.5, 0.5);
    assert!(state.reading().is_some());
    state.clear();
    assert!(state.reading().is_none());

    state.probe(0.5, 0.5);
    assert!(state.reading().is_some());
    state.adopt(None, Encoding::Working); // image switched away
    assert!(state.reading().is_none());
    assert!(!state.has_bitmap());
    state.probe(0.5, 0.5);
    assert!(state.reading().is_none(), "no bitmap → no reading");
}

/// A new render replaces the bytes the probe reads; a malformed buffer is treated as none.
#[test]
fn adopt_swaps_the_bitmap() {
    let mut state = DensitometerState::new();
    state.adopt(Some(quadrants()), Encoding::Working);
    state.probe(0.1, 0.1);
    assert_eq!(rgb(&state), Some([1.0, 0.0, 0.0]));
    state.adopt(Some((vec![128, 128, 128, 255], 1, 1)), Encoding::Working);
    assert!(state.reading().is_none(), "stale reading dropped on adopt");
    state.probe(0.1, 0.1);
    assert_eq!(rgb(&state), Some([128.0 / 255.0; 3]));
    state.adopt(Some((vec![0; 7], 2, 1)), Encoding::Working);
    assert!(!state.has_bitmap());
}

#[test]
fn negative_character_thresholds() {
    let normal = negkit::curve_logic::default_grade_range();
    assert_eq!(character(normal), Some(NegativeCharacter::Normal));
    assert_eq!(character(normal * 0.5), Some(NegativeCharacter::Flat));
    assert_eq!(character(normal * 1.5), Some(NegativeCharacter::Contrasty));
    assert_eq!(character(0.0), None);
    assert_eq!(character(f64::NAN), None);
    assert_eq!(NegativeCharacter::Normal.label(), "normal");
}

/// The zone overlay reads the SAME adopted bytes as the probe (ZoneGrid.compute): on a flat
/// paper-white bitmap every cell is zone X with one region label.
#[test]
fn zone_grid_comes_from_the_adopted_bytes() {
    let mut state = DensitometerState::new();
    assert!(state.zone_grid().is_none(), "no bitmap → no grid");
    let (w, h) = (48usize, 24usize);
    state.adopt(Some((vec![255u8; w * h * 4], w, h)), Encoding::Working);
    let grid = state.zone_grid().unwrap();
    assert_eq!(grid.cols, 24);
    assert_eq!(grid.rows, 12);
    assert!(grid.zones.iter().all(|&z| z == 10));
    assert_eq!(grid.labels.len(), 1);
    assert_eq!(grid.labels[0].zone, 10);
    // The free function takes tightly packed rows and agrees with the state.
    assert_eq!(
        negsession::densitometer::zone_grid(&vec![255u8; w * h * 4], w, h, Encoding::Working),
        Some(grid)
    );
}

// ───────────────────────────── sRGB display bytes (review 7, finding 8) ─────────────────────────────

use negkit::reference::{encoded_from_srgb, encoded_from_srgb_f64, srgb_from_encoded};

/// `encoded_from_srgb` inverts the sRGB leg: the f64 core against an f64 evaluation of the
/// forward is exact to ≤ 1e-6 (the math); the f32 pair (the kernels' forward, the f32
/// interface) to ≤ 1e-5 — the residual is f32 rounding of a dark channel beside a bright one
/// (cancellation in the gamut matrix leaves ≈1e-7 absolute in a tiny linear value), amplified
/// ×16 by the inverse TRC's slope at e = 0.05. In-gamut colours only: the forward clamp admits
/// nothing else back.
#[test]
fn srgb_leg_round_trips_within_1e6() {
    use negkit::constants::WorkingOetf;
    use negkit::lab::TO_XYZ;
    use negkit::reference::SRGB_FROM_XYZ;
    // The forward leg in f64: decode the TRC, gamut matrix, piecewise OETF.
    let forward_f64 = |e: [f64; 3]| -> [f64; 3] {
        let lin: Vec<f64> = e.iter().map(|&v| v.powf(WorkingOetf::GAMMA)).collect();
        let xyz: Vec<f64> = TO_XYZ
            .iter()
            .map(|r| r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2])
            .collect();
        let mut out = [0f64; 3];
        for (o, r) in out.iter_mut().zip(&SRGB_FROM_XYZ) {
            let v = (r[0] * xyz[0] + r[1] * xyz[1] + r[2] * xyz[2]).clamp(0.0, 1.0);
            *o = if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
        }
        out
    };
    let mut worst_vs_f64 = 0f64;
    let mut worst_vs_kernel = 0f32;
    let steps: Vec<f32> = (1..=19).map(|i| i as f32 / 20.0).collect();
    for &r in &steps {
        for &g in &steps {
            for &b in &steps {
                let e = [r, g, b];
                let s = srgb_from_encoded(e);
                // Only colours the forward leg did not clip can come back (a clipped channel
                // lands at the OETF of exactly 0 or 1 — 0.99999994 in f32 for the latter).
                if s.iter().any(|&v| v <= 1e-6 || v >= 0.9999) {
                    continue;
                }
                let back_kernel = encoded_from_srgb(s);
                let e64 = [f64::from(r), f64::from(g), f64::from(b)];
                let back_f64 = encoded_from_srgb_f64(forward_f64(e64));
                for c in 0..3 {
                    worst_vs_kernel = worst_vs_kernel.max((back_kernel[c] - e[c]).abs());
                    worst_vs_f64 = worst_vs_f64.max((back_f64[c] - e64[c]).abs());
                }
            }
        }
    }
    assert!(
        worst_vs_f64 <= 1e-6,
        "inverse vs f64 forward: {worst_vs_f64}"
    );
    assert!(
        worst_vs_kernel <= 1e-5,
        "inverse vs the kernels' f32 forward: {worst_vs_kernel}"
    );
    // Black and white are fixed points.
    assert_eq!(encoded_from_srgb([0.0; 3]), [0.0; 3]);
    let w = encoded_from_srgb([1.0; 3]);
    assert!(w.iter().all(|&v| (v - 1.0).abs() < 2e-6), "{w:?}");
}

/// The probe over sRGB-rendered bytes reads the same density/zone as over working bytes of
/// the same picture — quantizing both to 8 bits moves the zone by far less than a third.
#[test]
fn srgb_bytes_read_the_same_zones_as_working_bytes() {
    let (w, h) = (64usize, 4usize);
    let mut working = Vec::with_capacity(w * h * 4);
    let mut srgb = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let e = [
                x as f32 / (w - 1) as f32,
                (x as f32 / (w - 1) as f32) * 0.9 + 0.05,
                0.3 + 0.1 * y as f32,
            ];
            let s = srgb_from_encoded(e);
            working.extend(e.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            working.push(255);
            srgb.extend(s.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            srgb.push(255);
        }
    }
    let mut a = DensitometerState::new();
    a.adopt(Some((working.clone(), w, h)), Encoding::Working);
    let mut b = DensitometerState::new();
    b.adopt(Some((srgb.clone(), w, h)), Encoding::Srgb);
    assert_eq!(b.encoding(), Encoding::Srgb);
    let mut worst_zone = 0f64;
    let mut worst_density = 0f64;
    for x in 1..w - 1 {
        let u = (x as f64 + 0.5) / w as f64;
        a.probe(u, 0.6);
        b.probe(u, 0.6);
        let (ra, rb) = (a.reading().unwrap(), b.reading().unwrap());
        worst_zone = worst_zone.max((ra.zone - rb.zone).abs());
        worst_density = worst_density.max((ra.print_density - rb.print_density).abs());
    }
    // One 8-bit step of the working encode is ~0.02 zone at the mid; a third of a zone is the
    // strip's resolution. The density cap region (near black) is excluded by starting at x = 1.
    assert!(worst_zone < 1.0 / 3.0, "zone drift {worst_zone}");
    assert!(worst_density < 0.1, "density drift {worst_density}");
    // The overlay transcodes the same way: identical cell zones on this gradient.
    let ga = negsession::densitometer::zone_grid(&working, w, h, Encoding::Working).unwrap();
    let gb = negsession::densitometer::zone_grid(&srgb, w, h, Encoding::Srgb).unwrap();
    assert_eq!(ga.cols, gb.cols);
    let differing = ga
        .zones
        .iter()
        .zip(&gb.zones)
        .filter(|(x, y)| x != y)
        .count();
    assert!(
        differing * 10 <= ga.zones.len(),
        "{differing} of {} cells differ",
        ga.zones.len()
    );
    // Not vacuous: the two byte buffers really differ (the sRGB leg is not the identity — the
    // TRCs nearly coincide at midtones, the gamut matrix does the rest), and the transcoded
    // overlay input is the working buffer back to within a byte.
    assert_ne!(srgb, working);
    let mut c = DensitometerState::new();
    c.adopt(Some((srgb, w, h)), Encoding::Working);
    assert_eq!(c.encoding(), Encoding::Working);
}

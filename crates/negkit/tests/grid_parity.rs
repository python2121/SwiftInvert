//! Port of `GridParityTests.swift` (the generator + prefiltered-grid parity) and the
//! `prefilteredGrid` case of `AnalysisParityTests.swift` (synthetic64).
//!
//! `GridParityTests.boundsMetersAndParams` (bounds / meters / curve params on the regenerated
//! grid, slope/pivot tolerance 2.5e-4) needs `ExposureKernel.analyze` + `deriveRenderParams` and
//! belongs to `tests/analysis.rs` (agent B) — copy `synthetic_grid_input` from here.
//!
//! synthetic_grid fixture: a 1600×1066 negative-like image regenerated here bit-exactly from the
//! integer-hash formula in `scripts/dump_fixtures.py`. Exercises the b=2 block-median path and
//! the full neutral-axis quadratic fit.

use negkit::RgbImage;
use negkit::fixtures;
use negkit::prefilter;

/// The shared bit-exact generator (`negkit::fixtures::synthetic_grid_input`).
fn synthetic_grid_input() -> RgbImage {
    fixtures::synthetic_grid_input().expect("generator")
}

fn px(img: &RgbImage, y: usize, x: usize, c: usize) -> f32 {
    img.pixel(x, y).expect("in range")[c]
}

#[test]
fn generator_matches_probes() {
    let manifest = fixtures::read_json("synthetic_grid/manifest.json").expect("manifest");
    let probes = manifest["input_probe"].as_object().expect("input_probe");
    let input = synthetic_grid_input();
    for (key, expected) in probes {
        // Keys are "y,x" (the Swift subscript is `[y, x, c]`).
        let parts: Vec<usize> = key.split(',').map(|s| s.parse().expect("int")).collect();
        let expected = expected.as_array().expect("triple");
        for (c, want) in expected.iter().enumerate().take(3) {
            let got = f64::from(px(&input, parts[0], parts[1], c));
            let want = want.as_f64().expect("f64");
            assert!(
                (got - want).abs() <= 1e-7,
                "probe {key} ch{c}: got {got}, expected {want} ± 1e-7"
            );
        }
    }
}

#[test]
fn prefiltered_grid() {
    let expected = fixtures::read_f32("synthetic_grid/prefiltered.bin").expect("fixture");
    let got = prefilter::prefilter_log_grid(&synthetic_grid_input(), 0.05);
    assert_eq!(got.data.len(), expected.len(), "grid element count");
    assert_eq!((got.width, got.height), (720, 480));
    let (max_diff, _) = fixtures::max_mean_abs_diff(&got.data, &expected);
    eprintln!("prefiltered grid max diff {max_diff}");
    assert!(max_diff < 1e-5, "prefiltered grid max diff {max_diff}");
}

/// `AnalysisParityTests.prefilteredGrid`: synthetic64 (64×64, buffer 0.05 → 58×58, b = 1 so the
/// grid is the logged crop itself).
#[test]
fn synthetic64_prefiltered_grid() {
    let input = fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("fixture");
    let expected = fixtures::read_f32("synthetic64/prefiltered.bin").expect("fixture");
    let got = prefilter::prefilter_log_grid(&input, 0.05);
    assert_eq!(got.data.len(), expected.len(), "grid size (58x58x3)");
    assert_eq!((got.width, got.height), (58, 58));
    let (max_diff, _) = fixtures::max_mean_abs_diff(&got.data, &expected);
    eprintln!("prefiltered grid max diff {max_diff}");
    assert!(max_diff < 1e-5, "prefiltered grid max diff {max_diff}");
}

/// The b > 2 scratch-median path has no fixture (the preview always lands on b = 2), so pin it
/// against the b = 2 closed form on an image where both apply: a 2048-wide frame gets b = 2;
/// tiling the same content so the long edge crosses 2048 gets b = 3, and each 3×3 cell's
/// median must equal the comparison-sorted median of its block.
#[test]
fn block_median_general_path_matches_a_sorted_median() {
    let (w, h) = (2100usize, 9usize); // b = ceil(2100 / 1024) = 3
    let img = RgbImage::new_with(w, h, |buf| {
        for (i, v) in buf.iter_mut().enumerate() {
            *v = (fixtures::hash32(i as u32) as f32 / 4294967296.0f32).clamp(1e-4, 1.0);
        }
    })
    .expect("non-empty");
    let grid = prefilter::block_median_grid(img.clone());
    assert_eq!((grid.width, grid.height), (700, 3));
    for gy in 0..grid.height {
        for gx in 0..grid.width {
            for c in 0..3 {
                let mut block: Vec<f32> = (0..3)
                    .flat_map(|by| (0..3).map(move |bx| (by, bx)))
                    .map(|(by, bx)| px(&img, gy * 3 + by, gx * 3 + bx, c))
                    .collect();
                block.sort_by(f32::total_cmp);
                assert_eq!(px(&grid, gy, gx, c), block[4], "cell ({gx},{gy}) ch{c}");
            }
        }
    }
}

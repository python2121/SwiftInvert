//! Ports of `DensitometryTests.swift`, `ZoneGridTests.swift`, `AuditZoneGridStrideTests.swift`,
//! `TestStripTests.swift` (+ `TestStripRotationTests`) and `ZonePlacementTests.swift` — the
//! darkroom read-outs and solvers. None of these are parity surfaces; the oracles are
//! closed-form (NegPy `densitometer.py` / `stats.py` / `placement.py` semantics).
//!
//! `characterReadsTheSameRangeTheCurveDoes` used `Synthetic64.analysis`
//! (`ExposureKernel.analyze`); here it reads NegPy's own measurement from
//! `synthetic64/default/analysis.json`, as `tests/chain.rs` does.

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use negkit::constants::{K, WorkingOetf};
use negkit::curve_logic;
use negkit::densitometry::{self, NegativeCharacter};
use negkit::kernel::ExposureAnalysis;
use negkit::test_strip::{self, RowCol};
use negkit::zone_grid::{self, Grid};
use negkit::zone_placement::{self, KNEE_CANDIDATES, ON_TARGET_TOL, Pin};
use negkit::{ExposureSettings, LogNegativeBounds, NormalizedRect, V3, fixtures};
use serde_json::Value;
use std::collections::HashSet;

fn v3(v: &Value) -> V3 {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

/// NegPy's own analysis of synthetic64, from `default/analysis.json`.
fn synthetic64_analysis() -> ExposureAnalysis {
    let j = fixtures::read_json("synthetic64/default/analysis.json").expect("analysis.json");
    let b = &j["bounds"];
    let m = &j["meters"];
    let na = m["neutral_axis_refs"]
        .as_array()
        .expect("neutral_axis_refs");
    let (neutral_mid, neutral_shadow, neutral_highlight, neutral_confidence) = if na[0].is_null() {
        (None, None, None, None)
    } else {
        (
            Some(v3(&na[0])),
            Some(v3(&na[1])),
            (!na[2].is_null()).then(|| v3(&na[2])),
            na[3].as_f64(),
        )
    };
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: v3(&b["base_floors"]),
            ceils: v3(&b["base_ceils"]),
        },
        anchor: m["metered_anchor"].as_f64().unwrap(),
        textural_range: m["textural_range"].as_f64().unwrap(),
        shadow_point: m["shadow_point"].as_f64(),
        highlight_point: m["highlight_point"].as_f64(),
        shadow_refs: v3(&m["shadow_log_refs"]),
        neutral_mid,
        neutral_shadow,
        neutral_highlight,
        neutral_confidence,
    }
}

// ═══════════════════════════════ DensitometryTests ═══════════════════════════════

// MARK: - Zone ruler

#[test]
fn zone_v_is_mid_gray() {
    // The ruler's defining property: 18% reflectance prints as Zone V.
    let enc = f64::from(WorkingOetf::encode(
        densitometry::ZONE_MID_REFLECTANCE as f32,
    ));
    assert!((densitometry::zone_of_encoded(enc) - 5.0).abs() < 1e-9);
}

#[test]
fn zone_endpoints() {
    assert!((densitometry::zone_of_encoded(0.0) - 0.0).abs() < 1e-9);
    assert!((densitometry::zone_of_encoded(1.0) - 10.0).abs() < 1e-9);
}

#[test]
fn zone_is_monotonic_and_clamped() {
    let mut previous = -1.0;
    for i in 0..=200 {
        let z = densitometry::zone_of_encoded(i as f64 / 200.0);
        assert!(z >= previous);
        assert!((0.0..=10.0).contains(&z));
        previous = z;
    }
    // Out-of-range input clamps rather than extrapolating.
    assert_eq!(densitometry::zone_of_encoded(-0.5), 0.0);
    assert_eq!(densitometry::zone_of_encoded(1.5), 10.0);
}

#[test]
fn zone_piecewise_linear_halves() {
    // Below the hinge the ruler is linear from 0 to V, above it from V to X (NegPy
    // zone_of_encoded) — check a midpoint of each leg.
    let mid = densitometry::mid_gray_encoded();
    assert!((densitometry::zone_of_encoded(mid / 2.0) - 2.5).abs() < 1e-9);
    assert!((densitometry::zone_of_encoded((mid + 1.0) / 2.0) - 7.5).abs() < 1e-9);
}

// MARK: - Print density

#[test]
fn print_density_of_paper_white_is_zero() {
    assert!(densitometry::print_density_of_encoded(1.0).abs() < 1e-9);
}

#[test]
fn print_density_clamps_at_paper_black() {
    assert_eq!(
        densitometry::print_density_of_encoded(0.0),
        densitometry::PRINT_DENSITY_MAX
    );
}

#[test]
fn print_density_matches_negative_log10_of_decoded_tone() {
    // D = −log10(OETF⁻¹(enc)) over the range where the clamp is inactive.
    for enc in [0.2, 0.4, 0.6, 0.8, 1.0] {
        let t = f64::from(WorkingOetf::decode(enc as f32));
        assert!((densitometry::print_density_of_encoded(enc) - (-t.log10())).abs() < 1e-6);
    }
}

#[test]
fn print_density_decreases_with_lightness() {
    let mut previous = f64::INFINITY;
    for i in 1..=100 {
        let d = densitometry::print_density_of_encoded(i as f64 / 100.0);
        assert!(d <= previous);
        previous = d;
    }
}

#[test]
fn mid_gray_prints_near_density_point_seven_four() {
    // 18% reflectance ⇒ D = −log10(0.18) ≈ 0.745, the paper's mid-tone.
    let enc = densitometry::mid_gray_encoded();
    assert!((densitometry::print_density_of_encoded(enc) - 0.7447).abs() < 1e-3);
}

// MARK: - Reading

#[test]
fn read_takes_luma_on_encoded_values_before_decoding() {
    // NegPy computes luma on the ENCODED triplet, then decodes; decoding per channel first
    // would give a different (wrong) density.
    let rgb = [0.2, 0.6, 0.9];
    let reading = densitometry::read(rgb);
    let lum = K::LUMA_R * 0.2 + K::LUMA_G * 0.6 + K::LUMA_B * 0.9;
    assert!((reading.print_density - densitometry::print_density_of_encoded(lum)).abs() < 1e-9);
    assert!((reading.zone - densitometry::zone_of_encoded(lum)).abs() < 1e-9);
    assert_eq!(reading.rgb, rgb);
}

#[test]
fn neutral_gray_reading_is_self_consistent() {
    let enc = densitometry::mid_gray_encoded();
    let reading = densitometry::read([enc, enc, enc]);
    assert!((reading.zone - 5.0).abs() < 1e-9);
}

// MARK: - Zone roman numerals

#[test]
fn zone_roman_formats_thirds() {
    assert_eq!(densitometry::zone_roman(0.0), "0");
    assert_eq!(densitometry::zone_roman(3.0), "III");
    assert_eq!(densitometry::zone_roman(4.33), "IV⅓"); // NegPy's own example
    assert_eq!(densitometry::zone_roman(4.67), "IV⅔");
    assert_eq!(densitometry::zone_roman(5.0), "V");
    assert_eq!(densitometry::zone_roman(10.0), "X");
}

#[test]
fn zone_roman_clamps_and_never_overflows() {
    assert_eq!(densitometry::zone_roman(-3.0), "0");
    assert_eq!(densitometry::zone_roman(99.0), "X");
    // 9.9 rounds to 30 thirds = base 10: must yield "X", not index past it.
    assert_eq!(densitometry::zone_roman(9.9), "X");
    for i in 0..=1000 {
        let _ = densitometry::zone_roman(i as f64 / 100.0); // no trap
    }
    let _ = densitometry::zone_roman(f64::NAN);
}

// MARK: - Negative character

#[test]
fn character_thresholds() {
    let normal = curve_logic::default_grade_range();
    assert_eq!(
        densitometry::character(normal),
        Some(NegativeCharacter::Normal)
    );
    // Ratios sit either side of NegPy's 0.80 / 1.25 gates.
    assert_eq!(
        densitometry::character(normal * 0.79),
        Some(NegativeCharacter::Flat)
    );
    assert_eq!(
        densitometry::character(normal * 0.81),
        Some(NegativeCharacter::Normal)
    );
    assert_eq!(
        densitometry::character(normal * 1.24),
        Some(NegativeCharacter::Normal)
    );
    assert_eq!(
        densitometry::character(normal * 1.26),
        Some(NegativeCharacter::Contrasty)
    );
}

#[test]
fn character_boundaries_are_inclusive_of_normal() {
    // NegPy: `< 0.80` is flat and `> 1.25` is contrasty, so both exact ratios are normal.
    let normal = curve_logic::default_grade_range();
    assert_eq!(
        densitometry::character(normal * 0.80),
        Some(NegativeCharacter::Normal)
    );
    assert_eq!(
        densitometry::character(normal * 1.25),
        Some(NegativeCharacter::Normal)
    );
}

#[test]
fn character_is_none_without_a_measurement() {
    assert_eq!(densitometry::character(0.0), None);
    assert_eq!(densitometry::character(-1.0), None);
    assert_eq!(densitometry::character(f64::NAN), None);
    assert_eq!(densitometry::character(f64::INFINITY), None);
}

#[test]
fn character_labels_carry_the_grade_hint() {
    assert_eq!(NegativeCharacter::Flat.label(), "flat (≈N−1)");
    assert_eq!(NegativeCharacter::Normal.label(), "normal");
    assert_eq!(NegativeCharacter::Contrasty.label(), "contrasty (≈N+1)");
    assert_eq!(NegativeCharacter::Flat.raw_value(), "flat");
}

/// The diagnostic must read the same range the curve does — a drift here would have it
/// describe a negative the render isn't printing.
#[test]
fn character_reads_the_same_range_the_curve_does() {
    let analysis = synthetic64_analysis();
    let range = analysis.base_bounds.luminance_density_range();
    assert!(range > 0.0);
    assert!(densitometry::character(range).is_some());
    // Same value derive_render_params hands effective_grade_range as floor_ceil_range.
    let via_curve =
        curve_logic::effective_grade_range(false, Some(range), Some(analysis.textural_range));
    assert_eq!(via_curve, Some(range));
}

// ═══════════════════════════════ ZoneGridTests ═══════════════════════════════

/// rgba8 buffer helper; bytes_per_row can exceed width*4 (CG rows pad).
fn buffer(
    width: usize,
    height: usize,
    pad: usize,
    value: impl Fn(usize, usize) -> u8,
) -> (Vec<u8>, usize) {
    let bpr = width * 4 + pad;
    let mut bytes = vec![0u8; bpr * height];
    for y in 0..height {
        for x in 0..width {
            let v = value(x, y);
            let i = y * bpr + x * 4;
            bytes[i] = v;
            bytes[i + 1] = v;
            bytes[i + 2] = v;
            bytes[i + 3] = 255;
        }
    }
    (bytes, bpr)
}

fn grid(bytes: &[u8], width: usize, height: usize, bpr: usize) -> Option<Grid> {
    zone_grid::compute(bytes, width, height, bpr)
}

/// Every label must sit on a cell of its own zone (the anchor-snapping contract) — checked
/// for every grid the suite builds.
fn expect_labels_owned(g: &Grid) {
    for label in &g.labels {
        assert_eq!(g.zone(label.col, label.row), Some(label.zone));
    }
}

fn mid_gray_u8() -> u8 {
    (densitometry::mid_gray_encoded() * 255.0).round() as u8
}

#[test]
fn uniform_mid_gray_is_one_zone_v_region() {
    let mid = mid_gray_u8();
    let (bytes, bpr) = buffer(240, 160, 0, |_, _| mid);
    let g = grid(&bytes, 240, 160, bpr).expect("grid");
    assert_eq!(g.cols, 24);
    assert_eq!(g.rows, 16);
    assert!(g.zones.iter().all(|&z| z == 5));
    assert_eq!(g.labels.len(), 1);
    assert_eq!(g.labels[0].zone, 5);
    expect_labels_owned(&g);
}

#[test]
fn tall_frame_swaps_grid_axes() {
    let (bytes, bpr) = buffer(160, 240, 0, |_, _| 128);
    let g = grid(&bytes, 160, 240, bpr).expect("grid");
    assert_eq!(g.cols, 16);
    assert_eq!(g.rows, 24);
}

#[test]
fn black_white_split_yields_paper_ends_and_two_regions() {
    let (bytes, bpr) = buffer(240, 160, 12, |x, _| if x < 120 { 0 } else { 255 });
    let g = grid(&bytes, 240, 160, bpr).expect("grid");
    assert_eq!(g.zone(0, 8), Some(0));
    assert_eq!(g.zone(g.cols - 1, 8), Some(10));
    assert_eq!(g.labels.len(), 2);
    let zones: HashSet<u8> = g.labels.iter().map(|l| l.zone).collect();
    assert_eq!(zones, HashSet::from([0, 10]));
    expect_labels_owned(&g);
}

/// A U-shaped region whose centroid falls in the enclosed notch: the label anchor must still
/// land on a cell the region owns.
#[test]
fn concave_region_label_stays_inside() {
    let mid = mid_gray_u8();
    // Bright U: left column, right column and bottom row bright; the notch (top-centre)
    // mid-gray. 240x160 → 24x16 cells.
    let (bytes, bpr) = buffer(240, 160, 0, |x, y| {
        let left_arm = x < 40;
        let right_arm = x >= 200;
        let bottom = y >= 120;
        if left_arm || right_arm || bottom {
            255
        } else {
            mid
        }
    });
    let g = grid(&bytes, 240, 160, bpr).expect("grid");
    expect_labels_owned(&g);
    // Two regions: the bright U and the mid notch.
    assert_eq!(g.labels.len(), 2);
    let bright = g
        .labels
        .iter()
        .find(|l| l.zone == 10)
        .expect("bright label");
    // The U's centroid sits inside the notch (which is zone 5) — the snapped anchor must be
    // a U cell anyway.
    assert_eq!(g.zone(bright.col, bright.row), Some(10));
}

/// The overlay's ruler is the densitometer's ruler — same bytes, same zone (uniform frame:
/// any probe pixel equals the single cell zone).
#[test]
fn ruler_matches_densitometer() {
    for v in [0u8, 40, 96, 128, 200, 255] {
        let (bytes, bpr) = buffer(96, 96, 0, |_, _| v);
        let g = grid(&bytes, 96, 96, bpr).expect("grid");
        let e = f64::from(v) / 255.0;
        let probe = densitometry::read([e, e, e]);
        let want = probe.zone.round() as u8;
        assert!(g.zones.iter().all(|&z| z == want), "value {v}");
    }
}

/// Buffers shorter than their declared geometry are refused, never read past.
#[test]
fn zone_grid_refuses_short_buffers() {
    let (bytes, bpr) = buffer(240, 160, 0, |_, _| 128);
    assert!(zone_grid::compute(&bytes[..bytes.len() - 8], 240, 160, bpr).is_none());
    assert!(zone_grid::compute(&bytes, 240, 160, 100).is_none()); // bpr < width*4
    assert!(zone_grid::compute(&bytes, 1, 160, bpr).is_none());
}

// ═══════════════════════════════ AuditZoneGridStrideTests ═══════════════════════════════

/// rgba8 horizontal gradient (dark left → bright right), padded rows.
fn gradient_buffer(width: usize, height: usize, pad: usize) -> (Vec<u8>, usize) {
    buffer(width, height, pad, |x, _| {
        (x as f64 * 255.0 / (width - 1) as f64).round() as u8
    })
}

/// 800×600: step = min(800,600)/(4·24) = 6 (stride path engaged); sampled dims 134×100 →
/// cols = 24, rows = round(100·24/134) = 18. The gradient is dark→bright left→right, and zone
/// is monotone in encoded luma, so each row's zones must be non-decreasing with the column.
#[test]
fn stride_subsampled_gradient() {
    let (bytes, bpr) = gradient_buffer(800, 600, 16);
    let g = grid(&bytes, 800, 600, bpr).expect("grid");
    assert_eq!(g.cols, 24);
    assert_eq!(g.rows, 18);
    assert!(g.zones.iter().all(|&z| z <= 10));

    for r in 0..g.rows {
        let mut last = 0u8;
        for c in 0..g.cols {
            let z = g.zone(c, r).unwrap();
            assert!(z >= last, "row {r}: zone dropped at col {c}");
            last = z;
        }
        // Direction pin: the gradient's dark edge is the low-zone edge.
        assert!(g.zone(0, r) < g.zone(g.cols - 1, r));
    }
    // Extremes reach the paper ends (mean of the first/last 1/24 of a 0…255 ramp encodes
    // near black / near white).
    assert!(g.zone(0, 0).unwrap() <= 1);
    assert!(g.zone(g.cols - 1, 0).unwrap() >= 9);
}

/// The same CONTENT at 96×72 runs with step = 1 and produces the same 24×18 grid shape; a
/// stride subsample of a smooth gradient must land the corner cells in the same zone as the
/// dense sampling.
#[test]
fn stride_matches_dense_sampling_at_the_corners() {
    let (big_bytes, big_bpr) = gradient_buffer(800, 600, 16);
    let big = grid(&big_bytes, 800, 600, big_bpr).expect("grid");
    let (small_bytes, small_bpr) = gradient_buffer(96, 72, 0);
    let small = grid(&small_bytes, 96, 72, small_bpr).expect("grid");

    assert_eq!((small.cols, small.rows), (big.cols, big.rows));
    for (c, r) in [
        (0, 0),
        (big.cols - 1, 0),
        (0, big.rows - 1),
        (big.cols - 1, big.rows - 1),
    ] {
        assert_eq!(
            big.zone(c, r),
            small.zone(c, r),
            "corner ({c},{r}): stride {:?} vs dense {:?}",
            big.zone(c, r),
            small.zone(c, r)
        );
    }
}

// ═══════════════════════════════ TestStripTests ═══════════════════════════════

#[test]
fn ladders_are_centred_on_defaults() {
    // Centre patch = the stock conversion (density 1.0, grade 115).
    assert_eq!(test_strip::DENSITIES_BY_COLUMN[test_strip::COLS / 2], 1.0);
    assert_eq!(test_strip::GRADES_BY_ROW[test_strip::ROWS / 2], 115.0);
    // Columns brighten left → right (density descending — this app's "right = brighter"
    // convention; deliberate mirror of upstream).
    let mut desc = test_strip::DENSITIES_BY_COLUMN.to_vec();
    desc.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert_eq!(test_strip::DENSITIES_BY_COLUMN.to_vec(), desc);
    // Rows soften top → bottom (grade ascending, NegPy order).
    let mut asc = test_strip::GRADES_BY_ROW.to_vec();
    asc.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(test_strip::GRADES_BY_ROW.to_vec(), asc);
    // The exact NegPy ladder values, mirrored.
    let densities: HashSet<u64> = test_strip::DENSITIES_BY_COLUMN
        .iter()
        .map(|d| d.to_bits())
        .collect();
    let want: HashSet<u64> = [0.4f64, 0.7, 1.0, 1.3, 1.6]
        .iter()
        .map(|d| d.to_bits())
        .collect();
    assert_eq!(densities, want);
    assert_eq!(test_strip::GRADES_BY_ROW, [75.0, 95.0, 115.0, 135.0, 155.0]);
}

#[test]
fn cells_are_row_major_and_complete() {
    let cells = test_strip::cells();
    assert_eq!(cells.len(), 25);
    assert_eq!((cells[0].row, cells[0].col), (0, 0));
    assert_eq!(cells[1].col, 1); // row-major
    assert_eq!((cells[24].row, cells[24].col), (4, 4));
    assert_eq!(cells[7].density, test_strip::DENSITIES_BY_COLUMN[2]);
    assert_eq!(cells[7].grade, test_strip::GRADES_BY_ROW[1]);
    assert!(test_strip::cell(5, 0, 0).is_none());
}

/// Patch rects tile the frame exactly: disjoint, union = frame — for dimensions that don't
/// divide evenly.
#[test]
fn patch_rects_tile_exactly() {
    for (w, h) in [(100usize, 75usize), (1536, 1025), (7, 11)] {
        let mut covered = vec![false; w * h];
        for cell in test_strip::cells() {
            let r = test_strip::patch_rect(w, h, cell.row, cell.col);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    assert!(!covered[y * w + x], "overlap at ({x}, {y})");
                    covered[y * w + x] = true;
                }
            }
        }
        assert!(covered.iter().all(|&c| c), "gaps in tiling {w}x{h}");
    }
}

/// Hit-testing inverts the rect math: the centre of every patch maps back to its own
/// (row, col).
#[test]
fn cell_at_inverts_patch_rect() {
    let (w, h) = (1536usize, 1025usize);
    for cell in test_strip::cells() {
        let r = test_strip::patch_rect(w, h, cell.row, cell.col);
        let nx = ((r.x0 + r.x1) as f64 / 2.0) / w as f64;
        let ny = ((r.y0 + r.y1) as f64 / 2.0) / h as f64;
        let hit = test_strip::cell_at(nx, ny);
        assert_eq!((hit.row, hit.col), (cell.row, cell.col));
    }
    // Edges clamp.
    assert_eq!(
        test_strip::cell_at(1.0, 1.0),
        RowCol {
            row: test_strip::ROWS - 1,
            col: test_strip::COLS - 1
        }
    );
    assert_eq!(test_strip::cell_at(0.0, 0.0), RowCol { row: 0, col: 0 });
    assert_eq!(
        test_strip::cell_at(-1.0, f64::NAN),
        RowCol { row: 0, col: 0 }
    );
}

#[test]
fn nearest_cell_finds_defaults_and_clamps() {
    assert_eq!(
        test_strip::nearest_cell(1.0, 115.0),
        RowCol { row: 2, col: 2 }
    );
    // 0.6→0.7 (col 3), 80→75 (row 0)
    assert_eq!(
        test_strip::nearest_cell(0.6, 80.0),
        RowCol { row: 0, col: 3 }
    );
    // clamps to extremes
    assert_eq!(
        test_strip::nearest_cell(5.0, 300.0),
        RowCol { row: 4, col: 0 }
    );
}

/// Mosaic assembly: tiles filled with distinct values land exactly in their own patch and
/// nowhere else.
#[test]
fn mosaic_patches_come_from_their_own_tiles() {
    let (w, h) = (50usize, 35usize);
    let bpr = w * 4;
    let mut mosaic = vec![0u8; h * bpr];
    for cell in test_strip::cells() {
        let value = (10 + cell.row * test_strip::COLS + cell.col) as u8;
        let tile = vec![value; h * bpr];
        test_strip::copy_patch(&tile, &mut mosaic, w, h, bpr, cell.row, cell.col);
    }
    for cell in test_strip::cells() {
        let want = (10 + cell.row * test_strip::COLS + cell.col) as u8;
        let r = test_strip::patch_rect(w, h, cell.row, cell.col);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                assert_eq!(
                    mosaic[y * bpr + x * 4],
                    want,
                    "patch ({},{}) at ({x},{y})",
                    cell.row,
                    cell.col
                );
            }
        }
    }
    // A short tile leaves the mosaic untouched.
    let before = mosaic.clone();
    test_strip::copy_patch(&[7u8; 16], &mut mosaic, w, h, bpr, 0, 0);
    assert_eq!(mosaic, before);
}

// MARK: TestStripRotationTests (NegPy a2455ab)

fn pair_key(c: &test_strip::Cell) -> (u64, u64) {
    (c.density.to_bits(), c.grade.to_bits())
}

/// Every orientation is a bijection onto the full ladder: all 25 (density, grade) pairs
/// appear exactly once.
#[test]
fn orientation_covers_ladder_exactly_once() {
    for o in 0..4 {
        let cells = test_strip::cells_oriented(o);
        assert_eq!(cells.len(), 25);
        let pairs: HashSet<(u64, u64)> = cells.iter().map(pair_key).collect();
        assert_eq!(pairs.len(), 25, "orientation {o}");
    }
}

#[test]
fn orientation_zero_is_the_baseline() {
    assert_eq!(test_strip::cells_oriented(0), test_strip::cells());
    // And 4 wraps to 0, -1 to 3.
    assert_eq!(test_strip::cells_oriented(4), test_strip::cells_oriented(0));
    assert_eq!(
        test_strip::cells_oriented(-1),
        test_strip::cells_oriented(3)
    );
}

/// One clockwise turn: the top-left corner receives what was at the bottom-left (standard
/// 90° CW rotation of the logical grid).
#[test]
fn clockwise_turn_moves_corners() {
    let base = test_strip::cell(test_strip::ROWS - 1, 0, 0).unwrap();
    let turned = test_strip::cell(0, 0, 1).unwrap();
    assert_eq!(turned.density, base.density);
    assert_eq!(turned.grade, base.grade);
}

/// Each display axis carries exactly ONE ladder under every orientation (all cells in a
/// column share density or all share grade — what the axis labels rely on).
#[test]
fn axes_stay_pure() {
    for o in 0..4 {
        for c in 0..test_strip::COLS {
            let column: Vec<_> = (0..test_strip::ROWS)
                .map(|r| test_strip::cell(r, c, o).unwrap())
                .collect();
            let same_density = column.iter().all(|x| x.density == column[0].density);
            let same_grade = column.iter().all(|x| x.grade == column[0].grade);
            assert_ne!(
                same_density, same_grade,
                "column {c} at o={o} must carry exactly one ladder"
            );
        }
        for r in 0..test_strip::ROWS {
            let row: Vec<_> = (0..test_strip::COLS)
                .map(|c| test_strip::cell(r, c, o).unwrap())
                .collect();
            let same_density = row.iter().all(|x| x.density == row[0].density);
            let same_grade = row.iter().all(|x| x.grade == row[0].grade);
            assert_ne!(
                same_density, same_grade,
                "row {r} at o={o} must carry exactly one ladder"
            );
        }
    }
}

// ═══════════════════════════════ ZonePlacementTests ═══════════════════════════════

/// A plausible frame: bounds/anchor in the fixtures' working range.
fn analysis() -> ExposureAnalysis {
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: [-2.0; 3],
            ceils: [-0.7; 3],
        },
        anchor: 0.46,
        textural_range: 1.0,
        shadow_point: None,
        highlight_point: None,
        shadow_refs: [-0.9; 3],
        neutral_mid: None,
        neutral_shadow: None,
        neutral_highlight: None,
        neutral_confidence: None,
    }
}

fn pin(val_luma: f64, target: f64) -> Pin {
    Pin::new(0.5, 0.5, [val_luma; 3], val_luma, target)
}

fn predicted(s: &ExposureSettings, val_luma: f64) -> f64 {
    zone_placement::predicted_zone(s, &analysis(), val_luma, 0.0)
}

fn solve(settings: &ExposureSettings, pins: &[Pin]) -> Option<zone_placement::Solution> {
    zone_placement::solve(settings, &analysis(), pins)
}

// MARK: Ruler

#[test]
fn encoded_of_zone_is_exact_inverse() {
    for i in 0..=100 {
        let z = i as f64 / 10.0;
        let enc = densitometry::encoded_of_zone(z);
        assert!((densitometry::zone_of_encoded(enc) - z).abs() < 1e-12);
    }
    assert_eq!(
        densitometry::encoded_of_zone(5.0),
        densitometry::mid_gray_encoded()
    );
    assert_eq!(densitometry::encoded_of_zone(0.0), 0.0);
    assert_eq!(densitometry::encoded_of_zone(10.0), 1.0);
    // Out-of-range asks clamp to the paper's ends.
    assert_eq!(densitometry::encoded_of_zone(-1.0), 0.0);
    assert_eq!(densitometry::encoded_of_zone(12.0), 1.0);
}

// MARK: Forward model

#[test]
fn predicted_zone_is_decreasing_in_density_and_val() {
    let mut s = ExposureSettings::default();
    s.auto_exposure = false;
    let mid = 0.5;
    let mut last = f64::INFINITY;
    let mut density = -2.0;
    while density <= 5.0 + 1e-9 {
        s.density = density;
        let z = predicted(&s, mid);
        assert!(z <= last + 1e-9, "zone must fall as the print darkens");
        last = z;
        density += 0.5;
    }
    s.density = 1.0;
    // Higher normalized-log = denser negative = darker print = lower zone.
    let dark = predicted(&s, 0.9);
    let light = predicted(&s, 0.1);
    assert!(dark < light);
}

#[test]
fn predicted_zone_sees_exposure_stops_and_levels() {
    let mut s = ExposureSettings::default();
    s.auto_exposure = false;
    let base = predicted(&s, 0.5);
    s.exposure_stops = 1.0;
    let brighter = predicted(&s, 0.5);
    assert!(
        brighter > base + 0.2,
        "a +1 stop must lift the predicted zone"
    );
    s.exposure_stops = 0.0;
    s.levels_green = vec![[0.5, 0.7]];
    let remapped = predicted(&s, 0.5);
    assert!(
        remapped > base,
        "the green levels remap reshapes the displayed tone"
    );
}

// MARK: 1 pin

#[test]
fn one_pin_places_the_tone_and_disables_auto_exposure() {
    let p = pin(0.5, 6.5);
    let sol = solve(&ExposureSettings::default(), &[p]).expect("solution");
    assert!(!sol.settings.auto_exposure);
    assert!(sol.settings.auto_normalize_contrast); // 1 pin leaves auto grade alone
    assert!(!sol.clamped);
    assert_eq!(sol.knee_label, None);
    assert!((sol.achieved[0] - 6.5).abs() <= ON_TARGET_TOL);
    let live = predicted(&sol.settings, p.val_luma);
    assert!(
        (live - sol.achieved[0]).abs() < 1e-9,
        "achieved must be the rounded settings' truth"
    );
    // Rounded to the slider's precision.
    let d = sol.settings.density * 100.0;
    assert!((d - d.round()).abs() < 1e-9);
}

#[test]
fn one_pin_unreachable_target_clamps_to_the_slider_end() {
    // Zone 10 for a deep-shadow tone: brighter than the slider can print.
    let sol = solve(&ExposureSettings::default(), &[pin(0.95, 10.0)]).expect("solution");
    assert!(sol.clamped);
    assert_eq!(sol.settings.density, zone_placement::DENSITY_RANGE.0);
}

// MARK: 2 pins

/// Feasible asks derived from the model itself: what the tones print at with autos off,
/// nudged outward by half a zone (a slightly harder grade, well inside R50…R180).
fn feasible_outer_pins() -> (Pin, Pin) {
    let mut s = ExposureSettings::default();
    s.auto_exposure = false;
    s.auto_normalize_contrast = false;
    let z_dark = predicted(&s, 0.8);
    let z_light = predicted(&s, 0.2);
    (pin(0.8, z_dark - 0.5), pin(0.2, z_light + 0.5))
}

#[test]
fn two_pins_place_both_tones_and_disable_both_autos() {
    let (dark, light) = feasible_outer_pins();
    let sol = solve(&ExposureSettings::default(), &[dark, light]).expect("solution");
    assert!(!sol.settings.auto_exposure && !sol.settings.auto_normalize_contrast);
    assert!(!sol.clamped);
    assert_eq!(sol.knee_label, None);
    assert!((sol.achieved[0] - dark.target_zone).abs() <= ON_TARGET_TOL);
    assert!((sol.achieved[1] - light.target_zone).abs() <= ON_TARGET_TOL);
    assert_eq!(sol.settings.grade, sol.settings.grade.round());
    assert!(sol.settings.grade >= K::ISO_R_MIN && sol.settings.grade <= K::ISO_R_MAX);
}

#[test]
fn two_pins_pin_order_does_not_matter() {
    let (dark, light) = feasible_outer_pins();
    let a = solve(&ExposureSettings::default(), &[dark, light]).expect("solution");
    let b = solve(&ExposureSettings::default(), &[light, dark]).expect("solution");
    assert_eq!(a.settings, b.settings);
    let mut rev = b.achieved.clone();
    rev.reverse();
    assert_eq!(a.achieved, rev);
}

#[test]
fn two_pins_degenerate_tones_return_none() {
    let sol = solve(
        &ExposureSettings::default(),
        &[pin(0.5, 3.0), pin(0.5 + 1e-4, 8.0)],
    );
    assert!(sol.is_none());
}

#[test]
fn two_pins_impossible_spread_clamps_honestly() {
    // Nearly identical tones asked to sit 6 zones apart: no grade can.
    let sol = solve(
        &ExposureSettings::default(),
        &[pin(0.52, 2.0), pin(0.48, 8.0)],
    )
    .expect("solution");
    assert!(sol.clamped);
    assert_eq!(
        sol.settings.grade,
        K::ISO_R_MIN,
        "maximum hardness is the closest answer"
    );
}

// MARK: 3 pins

#[test]
fn third_pin_on_target_needs_no_knee() {
    // First solve the outer pair alone, then ask the mid pin for exactly what it already
    // gets — the knee must not engage.
    let (dark, light) = feasible_outer_pins();
    let outer = solve(&ExposureSettings::default(), &[dark, light]).expect("outer");
    let mid_landed = predicted(&outer.settings, 0.55);
    let sol = solve(
        &ExposureSettings::default(),
        &[dark, light, pin(0.55, mid_landed)],
    )
    .expect("solution");
    assert_eq!(sol.knee_label, None);
    assert!(sol.settings.shadow_contrast == 0.0 && sol.settings.highlight_contrast == 0.0);
}

#[test]
fn third_pin_solves_a_knee_control_and_keeps_the_outer_tones() {
    // A shadow-side tone asked to sit off where the straight 2-pin solve puts it (the outer
    // asks themselves are feasible).
    let (dark, light) = feasible_outer_pins();
    let outer = solve(&ExposureSettings::default(), &[dark, light]).expect("outer");
    let mid_val = 0.7;
    let landed = predicted(&outer.settings, mid_val);
    // Modest ask: within the knee's physical reach against the outer re-solve's
    // counteraction (a large ask clamps honestly instead).
    let mid = pin(mid_val, (landed + 0.3).min(9.0));
    let sol = solve(&ExposureSettings::default(), &[dark, light, mid]).expect("solution");
    assert!(
        sol.knee_label.is_some(),
        "an off-target mid pin must engage a knee"
    );
    assert!(
        !sol.clamped,
        "a modest mid ask must be met without clamping"
    );
    for (a, p) in sol.achieved.iter().zip([dark, light, mid]) {
        assert!(
            (a - p.target_zone).abs() <= ON_TARGET_TOL,
            "solution must meet every ask (zone {a} vs {})",
            p.target_zone
        );
    }
    // The solved knee is a real slider value inside its range.
    let cand = KNEE_CANDIDATES
        .iter()
        .find(|c| Some(c.label) == sol.knee_label)
        .expect("candidate");
    let value = cand.control.get(&sol.settings);
    assert!(value >= cand.lo && value <= cand.hi);
    assert_ne!(value, 0.0, "the engaged knee must actually move");
}

#[test]
fn four_pins_return_none() {
    let pins: Vec<Pin> = [0.2, 0.4, 0.6, 0.8].iter().map(|&v| pin(v, 5.0)).collect();
    assert!(solve(&ExposureSettings::default(), &pins).is_none());
    assert!(solve(&ExposureSettings::default(), &[]).is_none());
}

/// The solve must respect the frame's own metering the way the render will: settings that
/// carry crop/color state pass through untouched.
#[test]
fn solve_preserves_unrelated_settings() {
    let mut s = ExposureSettings::default();
    s.temp = 0.4;
    s.vibrance = 1.3;
    s.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.8, 0.8));
    let sol = solve(&s, &[pin(0.5, 6.0)]).expect("solution");
    assert_eq!(sol.settings.temp, 0.4);
    assert_eq!(sol.settings.vibrance, 1.3);
    assert_eq!(sol.settings.crop_rect, s.crop_rect);
}

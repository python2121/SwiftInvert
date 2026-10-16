//! `TestStripRenderTests` (Tests/SwiftInvertTests/): a strip patch is byte-identical to a full
//! render at that patch's settings; the ladder rotation is a bijection whose axes stay pure;
//! the strip reads from the warm tower (one prepare, one source, one mask plane).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::collections::HashSet;
use std::sync::Arc;

use negkit::test_strip::{COLS, DENSITIES_BY_COLUMN, GRADES_BY_ROW, ROWS};
use negkit::{ExposureSettings, NormalizedRect};
use negsession::{CpuRenderer, RenderRequest, Session, TestStripOutput, TestStripRequest};

fn session() -> Session {
    let native = common::synthetic_negative();
    Session::synthetic("strip.CR3", native, None, Arc::new(CpuRenderer))
}

/// The patch window of `cell` in the mosaic equals the same window of a full-frame render at
/// the cell's density/grade over the strip's base settings — byte for byte.
fn assert_patch_matches_full(s: &mut Session, strip: &TestStripOutput, row: usize, col: usize) {
    let cell = *strip.cell(row, col).unwrap();
    let full = s
        .render_with(
            &TestStripOutput::settings_for(&cell, s.settings()),
            RenderRequest {
                compute_histogram: false,
                ..RenderRequest::default()
            },
        )
        .unwrap();
    assert_eq!((full.width, full.height), (strip.width, strip.height));
    let bytes = full.frame.rgba8().unwrap();
    let r = strip.patch_rect(row, col);
    for y in r.y0..r.y1 {
        let a = (y * strip.width + r.x0) * 4..(y * strip.width + r.x1) * 4;
        assert_eq!(
            &strip.rgba8[a.clone()],
            &bytes[a],
            "row {y} of patch ({row},{col}) differs"
        );
    }
}

#[test]
fn patch_equals_full_render_at_its_settings() {
    let mut s = session();
    s.update_settings(|st| {
        st.crop_rect = Some(NormalizedRect::new(0.1, 0.05, 0.8, 0.9));
        st.exposure_stops = 0.3;
        st.contrast_mask = 0.25;
    });
    let strip = s.render_test_strip(TestStripRequest::default()).unwrap();
    let stats = s.stats();
    assert_eq!(stats.prepares, 1, "one analysis for all 25 patches");
    assert_eq!(stats.source_builds, 1, "one cropped source");
    assert_eq!(stats.mask_builds, 1, "one plane");
    assert_eq!(stats.renders, 25);
    assert_eq!(strip.rgba8.len(), strip.width * strip.height * 4);
    assert_eq!(strip.cells.len(), ROWS * COLS);
    // Corners + centre + the current-settings rung.
    for (r, c) in [(0, 0), (0, 4), (4, 0), (4, 4), (2, 2)] {
        assert_patch_matches_full(&mut s, &strip, r, c);
    }
    let cur = strip.current;
    assert_patch_matches_full(&mut s, &strip, cur.row, cur.col);
    // Default density 1.0 / grade 115 is the centre rung at orientation 0.
    assert_eq!((cur.row, cur.col), (2, 2));
    // The full renders reused the strip's source and plane.
    let after = s.stats();
    assert_eq!(after.prepares, 1);
    assert_eq!(after.source_builds, 1);
    assert_eq!(after.mask_builds, 1);
    // Geometry mirrors a cropped proxy render.
    let out = s.render(RenderRequest::default()).unwrap();
    assert_eq!(strip.frame_size, out.frame_size);
    assert!((strip.display_aspect - out.display_aspect).abs() < 1e-12);
    assert_eq!(strip.content_window, out.content_window);
}

/// Rotating the ladder permutes WHICH (density, grade) each display cell renders — every
/// orientation covers all 25 rungs exactly once, each patch still equals its own full render,
/// and the accented rung follows the rotation.
#[test]
fn ladder_rotation_is_a_bijection_through_the_session() {
    let mut s = session();
    let all: HashSet<(u64, u64)> = DENSITIES_BY_COLUMN
        .iter()
        .flat_map(|d| {
            GRADES_BY_ROW
                .iter()
                .map(move |g| (d.to_bits(), g.to_bits()))
        })
        .collect();
    let mut mosaics = Vec::new();
    for orientation in 0..4 {
        let strip = s
            .render_test_strip(TestStripRequest {
                orientation,
                srgb: true,
            })
            .unwrap();
        assert_eq!(strip.orientation, orientation);
        let seen: HashSet<(u64, u64)> = strip
            .cells
            .iter()
            .map(|c| (c.density.to_bits(), c.grade.to_bits()))
            .collect();
        assert_eq!(
            seen, all,
            "orientation {orientation} covers every rung once"
        );
        // Axis purity: at even orientations a row shares its grade, at odd ones its density.
        for row in 0..ROWS {
            let cells: Vec<_> = strip.cells.iter().filter(|c| c.row == row).collect();
            if orientation % 2 == 0 {
                assert!(cells.iter().all(|c| c.grade == cells[0].grade));
            } else {
                assert!(cells.iter().all(|c| c.density == cells[0].density));
            }
        }
        // The accented rung is the cell carrying the base settings' values.
        let cur = strip.cell(strip.current.row, strip.current.col).unwrap();
        assert_eq!((cur.density, cur.grade), (1.0, 115.0));
        // Patch == full render under a rotated ladder too (sRGB leg included).
        let cell = *strip.cell(1, 3).unwrap();
        let full = s
            .render_with(
                &TestStripOutput::settings_for(&cell, s.settings()),
                RenderRequest {
                    srgb: true,
                    compute_histogram: false,
                    ..RenderRequest::default()
                },
            )
            .unwrap();
        let r = strip.patch_rect(1, 3);
        let bytes = full.frame.rgba8().unwrap();
        for y in r.y0..r.y1 {
            let a = (y * strip.width + r.x0) * 4..(y * strip.width + r.x1) * 4;
            assert_eq!(&strip.rgba8[a.clone()], &bytes[a]);
        }
        mosaics.push(strip.rgba8);
    }
    // Orientation 2 is the 180° ladder: the dense/hard corner moved, so the mosaics differ.
    assert_ne!(mosaics[0], mosaics[2]);
    // 4 is 0 again.
    let again = s
        .render_test_strip(TestStripRequest {
            orientation: 4,
            srgb: true,
        })
        .unwrap();
    assert_eq!(again.rgba8, mosaics[0]);
}

#[test]
fn labels_and_picks() {
    let mut s = session();
    let strip = s.render_test_strip(TestStripRequest::default()).unwrap();
    let c00 = strip.cell(0, 0).unwrap();
    assert_eq!(TestStripOutput::label(c00), "Brightness 0.4 · Grade 75");
    let picked = TestStripOutput::settings_for(c00, &ExposureSettings::default());
    assert_eq!(picked.density, 1.6);
    assert_eq!(picked.grade, 75.0);
    assert!(picked.auto_exposure, "autos untouched by a pick");
    // Hover mapping: content-normalized → the cell under the pointer; edges clamp.
    assert_eq!(strip.cell_at(0.0, 0.0).unwrap().row, 0);
    let last = strip.cell_at(1.0, 1.0).unwrap();
    assert_eq!((last.row, last.col), (4, 4));
    let mid = strip.cell_at(0.5, 0.5).unwrap();
    assert_eq!((mid.row, mid.col), (2, 2));
}

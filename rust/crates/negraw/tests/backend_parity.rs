//! lcraw (pure Rust) vs LibRaw on the same file — the parity number PLAN §0 asks for.
//!
//! Runs only with BOTH backends compiled (`--features libraw`) and `NEG_RAW_SAMPLE` set; otherwise
//! it prints why it skipped and passes. Compares the PREVIEW path (binned / half-size linear),
//! which is what analysis and the proxy render see, and the full path's means. Prints the numbers
//! under `--nocapture`; asserts only that the two previews agree in size (modulo the default
//! crop) and that the per-channel means are within 0.2 % in the default (LibRaw-parity)
//! configuration; the per-sample max/mean figures are printed for the record.

#![cfg(all(feature = "lcraw", feature = "libraw"))]

use std::path::PathBuf;
use std::time::Instant;

use negraw::RawBackend;
use negraw::lcraw::LcrawBackend;
use negraw::libraw::LibrawBackend;

fn means(d: &[f32]) -> [f64; 3] {
    let mut s = [0f64; 3];
    for p in d.chunks_exact(3) {
        for c in 0..3 {
            s[c] += f64::from(p[c]);
        }
    }
    let n = (d.len() / 3).max(1) as f64;
    [s[0] / n, s[1] / n, s[2] / n]
}

#[test]
fn lcraw_matches_libraw_on_the_sample() {
    let Some(path) = std::env::var_os("NEG_RAW_SAMPLE").map(PathBuf::from) else {
        eprintln!("skip: NEG_RAW_SAMPLE not set");
        return;
    };
    let libraw = LibrawBackend;
    let t = Instant::now();
    let lr = libraw.decode_preview(&path).expect("libraw preview");
    let t_lr = t.elapsed();

    for (label, lc) in [
        (
            "lcraw default (libraw area + bit-depth white)",
            LcrawBackend::default(),
        ),
        (
            "lcraw file-native (default crop + file white)",
            LcrawBackend::file_native(),
        ),
    ] {
        let t = Instant::now();
        let ours = lc.decode_preview(&path).expect("lcraw preview");
        let t_lc = t.elapsed();
        let (a, b) = (&lr.image, &ours.image);
        eprintln!(
            "{label}: libraw {}×{} ({t_lr:?}) vs lcraw {}×{} ({t_lc:?}); flips {} / {}",
            a.width, a.height, b.width, b.height, lr.flip, ours.flip
        );
        let (ma, mb) = (means(&a.data), means(&b.data));
        eprintln!("  means libraw {ma:?} lcraw {mb:?}");
        if a.width == b.width && a.height == b.height {
            let (mut max, mut sum) = (0f64, 0f64);
            for (x, y) in a.data.iter().zip(&b.data) {
                let d = (f64::from(*x) - f64::from(*y)).abs();
                max = max.max(d);
                sum += d;
            }
            eprintln!(
                "  SAME SIZE: per-sample max {max:.3e} mean {:.3e}",
                sum / a.data.len() as f64
            );
        } else {
            eprintln!(
                "  size differs by {}×{} px (crop margins)",
                a.width.abs_diff(b.width),
                a.height.abs_diff(b.height)
            );
        }
        if label.starts_with("lcraw default") {
            assert_eq!((a.width, a.height), (b.width, b.height), "{label}: size");
            for c in 0..3 {
                let rel = (ma[c] - mb[c]).abs() / ma[c].max(1e-9);
                assert!(
                    rel < 0.002,
                    "{label}: channel {c} mean differs by {rel:.4} (libraw {} vs lcraw {})",
                    ma[c],
                    mb[c]
                );
            }
        } else {
            // the file's white is a uniform scale: the three channel ratios must agree
            let r: Vec<f64> = (0..3).map(|c| mb[c] / ma[c].max(1e-9)).collect();
            eprintln!("  uniform scale ratios {r:?}");
        }
    }

    let t = Instant::now();
    let lr_full = libraw.decode_full(&path).expect("libraw full");
    let t_lr = t.elapsed();
    let t = Instant::now();
    let lc_full = LcrawBackend::libraw_area()
        .decode_full(&path)
        .expect("lcraw full");
    let t_lc = t.elapsed();
    eprintln!(
        "full: libraw {}×{} ({t_lr:?}) means {:?} vs lcraw {}×{} ({t_lc:?}) means {:?}",
        lr_full.image.width,
        lr_full.image.height,
        means(&lr_full.image.data),
        lc_full.image.width,
        lc_full.image.height,
        means(&lc_full.image.data)
    );
}

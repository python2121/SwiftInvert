//! Smoke test against a real RAW: set `NEG_RAW_SAMPLE=/path/to/file.ext` to run it; otherwise it
//! prints why it skipped and passes (no media lives in the repo).
//!
//! `cargo test -p negraw --test smoke -- --nocapture` shows dims, timings and value ranges.

use std::path::PathBuf;
use std::time::Instant;

use negraw::{CfaKind, Decoded, RawBackend};

fn stats(d: &Decoded) -> String {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut sum = [0f64; 3];
    for p in d.image.data.chunks_exact(3) {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
            sum[c] += f64::from(p[c]);
        }
    }
    let n = d.image.pixel_count() as f64;
    format!(
        "{}×{} flip {} xtrans {} min {:?} max {:?} mean [{:.4}, {:.4}, {:.4}]",
        d.image.width,
        d.image.height,
        d.flip,
        d.is_xtrans,
        lo,
        hi,
        sum[0] / n,
        sum[1] / n,
        sum[2] / n
    )
}

#[test]
fn smoke_decode_sample_raw() {
    let Some(path) = std::env::var_os("NEG_RAW_SAMPLE").map(PathBuf::from) else {
        eprintln!(
            "skip: NEG_RAW_SAMPLE is not set (point it at a camera RAW to exercise the decoder)"
        );
        return;
    };
    let backend = negraw::default_backend();
    eprintln!("backend: {}", backend.name());
    assert!(
        negraw::is_raw_file(&path),
        "{} has no RAW extension",
        path.display()
    );

    let t = Instant::now();
    let info = backend.probe(&path).expect("probe");
    eprintln!(
        "probe {:.1} ms: {info:?} oriented {:?}",
        t.elapsed().as_secs_f64() * 1e3,
        info.oriented_size()
    );
    assert!(info.width > 0 && info.height > 0);
    #[cfg(feature = "lcraw")]
    {
        // LibRaw outputs the whole image area (no default crop): report both so the dims divergence is visible
        let area = negraw::lcraw::LcrawBackend::libraw_area()
            .probe(&path)
            .expect("probe (libraw area)");
        eprintln!(
            "probe without default crop (LibRaw area): {}×{}",
            area.width, area.height
        );
    }

    let t = Instant::now();
    let thumb = backend.thumbnail(&path).expect("thumbnail");
    match &thumb {
        Some(bytes) => {
            eprintln!(
                "thumbnail {:.1} ms: {} bytes JPEG",
                t.elapsed().as_secs_f64() * 1e3,
                bytes.len()
            );
            assert!(bytes.starts_with(&[0xFF, 0xD8]));
        }
        None => eprintln!("thumbnail: none embedded"),
    }

    let t = Instant::now();
    let preview = backend.decode_preview(&path).expect("decode_preview");
    eprintln!(
        "preview {:.1} ms: {}",
        t.elapsed().as_secs_f64() * 1e3,
        stats(&preview)
    );
    assert!(preview.exif_orientation_applied);
    assert!(
        preview.image.data.iter().all(|v| (0.0..=1.0).contains(v)),
        "preview outside [0,1]"
    );
    let (ow, oh) = info.oriented_size();
    let k = match info.cfa {
        CfaKind::Bayer => 2,
        CfaKind::XTrans if backend.name() == "lcraw" => 3,
        _ => 1,
    };
    assert!(
        preview.image.width.abs_diff(ow / k) <= 1 && preview.image.height.abs_diff(oh / k) <= 1,
        "preview dims {}×{} vs probe {ow}×{oh} / {k}",
        preview.image.width,
        preview.image.height
    );

    let t = Instant::now();
    let full = backend.decode_full(&path).expect("decode_full");
    eprintln!(
        "full {:.1} ms: {}",
        t.elapsed().as_secs_f64() * 1e3,
        stats(&full)
    );
    assert!(
        full.image.data.iter().all(|v| (0.0..=1.0).contains(v)),
        "full outside [0,1]"
    );
    assert_eq!(
        (full.image.width, full.image.height),
        (ow, oh),
        "full dims vs probe"
    );
    assert_eq!(full.flip, preview.flip);
}

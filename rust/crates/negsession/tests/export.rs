//! `ExportOptionsTests` + `AuditExportStickyTests` + `AuditExporterWriteTests`
//! (Tests/SwiftInvertTests/) — the pure path math, the sticky JSON, and `write_encoded` through
//! every format/colour-space arm, re-opened and checked for dims + the embedded ICC profile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::io::BufReader;
use std::path::{Path, PathBuf};

use image::ImageDecoder;
use negsession::{ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter};

fn src() -> PathBuf {
    PathBuf::from("/roll/IMG_0001.CR3")
}

#[test]
fn next_to_source_by_default() {
    let options = ExportOptions::default();
    assert_eq!(
        options.destination_path(&src()),
        Path::new("/roll/IMG_0001.jpg")
    );
}

#[test]
fn tiff_extension() {
    let options = ExportOptions {
        format: ExportFormat::Tiff16,
        ..Default::default()
    };
    assert_eq!(
        options.destination_path(&src()),
        Path::new("/roll/IMG_0001.tiff")
    );
}

#[test]
fn custom_destination_redirects_the_directory_only() {
    let options = ExportOptions {
        destination: ExportDestination::Folder(PathBuf::from("/exports")),
        ..Default::default()
    };
    assert_eq!(
        options.destination_path(&src()),
        Path::new("/exports/IMG_0001.jpg")
    );
}

/// The persisted blob can hold the custom flag without a path — it must fall back to
/// next-to-source, not crash.
#[test]
fn custom_flag_without_path_falls_back() {
    let options = ExportOptions::from_sticky_json(
        r#"{"format":"jpeg","jpegQuality":0.92,"resize":false,"maxLongEdge":3000,"colorSpace":"sRGB","useCustomDestination":true,"customDestinationPath":null}"#,
    )
    .unwrap();
    assert_eq!(options.destination, ExportDestination::NextToSource);
    assert_eq!(
        options.destination_path(&src()),
        Path::new("/roll/IMG_0001.jpg")
    );
}

/// Re-exporting the same frame overwrites: the name derives from the source basename alone.
#[test]
fn name_is_stable_across_options() {
    let a = ExportOptions {
        format: ExportFormat::Jpeg { quality: 0.5 },
        ..Default::default()
    };
    let b = ExportOptions {
        format: ExportFormat::Jpeg { quality: 0.99 },
        ..Default::default()
    };
    assert_eq!(a.destination_path(&src()), b.destination_path(&src()));
}

#[test]
fn defaults_match_the_sheet() {
    let d = ExportOptions::default();
    assert_eq!(d.format, ExportFormat::Jpeg { quality: 0.92 });
    assert_eq!(d.colorspace, ExportColorSpace::Srgb);
    assert_eq!(d.resize_long_edge, None);
    assert_eq!(d.destination, ExportDestination::NextToSource);
    assert_eq!(negsession::export::DEFAULT_MAX_LONG_EDGE, 3000);
    assert_eq!(ExportFormat::Tiff16.label(), "TIFF (16-bit)");
    assert_eq!(ExportColorSpace::AdobeRgb.label(), "Adobe RGB (wide gamut)");
}

#[test]
fn sticky_round_trip_and_swift_compat() {
    let options = ExportOptions {
        format: ExportFormat::Tiff16,
        colorspace: ExportColorSpace::AdobeRgb,
        resize_long_edge: Some(2048),
        destination: ExportDestination::Folder(PathBuf::from("/exports")),
    };
    let json = options.to_sticky_json().unwrap();
    // The Swift keys (and the `rommRGB` case name kept for compatibility).
    assert!(json.contains("\"colorSpace\":\"rommRGB\""), "{json}");
    assert!(json.contains("\"format\":\"tiff16\""), "{json}");
    assert!(json.contains("\"useCustomDestination\":true"), "{json}");
    assert_eq!(ExportOptions::from_sticky_json(&json).unwrap(), options);

    // A Swift-written blob decodes.
    let swift = r#"{"colorSpace":"sRGB","customDestinationPath":null,"format":"jpeg","jpegQuality":0.8,"maxLongEdge":3000,"resize":true,"useCustomDestination":false}"#;
    let o = ExportOptions::from_sticky_json(swift).unwrap();
    assert_eq!(o.format, ExportFormat::Jpeg { quality: 0.8 });
    assert_eq!(o.resize_long_edge, Some(3000));
    // Corrupt → None (callers fall back to defaults); missing keys → defaults.
    assert!(ExportOptions::from_sticky_json("{broken").is_none());
    assert_eq!(
        ExportOptions::from_sticky_json("{}").unwrap(),
        ExportOptions::default()
    );
}

fn reopen_jpeg(path: &Path) -> (u32, u32, Option<Vec<u8>>) {
    let file = std::fs::File::open(path).unwrap();
    let mut dec = image::codecs::jpeg::JpegDecoder::new(BufReader::new(file)).unwrap();
    let (w, h) = dec.dimensions();
    let icc = dec.icc_profile().unwrap();
    (w, h, icc)
}

fn reopen_tiff(path: &Path) -> (u32, u32, Option<Vec<u8>>, image::ColorType) {
    let file = std::fs::File::open(path).unwrap();
    let mut dec = image::codecs::tiff::TiffDecoder::new(BufReader::new(file)).unwrap();
    let (w, h) = dec.dimensions();
    let icc = dec.icc_profile().unwrap();
    let ct = dec.color_type();
    (w, h, icc, ct)
}

#[test]
fn writes_every_format_and_color_space_arm_with_icc() {
    let dir = common::scratch_dir("export");
    let image = common::encoded_gradient();
    let srgb_icc = ExportColorSpace::Srgb.icc_profile();
    let adobe_icc = ExportColorSpace::AdobeRgb.icc_profile();
    assert!(srgb_icc.len() > 128 && adobe_icc.len() > 128);
    assert_ne!(srgb_icc, adobe_icc);
    // ICC header sanity: 'acsp' signature at byte 36, declared size matches.
    for icc in [&srgb_icc, &adobe_icc] {
        assert_eq!(&icc[36..40], b"acsp");
        let declared = u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize;
        assert_eq!(declared, icc.len());
    }

    // JPEG / sRGB (the default arm).
    let jpeg = dir.join("out-srgb.jpg");
    Exporter::write_encoded(&image, &jpeg, &ExportOptions::default()).unwrap();
    let (w, h, icc) = reopen_jpeg(&jpeg);
    assert_eq!((w, h), (32, 24));
    assert_eq!(icc.as_deref(), Some(srgb_icc.as_slice()));

    // TIFF / sRGB (16-bit path).
    let tiff = dir.join("out-srgb.tiff");
    let opts = ExportOptions {
        format: ExportFormat::Tiff16,
        ..Default::default()
    };
    Exporter::write_encoded(&image, &tiff, &opts).unwrap();
    let (w, h, icc, ct) = reopen_tiff(&tiff);
    assert_eq!((w, h), (32, 24));
    assert_eq!(ct, image::ColorType::Rgb16);
    assert_eq!(icc.as_deref(), Some(srgb_icc.as_slice()));

    // JPEG / wide gamut (Adobe RGB): tagged with the Adobe-compatible profile.
    let wide = dir.join("out-wide.jpg");
    let opts = ExportOptions {
        colorspace: ExportColorSpace::AdobeRgb,
        ..Default::default()
    };
    Exporter::write_encoded(&image, &wide, &opts).unwrap();
    let (w, h, icc) = reopen_jpeg(&wide);
    assert_eq!((w, h), (32, 24));
    assert_eq!(icc.as_deref(), Some(adobe_icc.as_slice()));

    // TIFF / Adobe.
    let wide_tiff = dir.join("out-wide.tiff");
    let opts = ExportOptions {
        format: ExportFormat::Tiff16,
        colorspace: ExportColorSpace::AdobeRgb,
        ..Default::default()
    };
    Exporter::write_encoded(&image, &wide_tiff, &opts).unwrap();
    let (_, _, icc, _) = reopen_tiff(&wide_tiff);
    assert_eq!(icc.as_deref(), Some(adobe_icc.as_slice()));
}

/// 16-bit TIFF pixels round-trip the encoded values to 1/65535.
#[test]
fn tiff16_preserves_the_encoded_values() {
    let dir = common::scratch_dir("export");
    let image = common::encoded_gradient();
    let tiff = dir.join("exact.tiff");
    let opts = ExportOptions {
        format: ExportFormat::Tiff16,
        ..Default::default()
    };
    Exporter::write_encoded(&image, &tiff, &opts).unwrap();
    let decoded = image::open(&tiff).unwrap().into_rgb16();
    for (i, px) in decoded.pixels().enumerate() {
        for ch in 0..3 {
            let want = image.data[i * 3 + ch];
            let got = px.0[ch] as f32 / 65535.0;
            assert!(
                (want - got).abs() < 1.0 / 65535.0,
                "px {i} ch {ch}: {want} vs {got}"
            );
        }
    }
}

#[test]
fn resize_caps_the_long_edge() {
    let dir = common::scratch_dir("export");
    let out = dir.join("out-small.jpg");
    let opts = ExportOptions {
        resize_long_edge: Some(16),
        ..Default::default()
    };
    Exporter::write_encoded(&common::encoded_gradient(), &out, &opts).unwrap();
    // 32×24 at long edge 16 → scale 0.5 → 16×12 (RgbImage::downsampled).
    let (w, h, _) = reopen_jpeg(&out);
    assert_eq!((w, h), (16, 12));
    // Below the 16 px floor the resize is ignored.
    let out2 = dir.join("out-tiny.jpg");
    let opts = ExportOptions {
        resize_long_edge: Some(8),
        ..Default::default()
    };
    Exporter::write_encoded(&common::encoded_gradient(), &out2, &opts).unwrap();
    assert_eq!(reopen_jpeg(&out2).0, 32);
}

#[test]
fn unwritable_destination_errors() {
    let bad = Path::new("/proc/nope/sub/file.jpg");
    assert!(
        Exporter::write_encoded(&common::encoded_gradient(), bad, &ExportOptions::default())
            .is_err()
    );
}

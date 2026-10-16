//! End-to-end smoke on a real RAW: set `NEG_RAW_SAMPLE=/path/to/file.CR3` to run (skipped with
//! a printed reason otherwise). Opens the file through the default decoder, renders the proxy,
//! exports a JPEG and a TIFF to a scratch dir and checks dims + the embedded ICC tag.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use image::ImageDecoder;
use negsession::{
    CpuRenderer, ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter,
    RenderRequest, Session,
};

fn sample() -> Option<PathBuf> {
    let p = std::env::var_os("NEG_RAW_SAMPLE").map(PathBuf::from)?;
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("NEG_RAW_SAMPLE={} is not a file — skipping", p.display());
        None
    }
}

#[test]
fn opens_renders_and_exports_a_real_raw() {
    let Some(path) = sample() else {
        eprintln!("raw_smoke: NEG_RAW_SAMPLE unset — skipped");
        return;
    };
    let backend: Arc<dyn negraw::RawBackend> = Arc::from(negraw::default_backend());
    let mut session = Session::open(&path, Arc::clone(&backend), Arc::new(CpuRenderer)).unwrap();
    let (pw, ph) = session.proxy_size();
    assert_eq!(pw.max(ph), 1536, "proxy long edge");
    assert_eq!(session.stats().preview_decodes, 1);

    let out = session
        .render(RenderRequest {
            srgb: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!((out.width, out.height), (pw, ph));
    assert!(out.density_range > 0.0);
    let info = session.session_info().unwrap();
    assert!(info.character.is_some(), "a real negative has a character");
    assert!(session.thumbnail().unwrap().is_some_and(|b| b.len() > 1000));

    let dir = common::scratch_dir("raw-smoke");
    let probe = backend.probe(&path).unwrap();
    let (fw, fh) = probe.oriented_size();

    // JPEG, sRGB, resized to 3000 — the sheet's sticky default when resize is on.
    let jpeg_opts = ExportOptions {
        format: ExportFormat::Jpeg { quality: 0.92 },
        colorspace: ExportColorSpace::Srgb,
        resize_long_edge: Some(3000),
        destination: ExportDestination::Folder(dir.clone()),
    };
    let jpeg_path = Exporter::write(&mut session, &jpeg_opts).unwrap();
    assert_eq!(jpeg_path.extension().unwrap(), "jpg");
    let mut dec = image::codecs::jpeg::JpegDecoder::new(BufReader::new(
        std::fs::File::open(&jpeg_path).unwrap(),
    ))
    .unwrap();
    let (w, h) = dec.dimensions();
    assert_eq!(w.max(h), 3000);
    let scale = 3000.0 / fw.max(fh) as f64;
    assert_eq!(w as f64, (fw as f64 * scale).round());
    assert_eq!(h as f64, (fh as f64 * scale).round());
    assert_eq!(
        dec.icc_profile().unwrap().as_deref(),
        Some(ExportColorSpace::Srgb.icc_profile().as_slice())
    );

    // TIFF-16, Adobe RGB, full size: the decoder's oriented dims.
    let tiff_opts = ExportOptions {
        format: ExportFormat::Tiff16,
        colorspace: ExportColorSpace::AdobeRgb,
        resize_long_edge: None,
        destination: ExportDestination::Folder(dir.clone()),
    };
    let tiff_path = Exporter::write(&mut session, &tiff_opts).unwrap();
    let mut dec = image::codecs::tiff::TiffDecoder::new(BufReader::new(
        std::fs::File::open(&tiff_path).unwrap(),
    ))
    .unwrap();
    assert_eq!(dec.dimensions(), (fw as u32, fh as u32));
    assert_eq!(dec.color_type(), image::ColorType::Rgb16);
    assert_eq!(
        dec.icc_profile().unwrap().as_deref(),
        Some(ExportColorSpace::AdobeRgb.icc_profile().as_slice())
    );
    assert_eq!(
        session.stats().prepares,
        1,
        "both exports shared the proxy analysis"
    );
    eprintln!(
        "raw_smoke: {} → {} ({w}×{h}) + {} ({fw}×{fh})",
        path.display(),
        jpeg_path.display(),
        tiff_path.display()
    );
}

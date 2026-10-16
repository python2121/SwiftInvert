//! The LibRaw backend — `Sources/RawDecodeKit/RawDecoder.swift` line by line over the C API.
//! Feature `libraw`; needs `libraw_r`, its headers and pkg-config on the build host (see
//! `build.rs`). This is the byte-exact-vs-NegPy reference decode: the same LibRaw parameters
//! rawpy uses (`gamma=(1,1)`, `no_auto_bright`, `adjust_maximum_thr=0`, `output_bps=16`,
//! `output_color=RAW`, unity WB, `user_flip=0` + the flip baked in code).
//!
//! Not compiled on the SteamOS host (no LibRaw); the Mac compile-tests it. Everything that does not
//! need the library (parameter order, CFA classification, u16 conversion) is kept in plain Rust
//! so the unit tests here still run with the feature on.

pub mod ffi;

use std::path::Path;

use crate::convert::rgb16_bytes_to_image;
use crate::orient::applying_flip;
use crate::{CfaKind, Decoded, NegRawError, RawBackend, RawInfoLite, Result};

/// LibRaw's `idata.filters` value for Fujifilm X-Trans.
pub const FILTERS_XTRANS: u32 = 9;

/// The LibRaw FFI backend (stateless; every call opens its own `libraw_data_t`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LibrawBackend;

/// `idata.filters` + `idata.colors` → [`CfaKind`]: 9 = X-Trans, 0 = no CFA (3 colours = linear
/// RGB, else mono), 1 = Leaf Catchlight's 16×16 (other), anything else = a 2×2 Bayer bitmask.
pub fn cfa_kind_from_filters(filters: u32, colors: i32) -> CfaKind {
    match filters {
        FILTERS_XTRANS => CfaKind::XTrans,
        0 if colors == 3 => CfaKind::Linear,
        0 => CfaKind::Mono,
        1 => CfaKind::Other,
        _ => CfaKind::Bayer,
    }
}

fn open(path: &Path) -> Result<ffi::Handle> {
    let mut h = ffi::Handle::new()?;
    h.open(path)?;
    Ok(h)
}

/// The NegPy decode contract (`linear_raw` path), in the Swift file's order.
fn configure(h: &mut ffi::Handle) {
    h.set_gamma(0, 1.0);
    h.set_gamma(1, 1.0);
    h.set_no_auto_bright(1);
    // Pin the u16 scale to the camera white level (NegPy 2a6cb22): the default
    // adjust_maximum_thr=0.75 rescales to the frame's own brightest pixel once it passes 75% of
    // white level — routine when the film base is exposed near clipping — putting each frame on
    // its own scale.
    h.set_adjust_maximum_thr(0.0);
    h.set_output_bps(16);
    h.set_output_color(0); // sensor-native, no color matrix
    h.set_use_camera_wb(0);
    h.set_use_auto_wb(0);
    for i in 0..4 {
        h.set_user_mul(i, 1.0); // unity WB
    }
    h.set_user_flip(0);
}

fn decode(path: &Path, preview: bool) -> Result<Decoded> {
    let t0 = std::time::Instant::now();
    let mut h = open(path)?;
    h.unpack()?;
    let is_xtrans = h.filters() == FILTERS_XTRANS;
    configure(&mut h);
    if preview && !is_xtrans {
        // half_size + linear demosaic — NegPy's fast preview path. X-Trans sensors skip half_size
        // (it aliases the 6×6 CFA) and take the default demosaic at full size.
        h.set_half_size(1);
        h.set_demosaic(0);
    }
    h.process()?;
    let flip = h.flip();
    let image = {
        let p = h.make_mem_image()?;
        if p.kind() != ffi::LIBRAW_IMAGE_BITMAP || p.bits() != 16 || p.colors() != 3 {
            return Err(NegRawError::UnsupportedOutput(format!(
                "type={} bits={} colors={}",
                p.kind(),
                p.bits(),
                p.colors()
            )));
        }
        rgb16_bytes_to_image(p.width(), p.height(), p.data())?
    };
    // With user_flip=0 the output is sensor-oriented; bake the camera's recorded orientation
    // ourselves (NegPy: apply_exif_orientation).
    let image = applying_flip(image, flip)?;
    log::debug!(
        "libraw: {} {}×{} (preview {preview}, xtrans {is_xtrans}, flip {flip}) in {:.1} ms",
        path.display(),
        image.width,
        image.height,
        t0.elapsed().as_secs_f64() * 1e3
    );
    Ok(Decoded {
        image,
        exif_orientation_applied: true,
        is_xtrans,
        flip,
    })
}

impl RawBackend for LibrawBackend {
    fn name(&self) -> &'static str {
        "libraw"
    }

    fn decode_preview(&self, path: &Path) -> Result<Decoded> {
        decode(path, true)
    }

    fn decode_full(&self, path: &Path) -> Result<Decoded> {
        decode(path, false)
    }

    /// The embedded camera JPEG (`unpack_thumb` → `make_mem_thumb`); a bitmap thumbnail (some
    /// older bodies) is reported as `None` rather than re-encoded.
    fn thumbnail(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let mut h = open(path)?;
        h.unpack_thumb()?;
        let t = h.make_mem_thumb()?;
        if t.kind() != ffi::LIBRAW_IMAGE_JPEG {
            log::debug!(
                "libraw: {} thumbnail type {} is not JPEG",
                path.display(),
                t.kind()
            );
            return Ok(None);
        }
        Ok(Some(t.data().to_vec()))
    }

    fn probe(&self, path: &Path) -> Result<RawInfoLite> {
        let h = open(path)?;
        let (width, height) = h.size();
        Ok(RawInfoLite {
            width,
            height,
            cfa: cfa_kind_from_filters(h.filters(), h.colors()),
            flip: h.flip(),
            make: h.make(),
            model: h.model(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn filters_classification() {
        assert_eq!(cfa_kind_from_filters(9, 3), CfaKind::XTrans);
        assert_eq!(cfa_kind_from_filters(0, 3), CfaKind::Linear);
        assert_eq!(cfa_kind_from_filters(0, 1), CfaKind::Mono);
        assert_eq!(cfa_kind_from_filters(1, 3), CfaKind::Other);
        assert_eq!(cfa_kind_from_filters(0x94949494, 3), CfaKind::Bayer);
        assert_eq!(cfa_kind_from_filters(0x16161616, 3), CfaKind::Bayer);
    }
}

//! Sample conversions at the decoder boundary: LibRaw's 16-bit output → unit floats, the
//! clip-to-`[0,1]` that LibRaw's `CLIP()` implies, and `lightcraft`'s `[f32; 3]` rows → the
//! interleaved [`RgbImage`] without a copy.

use negkit::RgbImage;

use crate::{NegRawError, Result};

/// `1 / 65535` as the Swift decoder multiplies it (`vDSP_vsmul` by a Float scale, not a divide).
pub const U16_SCALE: f32 = 1.0 / 65535.0;

/// `u16 → f32 × (1/65535)` into a caller-provided slice (every lane written; lengths must match,
/// extra lanes on either side are ignored).
pub fn u16_to_unit(src: &[u16], dst: &mut [f32]) {
    for (d, &s) in dst.iter_mut().zip(src) {
        // u16 → f32 is exact
        *d = s as f32 * U16_SCALE;
    }
}

/// Wrap little-/native-endian 16-bit interleaved RGB bytes (LibRaw's `output_bps=16` bitmap, host
/// byte order) as an image. Reads pairs unaligned, so the flexible array member's alignment is
/// irrelevant. Errors when `bytes` is shorter than `width × height × 3 × 2`.
pub fn rgb16_bytes_to_image(width: usize, height: usize, bytes: &[u8]) -> Result<RgbImage> {
    let lanes = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| NegRawError::UnsupportedOutput("image too large".into()))?;
    let need = lanes
        .checked_mul(2)
        .ok_or_else(|| NegRawError::UnsupportedOutput("image too large".into()))?;
    let src = bytes.get(..need).ok_or_else(|| {
        NegRawError::UnsupportedOutput(format!("{} bytes for {width}×{height}×3×u16", bytes.len()))
    })?;
    // `new_with` zero-allocates (untouched pages) and we write every lane once.
    Ok(RgbImage::new_with(width, height, |dst| {
        for (d, pair) in dst.iter_mut().zip(src.chunks_exact(2)) {
            let v = u16::from_ne_bytes([pair[0], pair[1]]);
            *d = v as f32 * U16_SCALE;
        }
    })?)
}

/// Clamp every lane to `[0, 1]` in place: LibRaw's `scale_colors` stores `CLIP(val)` into u16, so
/// black-subtracted negatives become 0 and anything over the white level becomes 1. Non-finite
/// lanes (a float DNG's NaN) become 0, which is what LibRaw's integer path would hold.
pub fn clamp_unit_in_place(data: &mut [f32]) {
    for v in data {
        *v = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

/// `lightcraft`'s row-major `Vec<[f32; 3]>` is already the interleaved layout: `into_flattened`
/// reuses the allocation (no copy), then the LibRaw clip is applied in place.
#[cfg(feature = "lcraw")]
pub fn rgb32f_into_image(img: lightcraft_raw::Rgb32f) -> Result<RgbImage> {
    let (w, h) = (img.width, img.height);
    let mut data = img.data.into_flattened();
    clamp_unit_in_place(&mut data);
    Ok(RgbImage::from_vec(w, h, data)?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn u16_scale_matches_swift_endpoints() {
        let src = [0u16, 1, 32768, 65535];
        let mut dst = [9.0f32; 4];
        u16_to_unit(&src, &mut dst);
        assert_eq!(dst[0], 0.0);
        assert_eq!(dst[3], 1.0);
        assert!((dst[1] - 1.0 / 65535.0).abs() < 1e-9);
        assert!((dst[2] - 32768.0 / 65535.0).abs() < 1e-6);
    }

    #[test]
    fn rgb16_bytes_round_trip_and_bounds() {
        let w = 2;
        let h = 1;
        let vals: [u16; 6] = [0, 65535, 1000, 2000, 3000, 40000];
        let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let img = rgb16_bytes_to_image(w, h, &bytes).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        for (lane, v) in img.data.iter().zip(vals) {
            assert!((lane - v as f32 / 65535.0).abs() < 1e-6);
        }
        // too short → error, never a panic
        assert!(rgb16_bytes_to_image(w, h, &bytes[..10]).is_err());
        // an extra trailing byte (odd length) is ignored
        let mut longer = bytes.clone();
        longer.push(7);
        assert!(rgb16_bytes_to_image(w, h, &longer).is_ok());
    }

    #[test]
    fn clamp_is_libraw_clip() {
        let mut v = [
            -0.5,
            0.0,
            0.25,
            1.0,
            1.5,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        clamp_unit_in_place(&mut v);
        assert_eq!(v, [0.0, 0.0, 0.25, 1.0, 1.0, 0.0, 0.0, 0.0]);
    }
}

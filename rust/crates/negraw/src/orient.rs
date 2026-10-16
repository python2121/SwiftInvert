//! Camera orientation as dcraw/LibRaw flip codes, baked into pixels.
//!
//! Mirrors `RGBImage.applyingFlip` in `Sources/NegativeKit/PlanarImage.swift`: the decoder runs with
//! `user_flip=0` (sensor-oriented output) and the recorded orientation is applied here, in code
//! (NegPy: `apply_exif_orientation`). This is a PRIVATE copy of the orientation op: `negkit` is
//! growing its own (`RgbImage::applying_flip`) concurrently and this crate must not depend on it.
//!
//! dcraw codes: bit 0 = mirror left↔right, bit 1 = mirror top↔bottom, bit 2 = transpose. The
//! camera-produced ones are `0` (none), `3` (180°), `5` (90° CCW) and `6` (90° CW); the mirror
//! codes `1`/`2`/`4`/`7` never come from a camera and — exactly like the Swift reference — are
//! treated as "no orientation". EXIF → dcraw is dcraw's own table: `flip = "50132467"[exif & 7]`.

use negkit::{NegError, RgbImage};

/// No orientation.
pub const FLIP_NONE: i32 = 0;
/// Rotate 180°.
pub const FLIP_180: i32 = 3;
/// Rotate 90° counter-clockwise (EXIF 8).
pub const FLIP_90_CCW: i32 = 5;
/// Rotate 90° clockwise (EXIF 6).
pub const FLIP_90_CW: i32 = 6;

/// dcraw's EXIF-orientation → flip table (`parse_tiff_ifd`, tag 274): `"50132467"[value & 7]`.
pub fn dcraw_flip_from_exif(exif_orientation: u16) -> i32 {
    const TABLE: [i32; 8] = [5, 0, 1, 3, 2, 4, 6, 7];
    // `& 7` keeps the index in range, as dcraw does (EXIF 8 lands on index 0 → 5).
    TABLE[(exif_orientation & 7) as usize]
}

/// Whether a flip code swaps width and height (the two quarter-turn codes this crate applies).
pub fn swaps_axes(flip: i32) -> bool {
    matches!(flip, FLIP_90_CCW | FLIP_90_CW)
}

/// Apply a dcraw flip code (`applyingFlip`): `3` = 180°, `5` = 90° CCW, `6` = 90° CW, anything
/// else = the image unchanged. Consumes the image so the no-op case is free.
pub fn applying_flip(img: RgbImage, flip: i32) -> Result<RgbImage, NegError> {
    let (w, h) = (img.width, img.height);
    let px = |x: usize, y: usize| img.pixel(x, y).unwrap_or([0.0; 3]);
    match flip {
        FLIP_180 => RgbImage::new_with(w, h, |out| {
            // out[y][x] = src[h-1-y][w-1-x]: every output row is a source row reversed pixel-wise
            for (y, row) in out.chunks_exact_mut(w * 3).enumerate() {
                let src = img.row(h - 1 - y).unwrap_or(&[]);
                for (dst, s) in row.chunks_exact_mut(3).zip(src.chunks_exact(3).rev()) {
                    dst.copy_from_slice(s);
                }
            }
        }),
        FLIP_90_CCW => RgbImage::new_with(h, w, |out| {
            // out[y][x] = src[x][w-1-y]: output row y is source column (w-1-y), top to bottom
            for (y, row) in out.chunks_exact_mut(h * 3).enumerate() {
                let sx = w - 1 - y;
                for (x, dst) in row.chunks_exact_mut(3).enumerate() {
                    dst.copy_from_slice(&px(sx, x));
                }
            }
        }),
        FLIP_90_CW => RgbImage::new_with(h, w, |out| {
            // out[y][x] = src[h-1-x][y]: output row y is source column y, bottom to top
            for (y, row) in out.chunks_exact_mut(h * 3).enumerate() {
                for (x, dst) in row.chunks_exact_mut(3).enumerate() {
                    dst.copy_from_slice(&px(y, h - 1 - x));
                }
            }
        }),
        _ => Ok(img),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    /// 3×2 asymmetric image: pixel (x, y) = [10x + y, 0, 0] (red lane carries the identity).
    fn probe() -> RgbImage {
        let mut data = Vec::new();
        for y in 0..2 {
            for x in 0..3 {
                data.extend_from_slice(&[(10 * x + y) as f32, 0.5, 0.25]);
            }
        }
        RgbImage::from_vec(3, 2, data).unwrap()
    }

    fn reds(img: &RgbImage) -> Vec<Vec<u32>> {
        (0..img.height)
            .map(|y| {
                (0..img.width)
                    .map(|x| img.pixel(x, y).unwrap()[0] as u32)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn flip_table_matches_swift_applying_flip() {
        // source rows: [0, 10, 20] / [1, 11, 21]
        let cases: [(i32, (usize, usize), Vec<Vec<u32>>); 6] = [
            (FLIP_NONE, (3, 2), vec![vec![0, 10, 20], vec![1, 11, 21]]),
            (FLIP_180, (3, 2), vec![vec![21, 11, 1], vec![20, 10, 0]]),
            // 90° CCW: the top row becomes the left column read bottom-up
            (
                FLIP_90_CCW,
                (2, 3),
                vec![vec![20, 21], vec![10, 11], vec![0, 1]],
            ),
            // 90° CW: the top row becomes the right column read top-down
            (
                FLIP_90_CW,
                (2, 3),
                vec![vec![1, 0], vec![11, 10], vec![21, 20]],
            ),
            (1, (3, 2), vec![vec![0, 10, 20], vec![1, 11, 21]]),
            (7, (3, 2), vec![vec![0, 10, 20], vec![1, 11, 21]]),
        ];
        for (flip, dims, want) in cases {
            let out = applying_flip(probe(), flip).unwrap();
            assert_eq!((out.width, out.height), dims, "flip {flip} dims");
            assert_eq!(reds(&out), want, "flip {flip}");
            // the other lanes ride along untouched
            assert!(
                out.data
                    .chunks_exact(3)
                    .all(|p| p[1] == 0.5 && p[2] == 0.25)
            );
        }
    }

    #[test]
    fn quarter_turns_are_inverses_and_compose_to_180() {
        let src = probe();
        let cw_then_ccw =
            applying_flip(applying_flip(src.clone(), FLIP_90_CW).unwrap(), FLIP_90_CCW).unwrap();
        assert_eq!(cw_then_ccw, src);
        let twice_cw =
            applying_flip(applying_flip(src.clone(), FLIP_90_CW).unwrap(), FLIP_90_CW).unwrap();
        assert_eq!(twice_cw, applying_flip(src, FLIP_180).unwrap());
    }

    #[test]
    fn exif_to_dcraw_table() {
        assert_eq!(dcraw_flip_from_exif(1), FLIP_NONE);
        assert_eq!(dcraw_flip_from_exif(3), FLIP_180);
        assert_eq!(dcraw_flip_from_exif(6), FLIP_90_CW);
        assert_eq!(dcraw_flip_from_exif(8), FLIP_90_CCW);
        assert_eq!(dcraw_flip_from_exif(2), 1);
        assert_eq!(dcraw_flip_from_exif(4), 2);
        assert_eq!(dcraw_flip_from_exif(5), 4);
        assert_eq!(dcraw_flip_from_exif(7), 7);
        assert_eq!(dcraw_flip_from_exif(0), 5); // dcraw's quirk, kept
        assert!(swaps_axes(FLIP_90_CW) && swaps_axes(FLIP_90_CCW));
        assert!(!swaps_axes(FLIP_180) && !swaps_axes(7));
    }
}

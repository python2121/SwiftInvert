//! The pure-Rust backend: `lightcraft-raw` expressing the LibRaw decode contract.
//!
//! How each LibRaw/rawpy parameter of `RawDecoder.swift` maps here (the parity ledger):
//!
//! | LibRaw parameter | lightcraft-raw equivalent | Match |
//! |---|---|---|
//! | `output_color=0` (sensor-native, no camera matrix) | `develop`/`normalized` never apply `ColorData` | exact |
//! | `user_mul=(1,1,1,1)`, `use_camera_wb=0`, `use_auto_wb=0` | no WB applied (`wb_multipliers` ignored) | exact |
//! | `gamm=(1,1)`, `output_bps=16` | float output, no TRC | exact (LibRaw quantizes to u16 steps, truncating: ≤1.5e-5) |
//! | black subtraction | `BlackLevel::at` (repeat pattern + per-row/col deltas), white scale `1/(white − mean black)` | LibRaw uses per-channel `cblack` + the same single `maximum`; same model |
//! | `no_auto_bright`, `adjust_maximum_thr=0` | nothing rescales; we clamp to `[0,1]` = LibRaw `CLIP` | exact |
//! | highlight mode 0 (clip) | no `highlight::reconstruct` call; clamp at 1.0 | exact |
//! | `half_size` + `user_qual=0` (preview) | `bin_mosaic` (ours, over `normalized()`): per-sample clip then per-colour block mean — R, B single, G = mean of 2 | exact up to u16 truncation (LibRaw's `(g1+g2)>>1`) |
//! | X-Trans preview = full demosaic (no `half_size`) | 3×3 bin (`PLAN` §3.6) | DIVERGES by design: 1/3 size, no interpolation |
//! | full = LibRaw default demosaic (AHD; Fuji: X-Trans-aware) | `develop(Method::Ahd)`; non-Bayer → `xtrans::directional` | same class, different code: NOT pixel-identical |
//! | `user_flip=0` + `applyingFlip(sizes.flip)` | `RawImage::orientation` (EXIF) → dcraw code → [`crate::orient::applying_flip`] | exact for 0/3/5/6; mirror codes ignored on both sides |
//! | output area = LibRaw's image area (masked borders off; NO DNG DefaultCrop) | active area (default); the file's default crop only with [`LcrawBackend::apply_default_crop`] | exact by default (6024×4020 on the R100; the crop would give 6000×4000) |
//! | white = `maximum` = `2^bits − 1` (`adjust_maximum_thr=0`) | [`WhiteReference::BitDepth`] overrides lightcraft's highlight-linearity `white` (default) | exact; `WhiteReference::File` keeps lightcraft's (1.43× brighter on the R100) |
//! | DNG opcodes: LibRaw (no DNG SDK) applies NONE | `OpcodeList1/2/3` CLEARED before developing unless [`LcrawBackend::apply_opcodes`] | exact by default — so GainMap vignetting, WarpRectilinear distortion, bad-pixel maps are NOT applied (lens corrections unwanted) |
//! | — | `decode` raises DNG samples stuck at the sensor's real saturation point to the white level (`lift_clipped_samples`, DNG/3FR only, not switchable) | DIVERGES only for clipped samples (they read 1.0 instead of ~0.9) |
//! | `pixel_aspect` (D1X) → `stretch()` | `develop` resizes rows (Mitchell) | same intent; preview falls back to the full path |
//! | thumbnail: `unpack_thumb` → JPEG | `embedded_preview` (largest JPEG; a DNG 1.7 JXL preview is dropped) | same source |
//!
//! Everything downstream (analysis, curves) is covered by the fixture suite from the linear
//! image on, so the residual decode differences are: demosaic algorithm at full size (export /
//! HQ tier), the default crop (dims), X-Trans preview resolution, and clipped DNG samples.

use std::path::Path;

use lightcraft_raw::{
    Method, OpcodeLists, RawError, RawImage, RawInfo, Rect, embedded_preview, probe_info,
};
use negkit::RgbImage;

use crate::convert::rgb32f_into_image;
use crate::orient::{applying_flip, dcraw_flip_from_exif};
use crate::{CfaKind, Decoded, NegRawError, RawBackend, RawInfoLite, Result, read_file};

/// Which white level the `[0, 1]` scale is anchored to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WhiteReference {
    /// LibRaw's `maximum` under `adjust_maximum_thr = 0`: the bit-depth ceiling `2^bits − 1`
    /// (16383 on a 14-bit body). NegPy/rawpy and the Swift app see this scale.
    #[default]
    BitDepth,
    /// The file's own white — lightcraft's highlight-linearity limit (11606 on the EOS R100),
    /// i.e. LibRaw's `linear_max`. ~1.43× brighter than [`Self::BitDepth`] on that body. A pure
    /// scale, which the log-density bounds cancel; only clipping differs.
    File,
}

/// The `lightcraft-raw` backend. `Default` = the LibRaw-parity settings: no opcodes, LibRaw's
/// image area (no default crop), bit-depth white. Measured 2026-10-15 on an EOS R100 CR3 against
/// LibRaw 0.22.1: with the file's white and default crop the previews were 3000×2000 vs
/// 3012×2010 and a uniform 1.44× brighter; this default closes both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LcrawBackend {
    /// Apply the DNG opcode lists (GainMap vignetting, WarpRectilinear distortion, FixBadPixels,
    /// MapTable…). LibRaw without the DNG SDK — rawpy's wheels, Homebrew, apt — applies none, and
    /// lens corrections are not wanted in a negative-conversion pipeline: default `false`.
    pub apply_opcodes: bool,
    /// Crop the developed frame to the file's default crop (DNG `DefaultCropOrigin/Size`, or the
    /// vendor crop lightcraft derives). LibRaw outputs the whole image area instead, which is what
    /// the Swift app and NegPy meter: default `false`.
    pub apply_default_crop: bool,
    /// See [`WhiteReference`]; default LibRaw's bit-depth maximum.
    pub white_reference: WhiteReference,
}

impl Default for LcrawBackend {
    fn default() -> Self {
        Self::libraw_area()
    }
}

impl LcrawBackend {
    /// LibRaw's exact output area and scale (no default crop, bit-depth white), opcodes off.
    pub fn libraw_area() -> Self {
        Self {
            apply_opcodes: false,
            apply_default_crop: false,
            white_reference: WhiteReference::BitDepth,
        }
    }

    /// lightcraft's own semantics: the file's default crop and white, opcodes off.
    pub fn file_native() -> Self {
        Self {
            apply_opcodes: false,
            apply_default_crop: true,
            white_reference: WhiteReference::File,
        }
    }

    /// Read + decode, then strip what the contract says LibRaw does not do.
    fn load(&self, path: &Path) -> Result<RawImage> {
        let bytes = read_file(path)?;
        let t0 = std::time::Instant::now();
        let mut raw = lightcraft_raw::decode(&bytes).map_err(|e| map_err(e, path))?;
        log::debug!(
            "lcraw: decoded {} ({}×{}, cpp {}, {:?}) in {:.1} ms",
            path.display(),
            raw.width,
            raw.height,
            raw.cpp,
            raw.format,
            t0.elapsed().as_secs_f64() * 1e3
        );
        if !self.apply_opcodes {
            raw.opcodes = OpcodeLists::default();
        }
        if !self.apply_default_crop {
            // a zero-size crop makes `develop_crop` fall back to the whole active area
            raw.crop = Rect::default();
        }
        if self.white_reference == WhiteReference::BitDepth && (1..=16).contains(&raw.bits) {
            // LibRaw's `maximum` (adjust_maximum_thr = 0): the bit-depth ceiling, not the
            // highlight-linearity limit lightcraft stores in `white`.
            let ceiling = ((1u32 << raw.bits) - 1) as f32;
            for w in &mut raw.white {
                *w = ceiling;
            }
        }
        Ok(raw)
    }

    fn finish(&self, image: RgbImage, raw: &RawImage, is_xtrans: bool) -> Result<Decoded> {
        let flip = dcraw_flip_from_exif(raw.orientation.to_exif());
        let image = applying_flip(image, flip)?;
        Ok(Decoded {
            image,
            exif_orientation_applied: true,
            is_xtrans,
            flip,
        })
    }
}

impl RawBackend for LcrawBackend {
    fn name(&self) -> &'static str {
        "lcraw"
    }

    fn decode_preview(&self, path: &Path) -> Result<Decoded> {
        let raw = self.load(path)?;
        let kind = cfa_kind(raw.cfa.as_ref(), raw.cpp);
        let k = match kind {
            CfaKind::Bayer => 2,
            CfaKind::XTrans => 3,
            _ => 0,
        };
        let t0 = std::time::Instant::now();
        let image = if k > 0 && raw.can_bin(k) {
            bin_mosaic(&raw, k)?
        } else {
            // linear / monochrome / non-square pixels (or an X-Trans that cannot bin): develop at
            // full size — the Swift X-Trans preview path — and halve linear/mono data like `half_size`
            let full = rgb32f_into_image(raw.develop(Method::Ahd).map_err(|e| map_err(e, path))?)?;
            if kind == CfaKind::XTrans {
                full
            } else {
                box2(&full)?
            }
        };
        log::debug!(
            "lcraw: preview {}×{} ({kind:?}, bin {k}) in {:.1} ms",
            image.width,
            image.height,
            t0.elapsed().as_secs_f64() * 1e3
        );
        self.finish(image, &raw, kind == CfaKind::XTrans)
    }

    fn decode_full(&self, path: &Path) -> Result<Decoded> {
        let raw = self.load(path)?;
        let kind = cfa_kind(raw.cfa.as_ref(), raw.cpp);
        let t0 = std::time::Instant::now();
        // AHD for Bayer; non-Bayer patterns take lightcraft's X-Trans directional interpolation
        let image = rgb32f_into_image(raw.develop(Method::Ahd).map_err(|e| map_err(e, path))?)?;
        log::debug!(
            "lcraw: full {}×{} ({kind:?}) in {:.1} ms",
            image.width,
            image.height,
            t0.elapsed().as_secs_f64() * 1e3
        );
        self.finish(image, &raw, kind == CfaKind::XTrans)
    }

    fn thumbnail(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let bytes = read_file(path)?;
        // the largest embedded preview; only a JPEG (SOI marker) is handed out — a DNG 1.7 JPEG XL
        // preview would need its own decoder on the consumer side
        Ok(embedded_preview(&bytes).filter(|p| p.starts_with(&[0xFF, 0xD8])))
    }

    fn probe(&self, path: &Path) -> Result<RawInfoLite> {
        let bytes = read_file(path)?;
        let info: RawInfo = probe_info(&bytes).map_err(|e| map_err(e, path))?;
        let (width, height) = if self.apply_default_crop {
            info.developed_size()
        } else {
            (
                info.active_area.width,
                lightcraft_raw::stretched_height(info.active_area.height, info.pixel_aspect),
            )
        };
        Ok(RawInfoLite {
            width,
            height,
            cfa: cfa_kind(info.cfa.as_ref(), info.cpp),
            flip: dcraw_flip_from_exif(info.orientation.to_exif()),
            make: info.metadata.make.clone(),
            model: info.metadata.model.clone(),
        })
    }
}

/// Classify lightcraft's CFA description.
fn cfa_kind(cfa: Option<&lightcraft_raw::Cfa>, cpp: usize) -> CfaKind {
    match cfa {
        None if cpp == 3 => CfaKind::Linear,
        None => CfaKind::Mono,
        Some(c) if c.is_bayer() => CfaKind::Bayer,
        Some(c) if c.width == 6 && c.height == 6 => CfaKind::XTrans,
        Some(_) => CfaKind::Other,
    }
}

fn map_err(e: RawError, path: &Path) -> NegRawError {
    match e {
        RawError::NotRaw => NegRawError::NotRaw(path.to_path_buf()),
        RawError::Unsupported(s) => NegRawError::Unsupported(s),
        RawError::Corrupt(s) => NegRawError::Corrupt(s),
        RawError::Limit(s) => NegRawError::Limit(s.to_string()),
        RawError::Tiff(t) => NegRawError::Corrupt(t.to_string()),
    }
}

/// LibRaw's `half_size` semantics on a CFA mosaic: every `k × k` block becomes one pixel whose
/// channels are the means of the block's samples of that colour, each sample clipped to `[0, 1]`
/// FIRST (LibRaw scales and `CLIP`s into u16 before `mix_green` averages the two greens). Bayer
/// `k=2`: R and B are single samples, G the mean of two. Runs on the active area after
/// `normalized()` (black/white per the file's model, opcodes as configured) and keeps only the
/// default crop region when the image carries one (zero-size crop = whole area), like
/// `develop_binned`. Written here rather than calling `develop_binned` because that one takes the
/// block MAXIMUM for a colour with a clipped sample (a highlight-reconstruction convenience) and
/// does not clip before averaging.
fn bin_mosaic(raw: &RawImage, k: usize) -> Result<RgbImage> {
    let n = raw
        .normalized()
        .map_err(|e| NegRawError::Corrupt(e.to_string()))?;
    let cfa = n
        .cfa
        .as_ref()
        .ok_or_else(|| NegRawError::Unsupported("binning a mosaic without a CFA".into()))?;
    if n.cpp != 1 || k == 0 {
        return Err(NegRawError::Unsupported(
            "binning needs single-sample CFA data".into(),
        ));
    }
    let crop = raw.develop_crop(n.width, n.height);
    let crop = if crop.width < k || crop.height < k {
        Rect::new(0, 0, n.width, n.height)
    } else {
        crop
    };
    let (bw, bh) = (crop.width / k, crop.height / k);
    if bw == 0 || bh == 0 {
        return Err(NegRawError::Unsupported(
            "image smaller than one bin block".into(),
        ));
    }
    // colour of each of the k×k samples for every pattern phase of a block's corner
    let (pw, ph) = (cfa.width.max(1), cfa.height.max(1));
    let layouts: Vec<Vec<usize>> = (0..ph)
        .flat_map(|j| (0..pw).map(move |i| (i, j)))
        .map(|(i, j)| {
            (0..k * k)
                .map(|t| usize::from(cfa.color_at(i + t % k, j + t / k).min(2)))
                .collect()
        })
        .collect();
    let empty: Vec<usize> = Vec::new();
    let wlen = bw * k;
    Ok(RgbImage::new_with(bw, bh, |out| {
        for (by, orow) in out.chunks_exact_mut(bw * 3).enumerate() {
            let y0 = crop.y + by * k;
            // the block row's k source rows, each `wlen` samples from the crop's left edge
            let rows: Vec<&[f32]> = (0..k)
                .map(|dy| {
                    let start = (y0 + dy) * n.width + crop.x;
                    n.data.get(start..start + wlen).unwrap_or(&[])
                })
                .collect();
            let py = y0 % ph;
            for (bx, dst) in orow.chunks_exact_mut(3).enumerate() {
                let layout = layouts
                    .get(py * pw + (crop.x + bx * k) % pw)
                    .unwrap_or(&empty);
                let mut sum = [0f32; 3];
                let mut cnt = [0u32; 3];
                for (dy, row) in rows.iter().enumerate() {
                    let block = row.get(bx * k..bx * k + k).unwrap_or(&[]);
                    for (dx, &v) in block.iter().enumerate() {
                        let c = layout.get(dy * k + dx).copied().unwrap_or(1).min(2);
                        let v = if v.is_finite() {
                            v.clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        sum[c] += v;
                        cnt[c] += 1;
                    }
                }
                for c in 0..3 {
                    dst[c] = if cnt[c] > 0 {
                        sum[c] / cnt[c] as f32
                    } else {
                        0.0
                    };
                }
            }
        }
    })?)
}

/// 2×2 box mean of an RGB image (the preview halving for data that has no CFA to bin; an odd
/// trailing row/column is dropped like `half_size` does with `iwidth = width >> 1`).
fn box2(img: &RgbImage) -> Result<RgbImage> {
    let (bw, bh) = (img.width / 2, img.height / 2);
    if bw == 0 || bh == 0 {
        return Err(NegRawError::Unsupported("image smaller than 2×2".into()));
    }
    Ok(RgbImage::new_with(bw, bh, |out| {
        for (by, orow) in out.chunks_exact_mut(bw * 3).enumerate() {
            let r0 = img.row(2 * by).unwrap_or(&[]);
            let r1 = img.row(2 * by + 1).unwrap_or(&[]);
            for (bx, dst) in orow.chunks_exact_mut(3).enumerate() {
                let s = bx * 6;
                let a = r0.get(s..s + 6).unwrap_or(&[]);
                let b = r1.get(s..s + 6).unwrap_or(&[]);
                let pick = |r: &[f32], i: usize| r.get(i).copied().unwrap_or(0.0);
                for (c, lane) in dst.iter_mut().enumerate() {
                    let v = (pick(a, c) + pick(a, c + 3) + pick(b, c) + pick(b, c + 3)) * 0.25;
                    *lane = if v.is_finite() {
                        v.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
            }
        }
    })?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use lightcraft_raw::{BlackLevel, Cfa, ColorData, Metadata, Orientation, RawData, RawFormat};

    /// A synthetic Bayer raw whose sample at (x, y) is `black + (10·x + y)` (so every sample is
    /// identifiable), white = black + 1000.
    fn synthetic(cfa: Cfa, w: usize, h: usize, crop: Rect) -> RawImage {
        let black = 100.0f32;
        let data = (0..h)
            .flat_map(|y| (0..w).map(move |x| (black as usize + 10 * x + y) as u16))
            .collect();
        RawImage {
            format: RawFormat::Dng,
            width: w,
            height: h,
            cpp: 1,
            data: RawData::U16(data),
            cfa: Some(cfa),
            bits: 12,
            black: BlackLevel::uniform(black),
            white: vec![black + 1000.0],
            active_area: Rect::new(0, 0, w, h),
            crop,
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: OpcodeLists::default(),
            metadata: Metadata::default(),
            lens_metadata: Default::default(),
        }
    }

    #[test]
    fn bayer_bin_is_libraw_half_size() {
        let raw = synthetic(Cfa::bayer("RGGB").unwrap(), 8, 6, Rect::default());
        let b = bin_mosaic(&raw, 2).unwrap();
        assert_eq!((b.width, b.height), (4, 3));
        // block (1, 2): samples (2,4)=R, (3,4)=G, (2,5)=G, (3,5)=B, normalized (10x+y)/1000
        let p = b.pixel(1, 2).unwrap();
        assert!((p[0] - 0.024).abs() < 1e-6, "{p:?}");
        assert!((p[1] - (0.034 + 0.025) / 2.0).abs() < 1e-6, "{p:?}");
        assert!((p[2] - 0.035).abs() < 1e-6, "{p:?}");
        // other phases: GBRG swaps roles
        let raw = synthetic(Cfa::bayer("GBRG").unwrap(), 8, 6, Rect::default());
        let p = bin_mosaic(&raw, 2).unwrap().pixel(0, 0).unwrap();
        assert!((p[0] - 0.001).abs() < 1e-6 && (p[2] - 0.010).abs() < 1e-6);
        assert!((p[1] - (0.0 + 0.011) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn bin_clips_each_sample_before_averaging_and_honours_crop() {
        let mut raw = synthetic(Cfa::bayer("RGGB").unwrap(), 8, 6, Rect::default());
        if let RawData::U16(d) = &mut raw.data {
            d[1] = 3000; // G1 of block (0,0) far over white; G2 = sample (0,1) = 100 + 1
        }
        let p = bin_mosaic(&raw, 2).unwrap().pixel(0, 0).unwrap();
        assert!((p[1] - (1.0 + 0.001) / 2.0).abs() < 1e-6, "{p:?}");
        // default crop at the binned scale: a 4×4 crop at (2, 2) → 2×2 output starting at sample (2,2)
        raw.crop = Rect::new(2, 2, 4, 4);
        let c = bin_mosaic(&raw, 2).unwrap();
        assert_eq!((c.width, c.height), (2, 2));
        assert!((c.pixel(0, 0).unwrap()[0] - 0.022).abs() < 1e-6);
        // and LibRaw's area when the crop is cleared
        raw.crop = Rect::default();
        assert_eq!(bin_mosaic(&raw, 2).unwrap().width, 4);
    }

    #[test]
    fn xtrans_bins_3x3_with_all_colours() {
        let raw = synthetic(Cfa::xtrans(), 12, 12, Rect::default());
        assert!(raw.can_bin(3) && !raw.can_bin(2));
        let b = bin_mosaic(&raw, 3).unwrap();
        assert_eq!((b.width, b.height), (4, 4));
        // every block of the 6×6 layout has 5 greens, 2 reds, 2 blues: all three channels populated
        for y in 0..4 {
            for x in 0..4 {
                let p = b.pixel(x, y).unwrap();
                assert!(
                    p.iter().all(|v| *v > 0.0 || (x, y) == (0, 0)),
                    "{p:?} at {x},{y}"
                );
            }
        }
        assert_eq!(cfa_kind(raw.cfa.as_ref(), 1), CfaKind::XTrans);
    }

    #[test]
    fn cfa_kinds() {
        assert_eq!(cfa_kind(None, 3), CfaKind::Linear);
        assert_eq!(cfa_kind(None, 1), CfaKind::Mono);
        assert_eq!(
            cfa_kind(Some(&Cfa::bayer("BGGR").unwrap()), 1),
            CfaKind::Bayer
        );
        let odd = Cfa {
            width: 4,
            height: 4,
            pattern: vec![0, 1, 1, 2, 0, 1, 1, 2, 0, 1, 1, 2, 0, 1, 1, 2],
        };
        assert_eq!(cfa_kind(Some(&odd), 1), CfaKind::Other);
    }

    #[test]
    fn box2_halves_with_means() {
        let img = RgbImage::from_vec(
            4,
            2,
            vec![
                0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.2, 0.4, 0.6, 0.2, 0.4, 0.6, //
                0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.2, 0.4, 0.6, 0.2, 0.4, 0.6,
            ],
        )
        .unwrap();
        let b = box2(&img).unwrap();
        assert_eq!((b.width, b.height), (2, 1));
        assert_eq!(b.pixel(0, 0).unwrap(), [0.5, 0.5, 0.5]);
        let p = b.pixel(1, 0).unwrap();
        assert!(
            (p[0] - 0.2).abs() < 1e-6 && (p[1] - 0.4).abs() < 1e-6 && (p[2] - 0.6).abs() < 1e-6
        );
    }

    #[test]
    fn develop_path_clamps_and_orients() {
        let mut raw = synthetic(Cfa::bayer("RGGB").unwrap(), 8, 6, Rect::default());
        raw.orientation = Orientation::Rotate90;
        let backend = LcrawBackend::default();
        let img = rgb32f_into_image(raw.develop(Method::Bilinear).unwrap()).unwrap();
        assert!(img.data.iter().all(|v| (0.0..=1.0).contains(v)));
        let d = backend.finish(img, &raw, false).unwrap();
        assert_eq!((d.image.width, d.image.height), (6, 8));
        assert_eq!(d.flip, crate::orient::FLIP_90_CW);
        assert!(d.exif_orientation_applied);
    }
}

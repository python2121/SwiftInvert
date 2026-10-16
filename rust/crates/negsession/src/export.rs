//! Export — `Sources/SwiftInvert/Exporter.swift` + `ExportSheet.swift` and the bridge's
//! `si_export_render` / `si_export_tiff` (session-spec §11).
//!
//! Pipeline: `Session::export_render` (full decode, proxy analysis, the sRGB leg IN-KERNEL when
//! sRGB is requested — the bridge/Vulkan order, `RenderOptions::srgb`) → resize on the ENCODED
//! output via `RgbImage::downsampled` (the Mac order: resize before quantization) → JPEG 8-bit
//! (quality 0.92 default) or baseline 16-bit TIFF, each ICC-TAGGED with the requested space's
//! profile so files are self-describing in every viewer. The file is encoded into a temp beside
//! the destination and renamed into place, so a failed encode or a full disk leaves a previous
//! export of the frame intact (ImageIO writes only at Finalize; review 7 finding 2).
//!
//! **Which ICC bytes are embedded.** craft-cms's built-in profiles, serialized by
//! `Profile::to_bytes()` (a fresh ICC v4 encoding): [`craft_cms::Builtin::Srgb`] ("sRGB
//! IEC61966-2.1": Rec. 709 primaries, D65, the sRGB transfer function) and
//! [`craft_cms::Builtin::AdobeRgbCompat`] ("Adobe RGB (1998)-compatible (Photocraft)": Adobe's
//! published primaries, D65, gamma 563/256 — the working space's own TRC, so the Adobe leg's
//! pixel values are tagged exactly as the chain encoded them). These replace the Swift negcli's
//! NegPy-sourced CC0 `sRGB-v4` / `AdobeCompat-v4` blobs (`Sources/negcli/ICCProfiles.swift`);
//! same colorimetry, different byte-level encoding. JPEG carries them as APP2 `ICC_PROFILE`
//! segments (the `image` crate's `JpegEncoder::set_icc_profile` chunks them per the ICC spec),
//! TIFF as tag 34675 (`InterColorProfile`). Not ported: the Mac's EXIF/GPS/Make/Model copy
//! (ImageIO-specific); the TIFF is uncompressed baseline (the Mac writes LZW).
//!
//! **Sticky options.** [`ExportOptions`] stores every sheet value independently, exactly like
//! the Swift struct — `jpegQuality` survives a switch to TIFF, `maxLongEdge` survives turning
//! resize off, `customDestinationPath` survives choosing next-to-source — and serializes with
//! the Swift keys (`colorSpace` still spelt `rommRGB` for the wide option) so the Mac's
//! `UserDefaults["exportOptions"]` blob and ours are interchangeable.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{ExtendedColorType, ImageEncoder};
use negkit::RgbImage;
use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::session::Session;
use crate::{Result, SessionError};

/// Default JPEG quality (`ExportOptions.jpegQuality`), 0…1.
pub const DEFAULT_JPEG_QUALITY: f64 = 0.92;
/// Default long edge when resizing is turned on (`ExportOptions.maxLongEdge`).
pub const DEFAULT_MAX_LONG_EDGE: u32 = 3000;
/// Resizes below this long edge are ignored (`Exporter.write`'s `maxLongEdge >= 16`).
pub const MIN_RESIZE_LONG_EDGE: u32 = 16;
/// The export sheet's long-edge range.
pub const LONG_EDGE_RANGE: (u32, u32) = (256, 20_000);
/// The export sheet's quality range.
pub const JPEG_QUALITY_RANGE: (f64, f64) = (0.5, 1.0);

/// Output container (`ExportFormat`; the JPEG quality lives on [`ExportOptions`], as in Swift).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Tiff16,
}

impl ExportFormat {
    /// File extension (`jpg` / `tiff`).
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 => "tiff",
        }
    }

    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Tiff16 => "TIFF (16-bit)",
        }
    }
}

/// Output colour space (`ExportColorSpace`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExportColorSpace {
    /// NegPy's default: correct everywhere, including profile-ignorant viewers.
    #[default]
    #[serde(rename = "sRGB", alias = "srgb")]
    Srgb,
    /// The Adobe RGB (1998) working space, tagged directly. The Swift case is still NAMED
    /// `rommRGB` (kept for sticky-JSON compatibility since the b3490eb working-space move).
    #[serde(rename = "rommRGB", alias = "adobe", alias = "adobeRGB")]
    AdobeRgb,
}

impl ExportColorSpace {
    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            ExportColorSpace::Srgb => "sRGB",
            ExportColorSpace::AdobeRgb => "Adobe RGB (wide gamut)",
        }
    }

    /// The craft-cms built-in whose bytes get embedded.
    pub fn builtin(self) -> craft_cms::Builtin {
        match self {
            ExportColorSpace::Srgb => craft_cms::Builtin::Srgb,
            ExportColorSpace::AdobeRgb => craft_cms::Builtin::AdobeRgbCompat,
        }
    }

    /// The ICC profile bytes embedded in exports of this space (see the module header).
    pub fn icc_profile(self) -> Arc<Vec<u8>> {
        self.builtin().profile().to_bytes()
    }
}

/// Where the file goes — a VIEW over [`ExportOptions`]' two destination fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExportDestination {
    /// Next to each source (the default).
    #[default]
    NextToSource,
    /// A chosen folder.
    Folder(PathBuf),
}

fn default_quality() -> f64 {
    DEFAULT_JPEG_QUALITY
}
fn default_long_edge() -> u32 {
    DEFAULT_MAX_LONG_EDGE
}

/// User-facing export options (the sheet) — field for field the Swift `ExportOptions`, with
/// its sticky-JSON keys. Defaults: JPEG · 0.92 · resize off / 3000 · sRGB · next to source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    #[serde(default)]
    pub format: ExportFormat,
    /// 0…1 (the slider shows a percentage); kept whatever `format` is.
    #[serde(default = "default_quality")]
    pub jpeg_quality: f64,
    /// Cap the long edge at `max_long_edge` after rendering.
    #[serde(default)]
    pub resize: bool,
    /// Remembered even while `resize` is off.
    #[serde(default = "default_long_edge")]
    pub max_long_edge: u32,
    #[serde(default)]
    pub color_space: ExportColorSpace,
    /// Write into `custom_destination_path` (when it is set) instead of next to the source.
    #[serde(default)]
    pub use_custom_destination: bool,
    /// Remembered even while `use_custom_destination` is off.
    #[serde(default)]
    pub custom_destination_path: Option<PathBuf>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            resize: false,
            max_long_edge: DEFAULT_MAX_LONG_EDGE,
            color_space: ExportColorSpace::Srgb,
            use_custom_destination: false,
            custom_destination_path: None,
        }
    }
}

impl ExportOptions {
    /// The effective resize: `Some(n)` when resizing is on.
    pub fn resize_long_edge(&self) -> Option<u32> {
        self.resize.then_some(self.max_long_edge)
    }

    /// The effective destination: the custom folder only when the flag is on AND a path is
    /// set (the persisted blob can hold the flag without a path — falls back to next-to-source).
    pub fn destination(&self) -> ExportDestination {
        match (&self.use_custom_destination, &self.custom_destination_path) {
            (true, Some(p)) => ExportDestination::Folder(p.clone()),
            _ => ExportDestination::NextToSource,
        }
    }

    /// Set the destination (a folder also becomes the remembered path).
    pub fn set_destination(&mut self, destination: ExportDestination) {
        match destination {
            ExportDestination::NextToSource => self.use_custom_destination = false,
            ExportDestination::Folder(p) => {
                self.use_custom_destination = true;
                self.custom_destination_path = Some(p);
            }
        }
    }

    /// Builder form of [`ExportOptions::set_destination`].
    pub fn with_destination(mut self, destination: ExportDestination) -> Self {
        self.set_destination(destination);
        self
    }

    /// Output path for one source (`destinationURL(for:)`): the chosen folder or the source's
    /// directory, the source's stem, the format's extension. Overwrites a previous export of the
    /// same frame by construction.
    pub fn destination_path(&self, source: &Path) -> PathBuf {
        let stem = source
            .file_stem()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        let dir = match self.destination() {
            ExportDestination::Folder(dir) => dir,
            ExportDestination::NextToSource => {
                source.parent().map(Path::to_path_buf).unwrap_or_default()
            }
        };
        let mut name = stem;
        name.push(".");
        name.push(self.format.extension());
        dir.join(name)
    }

    /// Whether the sheet's Export button is enabled: a custom destination needs a path, and
    /// the quality must be a number.
    pub fn is_valid(&self) -> bool {
        self.jpeg_quality.is_finite()
            && !(self.use_custom_destination && self.custom_destination_path.is_none())
    }

    /// The Swift sticky JSON for these options (`UserDefaults["exportOptions"]`).
    pub fn to_sticky_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Decode the Swift sticky JSON; `None` on a broken blob (callers fall back to defaults);
    /// missing keys take their defaults.
    pub fn from_sticky_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// Writes exports.
pub struct Exporter;

impl Exporter {
    /// Render `session`'s current settings at full resolution and write it per `options`,
    /// returning the file written. The sRGB leg runs in-kernel; resize follows on the encoded
    /// output.
    pub fn write(session: &mut Session, options: &ExportOptions) -> Result<PathBuf> {
        let dest = options.destination_path(session.path());
        let encoded = session.export_render(options.color_space == ExportColorSpace::Srgb)?;
        Self::write_encoded(&encoded, &dest, options)?;
        Ok(dest)
    }

    /// Write an already ENCODED buffer (in the options' colour space's own encoding) to `dest`:
    /// resize if asked, quantize, tag — into a temp beside `dest`, renamed into place only
    /// after a successful encode + fsync. Render-free, so tests and other front ends can drive it.
    pub fn write_encoded(encoded: &RgbImage, dest: &Path, options: &ExportOptions) -> Result<()> {
        let image = match options.resize_long_edge() {
            Some(n) if n >= MIN_RESIZE_LONG_EDGE => encoded.downsampled(n as usize),
            _ => std::borrow::Cow::Borrowed(encoded),
        };
        let (w, h) = dims_u32(&image)?;
        if !options.jpeg_quality.is_finite() {
            return Err(SessionError::Invalid("JPEG quality is not a number"));
        }
        let icc = options.color_space.icc_profile();
        atomic::write_atomic_with(dest, |file| {
            let mut out = BufWriter::new(file);
            let encode_err = |e: image::ImageError| std::io::Error::other(e.to_string());
            match options.format {
                ExportFormat::Jpeg => {
                    // 0…1 → the encoder's 1…100 integer scale (rounded).
                    let q = (options.jpeg_quality.clamp(0.01, 1.0) * 100.0).round() as u8;
                    let mut enc =
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q.max(1));
                    enc.set_icc_profile(icc.as_ref().clone())
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    enc.write_image(&rgb8_bytes(&image), w, h, ExtendedColorType::Rgb8)
                        .map_err(encode_err)?;
                }
                ExportFormat::Tiff16 => {
                    let mut enc = image::codecs::tiff::TiffEncoder::new(&mut out);
                    enc.set_icc_profile(icc.as_ref().clone())
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    enc.write_image(&rgb16_bytes(&image), w, h, ExtendedColorType::Rgb16)
                        .map_err(encode_err)?;
                }
            }
            out.flush()
        })
    }
}

fn dims_u32(image: &RgbImage) -> Result<(u32, u32)> {
    let w = u32::try_from(image.width).map_err(|_| SessionError::Invalid("image too wide"))?;
    let h = u32::try_from(image.height).map_err(|_| SessionError::Invalid("image too tall"))?;
    if w == 0 || h == 0 {
        return Err(SessionError::Invalid("empty image"));
    }
    Ok((w, h))
}

/// Encoded f32 → 8-bit (nearest), built straight into its allocation.
fn rgb8_bytes(image: &RgbImage) -> Vec<u8> {
    image
        .data
        .iter()
        // Clamped to [0, 255.5) before the truncating cast.
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect()
}

/// Encoded f32 → 16-bit native-endian bytes (what the TIFF encoder's `Rgb16` expects).
fn rgb16_bytes(image: &RgbImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.data.len() * 2);
    for &v in &image.data {
        // Clamped to [0, 65535.5) before the truncating cast.
        let q = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
        out.extend_from_slice(&q.to_ne_bytes());
    }
    out
}

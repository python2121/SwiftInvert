//! Export — `Sources/SwiftInvert/Exporter.swift` + `ExportSheet.swift` and the bridge's
//! `si_export_render` / `si_export_tiff` (session-spec §11).
//!
//! Pipeline: `Session::export_render` (full decode, proxy analysis, the sRGB leg IN-KERNEL when
//! sRGB is requested — the bridge/Vulkan order, `RenderOptions::srgb`) → resize on the ENCODED
//! output via `RgbImage::downsampled` (the Mac order: resize before quantization) → JPEG 8-bit
//! (quality 0.92 default) or baseline 16-bit TIFF, each ICC-TAGGED with the requested space's
//! profile so files are self-describing in every viewer.
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

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{ExtendedColorType, ImageEncoder};
use negkit::RgbImage;
use serde::{Deserialize, Serialize};

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

/// Output container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportFormat {
    /// 8-bit JPEG at `quality` (0…1; the slider shows it as a percentage).
    Jpeg { quality: f64 },
    /// 16-bit baseline TIFF.
    Tiff16,
}

impl Default for ExportFormat {
    fn default() -> Self {
        ExportFormat::Jpeg {
            quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

impl ExportFormat {
    /// File extension (`jpg` / `tiff`).
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg { .. } => "jpg",
            ExportFormat::Tiff16 => "tiff",
        }
    }

    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg { .. } => "JPEG",
            ExportFormat::Tiff16 => "TIFF (16-bit)",
        }
    }
}

/// Output colour space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExportColorSpace {
    /// NegPy's default: correct everywhere, including profile-ignorant viewers.
    #[default]
    Srgb,
    /// The Adobe RGB (1998) working space, tagged directly (the Swift case still NAMED `rommRGB`
    /// in sticky JSON — see [`ExportOptions::from_sticky_json`]).
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

/// Where the file goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExportDestination {
    /// Next to each source (the default).
    #[default]
    NextToSource,
    /// A chosen folder.
    Folder(PathBuf),
}

/// User-facing export options (the sheet). Defaults are deliberately high quality.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub colorspace: ExportColorSpace,
    /// `Some(n)` = cap the long edge at `n` after rendering; `None` = full size (the sheet's
    /// resize toggle defaults off with [`DEFAULT_MAX_LONG_EDGE`] in the field).
    pub resize_long_edge: Option<u32>,
    pub destination: ExportDestination,
}

/// The Swift sticky-options JSON (`UserDefaults["exportOptions"]`), kept readable both ways.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StickyOptions {
    #[serde(default = "sticky_format")]
    format: String,
    #[serde(default = "sticky_quality")]
    jpeg_quality: f64,
    #[serde(default)]
    resize: bool,
    #[serde(default = "sticky_long_edge")]
    max_long_edge: u32,
    #[serde(default = "sticky_colorspace")]
    color_space: String,
    #[serde(default)]
    use_custom_destination: bool,
    #[serde(default)]
    custom_destination_path: Option<String>,
}

fn sticky_format() -> String {
    "jpeg".to_owned()
}
fn sticky_quality() -> f64 {
    DEFAULT_JPEG_QUALITY
}
fn sticky_long_edge() -> u32 {
    DEFAULT_MAX_LONG_EDGE
}
fn sticky_colorspace() -> String {
    "sRGB".to_owned()
}

impl ExportOptions {
    /// Output path for one source (`destinationURL(for:)`): the chosen folder or the source's
    /// directory, the source's stem, the format's extension. Overwrites a previous export of the
    /// same frame by construction.
    pub fn destination_path(&self, source: &Path) -> PathBuf {
        let stem = source
            .file_stem()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        let dir = match &self.destination {
            ExportDestination::Folder(dir) => dir.clone(),
            ExportDestination::NextToSource => {
                source.parent().map(Path::to_path_buf).unwrap_or_default()
            }
        };
        let mut name = stem;
        name.push(".");
        name.push(self.format.extension());
        dir.join(name)
    }

    /// Whether the options can export (the sheet disables Export when a custom folder is chosen
    /// but empty — unrepresentable here, so this only rejects a non-finite quality).
    pub fn is_valid(&self) -> bool {
        match self.format {
            ExportFormat::Jpeg { quality } => quality.is_finite(),
            ExportFormat::Tiff16 => true,
        }
    }

    /// The Swift sticky JSON for these options.
    pub fn to_sticky_json(&self) -> Result<String> {
        let (format, jpeg_quality) = match self.format {
            ExportFormat::Jpeg { quality } => ("jpeg", quality),
            ExportFormat::Tiff16 => ("tiff16", DEFAULT_JPEG_QUALITY),
        };
        let (use_custom_destination, custom_destination_path) = match &self.destination {
            ExportDestination::NextToSource => (false, None),
            ExportDestination::Folder(dir) => (true, Some(dir.to_string_lossy().into_owned())),
        };
        let sticky = StickyOptions {
            format: format.to_owned(),
            jpeg_quality,
            resize: self.resize_long_edge.is_some(),
            max_long_edge: self.resize_long_edge.unwrap_or(DEFAULT_MAX_LONG_EDGE),
            color_space: match self.colorspace {
                ExportColorSpace::Srgb => "sRGB".to_owned(),
                ExportColorSpace::AdobeRgb => "rommRGB".to_owned(),
            },
            use_custom_destination,
            custom_destination_path,
        };
        Ok(serde_json::to_string(&sticky)?)
    }

    /// Decode the Swift sticky JSON; `None` on a broken blob (callers fall back to defaults).
    /// `rommRGB` (the case name kept for compatibility) and `adobe` both mean Adobe RGB; a custom
    /// destination flag without a path falls back to next-to-source.
    pub fn from_sticky_json(json: &str) -> Option<Self> {
        let s: StickyOptions = serde_json::from_str(json).ok()?;
        let format = match s.format.as_str() {
            "tiff16" | "tiff" => ExportFormat::Tiff16,
            _ => ExportFormat::Jpeg {
                quality: s.jpeg_quality,
            },
        };
        let colorspace = match s.color_space.as_str() {
            "rommRGB" | "adobe" | "adobeRGB" => ExportColorSpace::AdobeRgb,
            _ => ExportColorSpace::Srgb,
        };
        let destination = match (s.use_custom_destination, s.custom_destination_path) {
            (true, Some(p)) => ExportDestination::Folder(PathBuf::from(p)),
            _ => ExportDestination::NextToSource,
        };
        Some(Self {
            format,
            colorspace,
            resize_long_edge: s.resize.then_some(s.max_long_edge),
            destination,
        })
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
        let encoded = session.export_render(options.colorspace == ExportColorSpace::Srgb)?;
        Self::write_encoded(&encoded, &dest, options)?;
        Ok(dest)
    }

    /// Write an already ENCODED buffer (in the options' colour space's own encoding) to `dest`:
    /// resize if asked, quantize, tag. Render-free, so tests and other front ends can drive it.
    pub fn write_encoded(encoded: &RgbImage, dest: &Path, options: &ExportOptions) -> Result<()> {
        let image = match options.resize_long_edge {
            Some(n) if n >= MIN_RESIZE_LONG_EDGE => encoded.downsampled(n as usize),
            _ => std::borrow::Cow::Borrowed(encoded),
        };
        let (w, h) = dims_u32(&image)?;
        let icc = options.colorspace.icc_profile();
        let file = std::fs::File::create(dest).map_err(|e| SessionError::io(dest, e))?;
        let mut out = BufWriter::new(file);
        match options.format {
            ExportFormat::Jpeg { quality } => {
                // 0…1 → the encoder's 1…100 integer scale (rounded).
                let q = (quality.clamp(0.01, 1.0) * 100.0).round() as u8;
                let mut enc =
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q.max(1));
                enc.set_icc_profile(icc.as_ref().clone())
                    .map_err(|e| SessionError::Encode(e.to_string()))?;
                enc.write_image(&rgb8_bytes(&image), w, h, ExtendedColorType::Rgb8)
                    .map_err(|e| SessionError::Encode(e.to_string()))?;
            }
            ExportFormat::Tiff16 => {
                let mut enc = image::codecs::tiff::TiffEncoder::new(&mut out);
                enc.set_icc_profile(icc.as_ref().clone())
                    .map_err(|e| SessionError::Encode(e.to_string()))?;
                enc.write_image(&rgb16_bytes(&image), w, h, ExtendedColorType::Rgb16)
                    .map_err(|e| SessionError::Encode(e.to_string()))?;
            }
        }
        out.flush().map_err(|e| SessionError::io(dest, e))?;
        Ok(())
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

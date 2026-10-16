//! `negraw` — RAW files → [`negkit::RgbImage`] under the LibRaw contract SwiftInvert pins
//! (sensor-native camera RGB, unity white balance, no camera matrix, black-subtracted,
//! white-normalized to [0, 1], EXIF orientation baked). Owner: wave 1 / agent R. See `rust/PLAN.md` §3.6.
//!
//! Mirrors `Sources/RawDecodeKit/RawDecoder.swift` (NegPy's rawpy parameters, `preview_manager.py`):
//! `gamma=(1,1)`, `no_auto_bright`, `adjust_maximum_thr=0`, `output_bps=16`, `output_color=RAW`,
//! unity WB (`user_mul=(1,1,1,1)`, `use_camera_wb=0`, `use_auto_wb=0`), `user_flip=0` with the camera's
//! recorded orientation baked afterwards in code. Two quality tiers: **preview** = LibRaw
//! `half_size` + linear demosaic (one output pixel per 2×2 Bayer block: R, B and the mean of the two
//! greens; X-Trans skips `half_size`, which would alias the 6×6 CFA) and **full** = LibRaw's default
//! (AHD) demosaic at full resolution.
//!
//! Two backends behind [`RawBackend`]:
//! - [`lcraw`] (feature `lcraw`, default): pure-Rust `lightcraft-raw`, expressing the contract in its
//!   terms — see that module's header for exactly where it can and cannot match LibRaw.
//! - [`libraw`] (feature `libraw`, off by default): FFI to `libraw_r`, the byte-exact-vs-NegPy reference
//!   where the library exists (Mac, distrobox).
//!
//! The parity fixtures start at the linear image, so the rest of the pipeline is decode-independent.

#![cfg_attr(not(feature = "libraw"), forbid(unsafe_code))]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod convert;
#[cfg(feature = "lcraw")]
pub mod lcraw;
#[cfg(feature = "libraw")]
pub mod libraw;
pub mod orient;

use std::path::{Path, PathBuf};

use negkit::RgbImage;

#[cfg(not(any(feature = "lcraw", feature = "libraw")))]
compile_error!("negraw needs at least one backend feature: `lcraw` (default) or `libraw`");

/// Errors from RAW decoding, whichever backend.
#[derive(Debug, thiserror::Error)]
pub enum NegRawError {
    /// The file could not be read.
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The bytes are not a RAW container the backend recognises.
    #[error("not a recognised RAW file: {0}")]
    NotRaw(PathBuf),
    /// A recognised container whose variant the backend cannot decode.
    #[error("unsupported RAW: {0}")]
    Unsupported(String),
    /// Malformed data.
    #[error("corrupt RAW: {0}")]
    Corrupt(String),
    /// A decoder safety limit (header-driven allocation bound) was hit.
    #[error("decoder limit exceeded: {0}")]
    Limit(String),
    /// The decoded buffer could not be wrapped as an image (shape / size).
    #[error("image: {0}")]
    Image(#[from] negkit::NegError),
    /// A LibRaw call failed (`libraw_strerror` text and the raw code).
    #[error("LibRaw {stage} failed: {message} ({code})")]
    LibRaw {
        stage: &'static str,
        code: i32,
        message: String,
    },
    /// The decoder produced something other than 16-bit 3-channel linear RGB / a JPEG thumbnail.
    #[error("unsupported decoder output: {0}")]
    UnsupportedOutput(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, NegRawError>;

/// Colour-filter layout of a sensor, as far as the decode path cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CfaKind {
    /// 2×2 Bayer (any phase).
    Bayer,
    /// Fujifilm 6×6 X-Trans (LibRaw `idata.filters == 9`): no `half_size` preview.
    XTrans,
    /// Already-demosaiced linear RGB (LinearRaw DNG, Foveon-as-RGB).
    Linear,
    /// Single-plane monochrome (no CFA).
    Mono,
    /// A CFA the backend reports but this crate has no special path for.
    Other,
}

/// What [`RawBackend::probe`] learns from the headers alone (cheap: no sample decode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawInfoLite {
    /// Developed image size BEFORE the camera orientation is applied (active area / default crop,
    /// full resolution). See [`RawInfoLite::oriented_size`] for the on-screen dims.
    pub width: usize,
    pub height: usize,
    /// Sensor layout.
    pub cfa: CfaKind,
    /// dcraw/LibRaw flip code of the recorded orientation (see [`orient`]).
    pub flip: i32,
    /// Camera make / model when the headers carry them.
    pub make: Option<String>,
    pub model: Option<String>,
}

impl RawInfoLite {
    /// `true` for Fujifilm X-Trans sensors (the preview path must not bin 2×2).
    pub fn is_xtrans(&self) -> bool {
        self.cfa == CfaKind::XTrans
    }

    /// Size after the recorded orientation is baked (axes swap for the 90° codes).
    pub fn oriented_size(&self) -> (usize, usize) {
        if orient::swaps_axes(self.flip) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// A decoded frame: scene-linear sensor-native RGB in `[0, 1]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    /// Interleaved RGB, values in `[0, 1]` (1.0 = camera white level), camera orientation baked.
    pub image: RgbImage,
    /// Whether the camera's recorded orientation has been applied to `image` (always `true` for
    /// both backends today; kept explicit so a caller can never double-orient).
    pub exif_orientation_applied: bool,
    /// Fujifilm X-Trans sensor (the preview tier was NOT a 2×2 bin).
    pub is_xtrans: bool,
    /// The dcraw/LibRaw flip code that was baked (0 / 3 / 5 / 6; informational).
    pub flip: i32,
}

/// A RAW decoder. Every method re-opens the file (the Swift decoder does too): callers cache results.
pub trait RawBackend: Send + Sync {
    /// Short backend name for logs / `negcli` (`"lcraw"`, `"libraw"`).
    fn name(&self) -> &'static str;
    /// Preview tier: LibRaw `half_size` + linear demosaic semantics (native binned size — the
    /// session downsamples to its proxy); X-Trans without `half_size`.
    fn decode_preview(&self, path: &Path) -> Result<Decoded>;
    /// Full tier: full resolution, the backend's best (AHD-class) demosaic — export / HQ.
    fn decode_full(&self, path: &Path) -> Result<Decoded>;
    /// The embedded camera JPEG (encoded bytes, no conversion) for the library grid; `None` when
    /// the file carries no JPEG preview.
    fn thumbnail(&self, path: &Path) -> Result<Option<Vec<u8>>>;
    /// Header-only description.
    fn probe(&self, path: &Path) -> Result<RawInfoLite>;
}

/// The backend this build defaults to: `lcraw` when compiled in, else `libraw`.
pub fn default_backend() -> Box<dyn RawBackend> {
    #[cfg(feature = "lcraw")]
    {
        Box::new(lcraw::LcrawBackend::default())
    }
    #[cfg(all(not(feature = "lcraw"), feature = "libraw"))]
    {
        Box::new(libraw::LibrawBackend)
    }
}

/// Supported RAW file extensions (lowercase), from NegPy's `loaders/constants.py` minus TIFF/JPEG
/// (SwiftInvert is camera-RAW only). Mirrors `RawDecoder.rawExtensions`.
pub const RAW_EXTENSIONS: [&str; 26] = [
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "rw2", "orf", "pef",
    "srw", "erf", "kdc", "dcr", "mos", "mrw", "raw", "rwl", "iiq", "3fr", "fff", "x3f", "mef",
];

/// Whether `path` has a RAW extension (case-insensitive). Mirrors `RawDecoder.isRawFile`.
pub fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Read a whole file, mapping the error to [`NegRawError::Io`].
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| NegRawError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn raw_extension_check_is_case_insensitive_and_strict() {
        assert!(is_raw_file(Path::new("/x/IMG_0420.CR3")));
        assert!(is_raw_file(Path::new("a.arw")));
        assert!(is_raw_file(Path::new("a.DnG")));
        assert!(!is_raw_file(Path::new("a.tiff")));
        assert!(!is_raw_file(Path::new("a.jpg")));
        assert!(!is_raw_file(Path::new("noext")));
        assert_eq!(RAW_EXTENSIONS.len(), 26);
    }

    #[test]
    fn oriented_size_swaps_for_quarter_turns() {
        let mut info = RawInfoLite {
            width: 30,
            height: 20,
            cfa: CfaKind::Bayer,
            flip: orient::FLIP_NONE,
            make: None,
            model: None,
        };
        assert_eq!(info.oriented_size(), (30, 20));
        info.flip = orient::FLIP_90_CW;
        assert_eq!(info.oriented_size(), (20, 30));
        info.flip = orient::FLIP_180;
        assert_eq!(info.oriented_size(), (30, 20));
        assert!(!info.is_xtrans());
    }
}

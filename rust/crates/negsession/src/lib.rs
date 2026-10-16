#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negsession` — the platform-free engine behind every front end (L4 in `rust/PLAN.md` §1).
//!
//! The port of the Swift app's non-UI half: `ImageSession` (the per-image cache tower:
//! decode → orient → analyze → derive → render tiers — [`session`]), `SidecarStore`
//! ([`sidecar`]), `DefaultProfile` + `ProfileStore` ([`profiles`]), the edit-history model and
//! `HistoryLabels` ([`history`]), `Exporter` + `ExportOptions` ([`export`]), `AppModel`'s
//! library scan ([`library`]), the HQ tier decision ([`hq`]) and `DensitometerState`
//! ([`densitometer`]). Semantics, keys and numbers come from `rust/docs/session-spec.md`;
//! where that document flags Mac-vs-bridge differences this crate follows the Mac.
//!
//! Rendering goes through the [`renderer::Renderer`] trait: [`renderer::CpuRenderer`] runs
//! `negkit::reference` (the numerical oracle every GPU is pinned against) and
//! [`renderer::GpuRenderer`] runs `neggpu`'s wgpu chain; [`renderer::default_renderer`] picks
//! the GPU when one comes up. The darkroom math — Contrast Mask plane, crop geometry,
//! densitometry, zone grid, test strip, zone placement — is `negkit`'s; this crate wires it
//! to the cache tower ([`session`]) and names it for the front ends. Nothing here knows about
//! egui.

pub mod densitometer;
pub mod export;
pub mod geometry;
pub mod history;
pub mod hq;
pub mod library;
pub mod mask;
pub mod profiles;
pub mod renderer;
pub mod session;
pub mod sidecar;

pub use densitometer::{DensitometerState, ZoneGrid, zone_grid};
pub use export::{ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter};
pub use history::{History, HistoryEntry, history_label};
pub use hq::HqMode;
pub use library::{FolderNode, build_tree, flatten};
pub use mask::MaskPlane;
pub use profiles::{ProfileStore, SettingsGeometry, SettingsProfile};
pub use renderer::{CpuRenderer, GpuRenderer, Renderer, default_renderer};
pub use session::{
    Frame, OutputKind, RenderOutput, RenderRequest, RenderTier, Session, SessionInfo, SessionStats,
    TestStripOutput, TestStripRequest, ToolMode, ZonePinSample,
};
pub use sidecar::SidecarStore;
// FLAGGED (wave 4 / U1): the decoder seam `Session::open` takes, re-exported so a front end that
// reads only `negsession` can construct a session without a `negraw` dependency of its own.
pub use negraw::{RawBackend, default_backend, is_raw_file};

use std::path::PathBuf;

/// Errors from the engine. Pixel math is total; failures come from I/O, decoders, shapes and
/// encoders.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The RAW decoder failed.
    #[error(transparent)]
    Raw(#[from] negraw::NegRawError),
    /// A kernel entry point rejected its input (empty / degenerate image or region).
    #[error(transparent)]
    Kernel(#[from] negkit::NegError),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The renderer failed (a GPU renderer's device loss, for instance).
    #[error("render failed: {0}")]
    Render(String),
    /// The GPU chain refused or failed a render (device limits, out of memory, validation).
    #[error(transparent)]
    Gpu(neggpu::GpuError),
    /// An image encoder failed.
    #[error("encode failed: {0}")]
    Encode(String),
    /// JSON (sidecars, profiles, sticky options).
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A request that the session cannot satisfy (e.g. an export with no source frame).
    #[error("{0}")]
    Invalid(&'static str),
}

impl SessionError {
    /// Wrap an I/O error with the path it concerned.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, SessionError>;

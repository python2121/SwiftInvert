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
//! `negkit::reference` (the numerical oracle every GPU is pinned against); a GPU renderer
//! plugs in behind the same trait from the app layer, so nothing here knows about wgpu or
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

pub use export::{ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter};
pub use history::{History, HistoryEntry, history_label};
pub use hq::HqMode;
pub use library::{FolderNode, build_tree, flatten};
pub use mask::MaskPlane;
pub use profiles::{ProfileStore, SettingsGeometry, SettingsProfile};
pub use renderer::{CpuRenderer, Renderer};
pub use session::{
    Frame, OutputKind, RenderOutput, RenderRequest, RenderTier, Session, SessionInfo,
    SessionStats, ToolMode,
};
pub use sidecar::SidecarStore;

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

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negsession` — the platform-free engine behind every front end (L4 in `PLAN.md` §1).
//!
//! The port of the Swift app's non-UI half: `ImageSession` (the per-image cache tower:
//! decode → orient → analyze → derive → render tiers — [`session`]), `SidecarStore`
//! ([`sidecar`]), `DefaultProfile` + `ProfileStore` ([`profiles`]), the edit-history model and
//! `HistoryLabels` ([`history`]), `Exporter` + `ExportOptions` ([`export`]), `AppModel`'s
//! library scan ([`library`]), the HQ tier decision ([`hq`]) and `DensitometerState`
//! ([`densitometer`]). Semantics, keys and numbers come from `docs/session-spec.md`;
//! where that document flags Mac-vs-bridge differences this crate follows the Mac.
//!
//! Rendering goes through the [`renderer::Renderer`] trait: [`renderer::CpuRenderer`] runs
//! `negkit::reference` (the numerical oracle every GPU is pinned against) and
//! [`renderer::GpuRenderer`] runs `neggpu`'s wgpu chain; [`renderer::default_renderer`] picks
//! the GPU when one comes up. The darkroom math — Contrast Mask plane, crop geometry,
//! densitometry, zone grid, test strip, zone placement — is `negkit`'s; this crate wires it
//! to the cache tower ([`session`]) and names it for the front ends. Nothing here knows about
//! egui.
//!
//! # What a front end must do itself
//!
//! The engine is per-image state plus pure models; the timers and the cross-image policy that
//! `AppModel` owns on the Mac stay in the front end (session-spec §5, §6, §10):
//!
//! - **Session LRU of 2.** Keep the open session and the one navigated from
//!   (`AppModel.sessionLRU`); reopening a path reuses its session (move to back), the oldest
//!   beyond two is dropped. On every open call [`Session::release_enhanced_tiers`] on each
//!   retained session except the one being shown — only the on-screen frame may hold the
//!   full-resolution tier (hundreds of MB); the proxy tower (tens of MB) stays warm so arrowing
//!   back is instant. Clear the LRU when the library folder changes.
//! - **Render coalescing.** Latest-wins: one render in flight, a request during a render marks
//!   it pending and re-renders once with the newest snapshot. The snapshot is the baseline
//!   settings during a View-Original hold, the tool preview in a tool mode, `fine_rotation = 0`
//!   mid-straighten-drag; ask [`Session::needs_preparation_for`] about THAT snapshot to show
//!   "Analyzing…".
//! - **Sidecar saves** are debounced 1 s ([`sidecar::SidecarStore::save`] on a snapshot) and
//!   NEVER issued while restoring a frame's settings on open (`isRestoringSettings`) or in the
//!   profile-editor mode (one shared adjustments draft applies to every frame; no file is
//!   written) — the sidecar store cannot know either state. Flush the open frame's pending save
//!   before a batch export (outside the profile editor).
//! - **History** lives per path ([`History`] in a map keyed by the file), seeded at open,
//!   stashed/restored on switch; commits on the last drag release or 700 ms after a non-drag
//!   tick ([`history::PendingCommit`]), never while restoring or navigating history.
//! - **HQ timing**: the upward Auto swap is debounced 350 ms, the downward swap is instant;
//!   switching HQ off calls [`Session::release_full_tier`].
//!
//! # Deliberate divergences from the Swift app
//!
//! - **Lenient sidecars.** A sidecar field of the wrong type decodes to THAT field's default
//!   (negkit's per-field decoder); Swift's `decodeIfPresent` throws and the whole sidecar is
//!   treated as absent (the frame then gets the default profile). Ours keeps the other 53
//!   fields of a hand-edited file; a sidecar SwiftInvert wrote itself is identical either way.
//!   Only a non-object payload or a broken envelope is "absent".
//! - **Baseline (View Original) settings** ([`session::baseline_settings`]) keep the crop,
//!   rotation and flip, reset the fine rotation (both as Swift) AND keep the analysis region's
//!   drawn angle (Swift does not): the angle keys the meter image, so dropping it would re-meter
//!   on every hold-and-release; keeping it costs nothing and shows the same conversion the
//!   region was drawn for. Note the inherited quirk: a crop is normalised on the fine-rotated
//!   frame, so on a straightened frame the baseline's un-straightened crop covers a slightly
//!   different window — Swift behaves the same.
//! - **Medium tier survives `release_enhanced_tiers`** (Swift clears it): it is the native
//!   preview buffer the preview decode produced for free (≤ 20 MP), the only source
//!   `HqMode::Auto` draws on, and it cannot be rebuilt without a decode; dropping it would turn
//!   the next Auto swap into the full decode the mode promises never to trigger.
//! - **Tool previews take the caller's tier** like the Mac (HQ On ⇒ full tier uncropped in
//!   the tools; Auto is suppressed there by `HqMode::resolve`); the bridge/Qt forced the proxy.

pub mod atomic;
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

pub use densitometer::{DensitometerState, Encoding, ZoneGrid, zone_grid};
pub use export::{ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter};
pub use history::{History, HistoryEntry, history_label};
pub use hq::HqMode;
pub use library::{FolderNode, build_tree, flatten};
pub use mask::MaskPlane;
pub use profiles::{ProfileStore, SettingsGeometry, SettingsProfile};
pub use renderer::{CpuRenderer, GpuRenderer, Renderer, default_renderer};
pub use session::{
    Frame, OutputKind, RenderMeta, RenderOutput, RenderRequest, RenderTier, Session, SessionInfo,
    SessionStats, TestStripOutput, TestStripRequest, ToolMode, ZonePinSample,
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
    /// The active renderer has no such path (e.g. a texture handoff on the CPU renderer).
    #[error("unsupported by this renderer: {0}")]
    Unsupported(&'static str),
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

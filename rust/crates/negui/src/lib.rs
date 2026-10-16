#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negui` — the egui shell (L5, `rust/PLAN.md` §1): library tree, loupe, controls sidebar,
//! histogram, canvas control bar, menus, keyboard map, sidecar autosave and the history panel,
//! over the `negsession` engine. Specs: `rust/docs/controls-spec.md` and `session-spec.md`
//! §AppModel. Owner: wave 4 / agent U1 (tools, dialogs and the control channel follow in a
//! later wave — see the seams noted in [`app`], [`loupe`] and [`controls`]).

pub mod app;
pub mod control_spec;
pub mod controls;
pub mod engine;
pub mod histogram;
pub mod library;
pub mod loupe;
pub mod selftest;
pub mod theme;

pub use app::{AppOptions, AppPrefs, NegApp};
pub use loupe::{CpuPresenter, FramePresenter, PresentedFrame};

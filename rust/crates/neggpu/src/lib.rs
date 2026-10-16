#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `neggpu` — the GPU render chain: a wgpu compute twin of `negkit::reference` (the CPU
//! chain, which stays the numerical oracle). ONE WGSL module (`src/wgsl/negpipeline.wgsl`)
//! compiled by naga at runtime for Metal / Vulkan / DX12; no per-platform shaders, no
//! build-time compiler, no checked-in binaries. See `rust/PLAN.md` §2, §3.5, §3.7 and the
//! repository `CLAUDE.md` §"GPU render".
//!
//! Layering:
//! - [`context::GpuContext`] — instance / adapter / device / queue (headless, or shared with
//!   eframe through [`GpuContext::from_existing`]).
//! - [`uniforms`] — the pinned 48 / 304 / 16-byte uniform structs and the `RenderParams` →
//!   uniform packing (`ShaderTypes.swift`'s `UniformsBuilder`).
//! - [`pipeline::RenderPipeline`] — the six compute pipelines, per-size cached intermediates,
//!   uploads, and the float / rgba8 / texture render entry points.
//!
//! **CPU fallback.** This crate has no second implementation of the math: when
//! [`available`] is false (no adapter, or `NEG_GPU=off`), the session crate renders through
//! `negkit::reference::render`, which the GPU is pinned to at mean < 0.01 / max < 0.04
//! (`tests/parity.rs`). The two are interchangeable at the gates; the CPU is the oracle.

pub mod context;
pub mod pipeline;
pub mod uniforms;

use std::sync::OnceLock;

pub use context::GpuContext;
pub use pipeline::{
    DisplayResult, Histogram, MaskBuffer, RenderOptions, RenderPipeline, RenderResult, SourceBuffer,
};

/// Everything that can go wrong on the GPU side. Pixel math is total; errors come from the
/// environment (no adapter, a refused device), shapes and limits (a buffer past the device
/// maximum, a texture of the wrong size) and readback plumbing.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// `NEG_GPU_BACKEND` named something other than `vulkan` | `metal` | `dx12` | `off`.
    #[error("unknown NEG_GPU_BACKEND={0:?} (expected vulkan | metal | dx12 | off)")]
    BadBackend(String),
    /// The adapter was found but refused the device request.
    #[error("GPU device request failed: {0}")]
    Device(String),
    /// Shader compilation or pipeline creation failed validation.
    #[error("GPU pipeline validation failed: {0}")]
    Validation(String),
    /// A buffer would exceed what the device can allocate or bind.
    #[error("GPU buffer of {bytes} bytes exceeds the device limit of {limit} bytes")]
    TooLarge { bytes: u64, limit: u64 },
    /// Mapping a staging buffer for readback failed.
    #[error("GPU readback failed: {0}")]
    Readback(String),
    /// An image shape the kernels can't take.
    #[error(transparent)]
    Image(#[from] negkit::NegError),
    /// A caller-side contract violation (texture format/size, plane dims …).
    #[error("{0}")]
    Invalid(&'static str),
}

static SHARED: OnceLock<Result<Option<GpuContext>, String>> = OnceLock::new();

/// A process-wide headless context, created on first use (`None` when no GPU is usable).
/// Consumers that already own a device (the egui app) should use
/// [`GpuContext::from_existing`] instead; negcli and the tests share this one.
pub fn shared_headless() -> Option<GpuContext> {
    SHARED
        .get_or_init(|| GpuContext::new_headless().map_err(|e| e.to_string()))
        .as_ref()
        .ok()
        .and_then(|c| c.clone())
}

/// Whether a GPU render path is available in this process (probes once, see
/// [`shared_headless`]).
pub fn available() -> bool {
    shared_headless().is_some()
}

/// Why [`available`] is false — `None` when it is true.
pub fn unavailable_reason() -> Option<String> {
    match SHARED.get_or_init(|| GpuContext::new_headless().map_err(|e| e.to_string())) {
        Ok(Some(_)) => None,
        Ok(None) => Some(
            if std::env::var(context::ENV_GPU)
                .map(|v| v.eq_ignore_ascii_case("off"))
                .unwrap_or(false)
            {
                format!("{}=off", context::ENV_GPU)
            } else {
                format!(
                    "no {:?} adapter available",
                    context::backend_choice().ok().flatten()
                )
            },
        ),
        Err(e) => Some(e.clone()),
    }
}

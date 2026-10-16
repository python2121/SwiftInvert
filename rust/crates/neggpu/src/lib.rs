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
//! When the CPU fallback produces DISPLAY bytes it must quantize like the `encode_u8`
//! kernel's `pack4x8unorm` — round-to-nearest, [`pack_unorm8`] = `floor(255·clamp(x) + 0.5)`
//! — not `as u8` truncation, or densitometer / zone read-outs drift by 0.5/255 between the
//! CPU and GPU paths.
//!
//! **Locking.** A [`RenderPipeline`] serializes its renders with an internal mutex that is
//! held through `device.poll(Wait)` — a 24 MP export (~0.25 s) blocks every display render on
//! the SAME instance for that long. An app should own two instances on one device (one for
//! the interactive display, one for export); the device and queue are shared, the caches
//! are per instance.
//!
//! **Errors never abort the process.** Every context installs an uncaptured-error handler
//! (wgpu's default panics) that logs and records into [`GpuContext::take_last_error`], and
//! every upload/render runs under Validation + OutOfMemory error scopes, so a failed
//! allocation surfaces as [`GpuError::OutOfMemory`] / [`GpuError::Validation`]; sizes past
//! the device limits are refused up front as [`GpuError::TooLarge`] (see
//! [`context::required_limits`] for what the chain needs).

pub mod context;
pub mod pipeline;
pub mod uniforms;

use std::sync::OnceLock;

pub use context::{GpuContext, required_limits};
pub use pipeline::{
    CACHE_PIXEL_BUDGET, CacheStats, DisplayResult, Histogram, MaskBuffer, RenderOptions,
    RenderPipeline, RenderResult, SMALL_TIER_CAP, SourceBuffer,
};

/// The `encode_u8` kernel's quantization (`pack4x8unorm`): round-to-nearest on
/// `clamp(x, 0, 1) × 255`. The CPU display path must use this, not truncation.
#[inline]
pub fn pack_unorm8(x: f32) -> u8 {
    // Boundary cast: the value is already in [0, 255] and integral.
    (255.0 * x.clamp(0.0, 1.0) + 0.5).floor() as u8
}

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
    /// A buffer would exceed what the device can allocate or bind (refused before any GPU
    /// work; `limit_name` is the wgpu limit that binds — see [`required_limits`]).
    #[error(
        "GPU buffer of {bytes} bytes exceeds the device's {limit_name} of {limit} bytes (create the device with neggpu::required_limits)"
    )]
    TooLarge {
        bytes: u64,
        limit: u64,
        limit_name: &'static str,
    },
    /// The device ran out of memory inside an upload or render (caught by the error scope).
    #[error("GPU out of memory: {0}")]
    OutOfMemory(String),
    /// `render_display_to_texture` was handed a texture that is not `Rgba8Unorm`.
    #[error("display texture must be Rgba8Unorm (bytes are already display-encoded), got {0:?}")]
    TextureFormat(wgpu::TextureFormat),
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
                .map(|v| context::gpu_disabled_by(&v))
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

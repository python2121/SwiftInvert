//! `GpuContext` — the wgpu instance / adapter / device / queue the render pipeline runs on.
//!
//! Two ways in: [`GpuContext::new_headless`] for negcli, tests and any consumer without a
//! window (backend chosen per OS — Vulkan on Linux, Metal on macOS, DX12 on Windows, never GL —
//! or by `NEG_GPU_BACKEND`; `NEG_GPU=off` yields `None` so the session falls back to the CPU
//! chain), and [`GpuContext::from_existing`] to share eframe's device (`WgpuSetup::Existing`),
//! which is what makes the zero-copy display texture possible.

use std::sync::Arc;

use crate::GpuError;

/// Environment variable that disables the GPU path entirely (`NEG_GPU=off`).
pub const ENV_GPU: &str = "NEG_GPU";
/// Environment variable overriding the backend: `vulkan` | `metal` | `dx12` | `off`.
pub const ENV_BACKEND: &str = "NEG_GPU_BACKEND";

struct Inner {
    // Kept alive for the device's lifetime when we created them; `None` for a shared device.
    _instance: Option<wgpu::Instance>,
    _adapter: Option<wgpu::Adapter>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
    limits: wgpu::Limits,
}

/// A cheaply clonable handle on one GPU device (an `Arc` inside).
#[derive(Clone)]
pub struct GpuContext {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.inner.adapter_info.name)
            .field("backend", &self.inner.adapter_info.backend)
            .finish()
    }
}

/// The backend set the environment / OS selects: `Ok(None)` when the GPU is switched off.
pub fn backend_choice() -> Result<Option<wgpu::Backends>, GpuError> {
    if std::env::var(ENV_GPU)
        .map(|v| v.eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return Ok(None);
    }
    if let Ok(name) = std::env::var(ENV_BACKEND) {
        return match name.to_ascii_lowercase().as_str() {
            "off" => Ok(None),
            "vulkan" | "vk" => Ok(Some(wgpu::Backends::VULKAN)),
            "metal" | "mtl" => Ok(Some(wgpu::Backends::METAL)),
            "dx12" | "d3d12" => Ok(Some(wgpu::Backends::DX12)),
            _ => Err(GpuError::BadBackend(name)),
        };
    }
    Ok(Some(default_backends()))
}

/// The per-OS default backend (lightcraft's #136 lesson: never let wgpu enumerate drivers
/// you don't intend to use).
pub fn default_backends() -> wgpu::Backends {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::VULKAN
    }
}

/// The limits we ask for: wgpu's defaults with the buffer sizes raised to what the adapter
/// offers — a 24 MP float render is 288 MB per buffer, past the 256 MiB default.
fn requested_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let have = adapter.limits();
    let mut want = wgpu::Limits::default();
    want.max_buffer_size = have.max_buffer_size.max(want.max_buffer_size);
    want.max_storage_buffer_binding_size = have
        .max_storage_buffer_binding_size
        .max(want.max_storage_buffer_binding_size);
    want.max_compute_workgroups_per_dimension = have
        .max_compute_workgroups_per_dimension
        .max(want.max_compute_workgroups_per_dimension);
    want
}

impl GpuContext {
    /// Create a headless context on the OS-default (or `NEG_GPU_BACKEND`) backend. `Ok(None)`
    /// when the GPU is disabled (`NEG_GPU=off`) or no adapter is available — the caller falls
    /// back to the CPU chain. `Err` only for a malformed override or a device that was found
    /// but refused the request.
    pub fn new_headless() -> Result<Option<Self>, GpuError> {
        let Some(backends) = backend_choice()? else {
            return Ok(None);
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })) {
                Ok(a) => a,
                Err(e) => {
                    log::info!("neggpu: no adapter on {backends:?}: {e}");
                    return Ok(None);
                }
            };
        let adapter_info = adapter.get_info();
        let descriptor = |limits: wgpu::Limits| wgpu::DeviceDescriptor {
            label: Some("neggpu"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        };
        let requested =
            pollster::block_on(adapter.request_device(&descriptor(requested_limits(&adapter))));
        let (device, queue) = match requested {
            Ok(pair) => pair,
            Err(first) => {
                // A software or downlevel adapter may sit below wgpu's defaults; retry with
                // the downlevel set before giving up.
                log::info!("neggpu: default limits refused ({first}); retrying downlevel");
                pollster::block_on(
                    adapter.request_device(&descriptor(wgpu::Limits::downlevel_defaults())),
                )
                .map_err(|e| GpuError::Device(e.to_string()))?
            }
        };
        let limits = device.limits();
        log::info!(
            "neggpu: {} ({:?}, {:?}), max buffer {} MiB",
            adapter_info.name,
            adapter_info.backend,
            adapter_info.device_type,
            limits.max_buffer_size >> 20
        );
        Ok(Some(Self {
            inner: Arc::new(Inner {
                _instance: Some(instance),
                _adapter: Some(adapter),
                device,
                queue,
                adapter_info,
                limits,
            }),
        }))
    }

    /// Wrap a device the app already owns (eframe's `WgpuSetup::Existing`), so the render
    /// chain and the UI share one queue and the display frame never leaves the GPU.
    pub fn from_existing(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: wgpu::AdapterInfo,
    ) -> Self {
        let limits = device.limits();
        Self {
            inner: Arc::new(Inner {
                _instance: None,
                _adapter: None,
                device,
                queue,
                adapter_info,
                limits,
            }),
        }
    }

    /// The wgpu device.
    pub fn device(&self) -> &wgpu::Device {
        &self.inner.device
    }

    /// The wgpu queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.inner.queue
    }

    /// Adapter name / backend / vendor info.
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.inner.adapter_info
    }

    /// The device's effective limits (buffer-size checks in the pipeline read these).
    pub fn limits(&self) -> &wgpu::Limits {
        &self.inner.limits
    }

    /// `"Vulkan"` / `"Metal"` / `"Dx12"` — for status bars and bench output.
    pub fn backend_name(&self) -> String {
        format!("{:?}", self.inner.adapter_info.backend)
    }
}

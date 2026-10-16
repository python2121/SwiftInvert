//! `GpuContext` — the wgpu instance / adapter / device / queue the render pipeline runs on.
//!
//! Two ways in: [`GpuContext::new_headless`] for negcli, tests and any consumer without a
//! window (backend chosen per OS — Vulkan on Linux, Metal on macOS, DX12 on Windows, never GL —
//! or by `NEG_GPU_BACKEND`; `NEG_GPU=off` yields `None` so the session falls back to the CPU
//! chain), and [`GpuContext::from_existing`] to share eframe's device (`WgpuSetup::Existing`),
//! which is what makes the zero-copy display texture possible.

use std::sync::{Arc, Mutex, PoisonError};

use crate::GpuError;

/// The uncaptured-error slot every context installs on its device (wgpu's default handler
/// PANICS the process; ours logs and records, and the pipeline's error scopes turn errors
/// raised inside a render into `Err` instead).
type ErrorSlot = Arc<Mutex<Option<String>>>;

fn install_error_handler(device: &wgpu::Device) -> ErrorSlot {
    let slot: ErrorSlot = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&slot);
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let text = error.to_string();
        log::error!("neggpu: uncaptured wgpu error: {text}");
        *sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
    }));
    slot
}

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
    last_error: ErrorSlot,
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

/// Whether a `NEG_GPU` value switches the GPU off: `off`, `0`, `false`, `no`, `cpu`.
pub fn gpu_disabled_by(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "cpu"
    )
}

/// The backend set the environment / OS selects: `Ok(None)` when the GPU is switched off.
pub fn backend_choice() -> Result<Option<wgpu::Backends>, GpuError> {
    backend_choice_from(
        std::env::var(ENV_GPU).ok().as_deref(),
        std::env::var(ENV_BACKEND).ok().as_deref(),
    )
}

/// [`backend_choice`] on explicit values (`gpu` = `NEG_GPU`, `backend` = `NEG_GPU_BACKEND`).
pub fn backend_choice_from(
    gpu: Option<&str>,
    backend: Option<&str>,
) -> Result<Option<wgpu::Backends>, GpuError> {
    if gpu.is_some_and(gpu_disabled_by) {
        return Ok(None);
    }
    if let Some(name) = backend {
        return match name.trim().to_ascii_lowercase().as_str() {
            "off" | "cpu" => Ok(None),
            "vulkan" | "vk" => Ok(Some(wgpu::Backends::VULKAN)),
            "metal" | "mtl" => Ok(Some(wgpu::Backends::METAL)),
            "dx12" | "d3d12" => Ok(Some(wgpu::Backends::DX12)),
            _ => Err(GpuError::BadBackend(name.to_string())),
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

/// Raise `base`'s buffer/dispatch limits to what the adapter offers.
fn raise_buffer_limits(mut base: wgpu::Limits, have: &wgpu::Limits) -> wgpu::Limits {
    base.max_buffer_size = have.max_buffer_size.max(base.max_buffer_size);
    base.max_storage_buffer_binding_size = have
        .max_storage_buffer_binding_size
        .max(base.max_storage_buffer_binding_size);
    base.max_compute_workgroups_per_dimension = have
        .max_compute_workgroups_per_dimension
        .max(base.max_compute_workgroups_per_dimension);
    base
}

/// The device limits the render chain wants — what [`GpuContext::new_headless`] requests:
/// wgpu's defaults with `max_buffer_size`, `max_storage_buffer_binding_size` and
/// `max_compute_workgroups_per_dimension` raised to what the adapter offers. A 24 MP float
/// render is 288 MB per buffer, past the 256 MiB / 128 MiB defaults, so an app that shares
/// its device ([`GpuContext::from_existing`]) must request THESE limits when it creates the
/// device (eframe: `WgpuSetup::CreateNew { device_descriptor: … required_limits:
/// neggpu::required_limits(&adapter.limits()) }`), or the full display tier and full-size
/// export return [`GpuError::TooLarge`].
pub fn required_limits(adapter_limits: &wgpu::Limits) -> wgpu::Limits {
    raise_buffer_limits(wgpu::Limits::default(), adapter_limits)
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
        let have = adapter.limits();
        let requested =
            pollster::block_on(adapter.request_device(&descriptor(required_limits(&have))));
        let (device, queue) = match requested {
            Ok(pair) => pair,
            Err(first) => {
                // A software or downlevel adapter may sit below wgpu's defaults; retry with
                // the downlevel set (buffer limits still raised to the adapter's) before
                // giving up.
                log::info!("neggpu: default limits refused ({first}); retrying downlevel");
                pollster::block_on(adapter.request_device(&descriptor(raise_buffer_limits(
                    wgpu::Limits::downlevel_defaults(),
                    &have,
                ))))
                .map_err(|e| GpuError::Device(e.to_string()))?
            }
        };
        let last_error = install_error_handler(&device);
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
                last_error,
            }),
        }))
    }

    /// Wrap a device the app already owns (eframe's `WgpuSetup::Existing`), so the render
    /// chain and the UI share one queue and the display frame never leaves the GPU.
    ///
    /// Contract: the device must have been created with [`required_limits`] (or larger) —
    /// the actual `device.limits()` are recorded here and every upload/render/texture path
    /// checks against them, returning [`GpuError::TooLarge`] naming the binding limit when a
    /// buffer would exceed them. The uncaptured-error handler is installed on the device
    /// (replacing any the app set): errors log and land in [`GpuContext::take_last_error`].
    pub fn from_existing(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: wgpu::AdapterInfo,
    ) -> Self {
        let last_error = install_error_handler(&device);
        let limits = device.limits();
        log::info!(
            "neggpu: shared device {} — max buffer {} MiB, max storage binding {} MiB",
            adapter_info.name,
            limits.max_buffer_size >> 20,
            limits.max_storage_buffer_binding_size >> 20
        );
        Self {
            inner: Arc::new(Inner {
                _instance: None,
                _adapter: None,
                device,
                queue,
                adapter_info,
                limits,
                last_error,
            }),
        }
    }

    /// The most recent uncaptured wgpu error on this device (and clears it). Errors raised
    /// inside a render are returned as `Err` instead and never land here.
    pub fn take_last_error(&self) -> Option<String> {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Whether a `bytes`-sized storage buffer fits this device: `Err(TooLarge)` naming the
    /// binding limit (`max_buffer_size` or `max_storage_buffer_binding_size`) otherwise.
    pub fn check_buffer_size(&self, bytes: u64) -> Result<(), GpuError> {
        let limits = &self.inner.limits;
        if bytes > limits.max_buffer_size {
            return Err(GpuError::TooLarge {
                bytes,
                limit: limits.max_buffer_size,
                limit_name: "max_buffer_size",
            });
        }
        if bytes > limits.max_storage_buffer_binding_size {
            return Err(GpuError::TooLarge {
                bytes,
                limit: limits.max_storage_buffer_binding_size,
                limit_name: "max_storage_buffer_binding_size",
            });
        }
        Ok(())
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

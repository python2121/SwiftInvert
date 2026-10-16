//! The zero-copy loupe (wave 4 / U2b): display renders land in `Rgba8Unorm` wgpu textures that
//! egui-wgpu samples directly — no 4-byte-per-pixel readback, no `ColorImage`, no upload.
//!
//! **Where the render runs (the documented choice).** On the engine WORKER, like every other
//! render: the sessions (and their cache towers) live there, and a full-tier frame (~55 ms on
//! the NAVI33) must never stall the UI thread. This needs the engine's renderer to share the
//! window's `wgpu::Device` (`GpuRenderer::from_existing`, which the app already does when it
//! can); [`choose_presenter`] checks that and falls back to the [`CpuPresenter`] otherwise.
//!
//! **Hand-off — the slot guarantee.** The worker owns a [`TextureRing`] of [`RING`] textures
//! and a [`SlotPool`] of per-slot busy flags shared with the UI. A display render first ACQUIRES
//! a free slot (a [`SlotLease`]); the finished texture travels INSIDE its own `Evt::Rendered`
//! (as a [`GpuFrame`] carrying the lease), so the metadata and the pixels can never pair up
//! wrongly. [`GpuPresenter`] keeps the lease of the frame on screen; presenting the next frame
//! drops the previous lease, which frees that slot. A frame the UI discards (stale generation,
//! dropped channel) drops its lease too. The worker therefore can only ever write a slot that
//! is neither on screen nor in flight to the UI: with three slots that is the normal case
//! (one shown, one parked in the channel, one being written); when none is free the render
//! takes the readback path instead of waiting. GPU work is ordered on the one queue, so a slot
//! freed after egui last sampled it is rewritten only after that paint. A size change (tier
//! swap, crop, rotation) comes back once through `Session::render_into` (bytes the presenter
//! uploads like the CPU path) and the ring is recreated at the new size for the next render (a
//! leased old texture stays alive through the lease's own handle); a texture failure at the
//! size the readback then reports disables the ring for good (logged). Frames wider than the
//! device's `max_texture_dimension_2d` always take the readback path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui_wgpu;
use eframe::wgpu;
use negsession::RenderOutput;
use negsession::renderer::{GpuRenderer, Renderer, default_renderer};

use crate::loupe::{CpuPresenter, FramePresenter, PresentedFrame};

/// Textures in the ring: one on screen, one in flight to the UI, one being written.
pub const RING: usize = 3;
/// Environment override: `NEGUI_PRESENTER=cpu` forces the readback presenter (A/B timing).
pub const PRESENTER_ENV: &str = "NEGUI_PRESENTER";

/// Per-slot busy flags shared by the worker (acquire) and the UI (release by dropping the
/// lease). A slot is busy from the moment the worker picks it until the UI lets go of the frame
/// (replaced on screen, discarded, or never delivered).
#[derive(Debug, Default)]
pub struct SlotPool {
    busy: [AtomicBool; RING],
}

impl SlotPool {
    /// A fresh pool, every slot free.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Lease the first free slot; `None` when every slot is on screen or in flight.
    pub fn acquire(self: &Arc<Self>) -> Option<SlotLease> {
        self.busy.iter().enumerate().find_map(|(slot, flag)| {
            flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .ok()
                .map(|_| SlotLease {
                    pool: Arc::clone(self),
                    slot,
                })
        })
    }

    /// Whether `slot` is leased.
    pub fn is_busy(&self, slot: usize) -> bool {
        self.busy
            .get(slot)
            .is_some_and(|f| f.load(Ordering::Acquire))
    }
}

/// Exclusive use of one ring slot; dropping it frees the slot.
#[derive(Debug)]
pub struct SlotLease {
    pool: Arc<SlotPool>,
    slot: usize,
}

impl SlotLease {
    pub fn slot(&self) -> usize {
        self.slot
    }
}

impl Drop for SlotLease {
    fn drop(&mut self) {
        if let Some(f) = self.pool.busy.get(self.slot) {
            f.store(false, Ordering::Release);
        }
    }
}

/// A finished frame on its way to the presenter (inside its `Evt::Rendered`).
#[derive(Debug)]
pub struct GpuFrame {
    /// The slot it occupies; held until the UI lets go of the frame.
    pub lease: SlotLease,
    /// Bumped whenever the ring recreates its textures.
    pub epoch: u64,
    pub texture: wgpu::Texture,
}

impl GpuFrame {
    pub fn slot(&self) -> usize {
        self.lease.slot()
    }
}

/// The token that switches the engine to texture presentation (`Cmd::UseTextureTarget`): the
/// slot pool the worker's ring leases from.
#[derive(Clone, Debug)]
pub struct TextureSink {
    pool: Arc<SlotPool>,
}

impl Default for TextureSink {
    fn default() -> Self {
        Self {
            pool: SlotPool::new(),
        }
    }
}

impl TextureSink {
    /// The shared pool.
    pub fn pool(&self) -> &Arc<SlotPool> {
        &self.pool
    }
}

/// The worker's side: the textures and the device they live on.
pub struct TextureRing {
    sink: TextureSink,
    device: wgpu::Device,
    slots: [Option<wgpu::Texture>; RING],
    epoch: u64,
    size: Option<(u32, u32)>,
    failed_at: Option<(u32, u32)>,
    usable: bool,
}

impl TextureRing {
    /// A ring on `renderer`'s device; `None` unless it is the GPU renderer.
    pub fn new(renderer: &Arc<dyn Renderer>, sink: TextureSink) -> Option<Self> {
        let gpu = renderer.as_any()?.downcast_ref::<GpuRenderer>()?;
        Some(Self {
            sink,
            device: gpu.context().device().clone(),
            slots: [None, None, None],
            epoch: 0,
            size: None,
            failed_at: None,
            usable: true,
        })
    }

    /// Lease a free slot (never one on screen or in flight to the UI) and its texture; `None`
    /// when every slot is busy, before the first size is known, or when the frame is too large
    /// for a texture — the caller then takes the readback path.
    pub fn pick(&self) -> Option<(SlotLease, wgpu::Texture)> {
        let lease = self.sink.pool().acquire()?;
        let texture = self.slots.get(lease.slot()).and_then(|t| t.clone())?;
        Some((lease, texture))
    }

    /// Wrap the texture just rendered into `lease`'s slot for its `Evt::Rendered`.
    pub fn frame(&self, lease: SlotLease, texture: wgpu::Texture) -> GpuFrame {
        GpuFrame {
            lease,
            epoch: self.epoch,
            texture,
        }
    }

    /// A texture render failed at `tried`; [`TextureRing::resize`] decides whether the size was
    /// the reason.
    pub fn note_failure(&mut self, tried: (u32, u32), error: &str) {
        log::debug!("presenter: texture render failed at {tried:?}: {error}");
        self.failed_at = Some(tried);
    }

    /// The frame is `width × height`: recreate the ring when that changed. A failure at the
    /// very size the readback reports was not a size problem — the ring is disabled.
    pub fn resize(&mut self, width: usize, height: usize) {
        let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
            return;
        };
        if self.failed_at.take() == Some((w, h)) {
            log::warn!("presenter: GPU texture path failed at its own size; using readbacks");
            self.usable = false;
            return;
        }
        if self.size == Some((w, h)) {
            return;
        }
        self.size = Some((w, h));
        self.epoch += 1;
        let max = self.device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > max || h > max {
            self.slots = [None, None, None];
            return;
        }
        for slot in &mut self.slots {
            *slot = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("negui-loupe"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }));
        }
    }

    /// Whether the ring should stay in use.
    pub fn is_usable(&self) -> bool {
        self.usable
    }
}

/// The UI's side: egui texture ids per slot, plus the CPU presenter for readback frames.
pub struct GpuPresenter {
    render_state: egui_wgpu::RenderState,
    /// Per slot: (epoch registered, egui id).
    ids: [Option<(u64, egui::TextureId)>; RING],
    /// The lease of the frame on screen (dropping it frees the slot for the worker).
    displayed: Option<SlotLease>,
    cpu: CpuPresenter,
}

impl GpuPresenter {
    pub fn new(render_state: egui_wgpu::RenderState) -> Self {
        Self {
            render_state,
            ids: [None, None, None],
            displayed: None,
            cpu: CpuPresenter::default(),
        }
    }

    fn texture_id(&mut self, frame: &GpuFrame) -> Option<egui::TextureId> {
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let slot = self.ids.get_mut(frame.slot())?;
        let mut renderer = self.render_state.renderer.write();
        let id = match *slot {
            Some((epoch, id)) if epoch == frame.epoch => id,
            Some((_, id)) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    &self.render_state.device,
                    &view,
                    wgpu::FilterMode::Linear,
                    id,
                );
                id
            }
            None => renderer.register_native_texture(
                &self.render_state.device,
                &view,
                wgpu::FilterMode::Linear,
            ),
        };
        *slot = Some((frame.epoch, id));
        Some(id)
    }
}

impl FramePresenter for GpuPresenter {
    fn present(&mut self, ctx: &egui::Context, output: &RenderOutput) -> Option<PresentedFrame> {
        self.present_frame(ctx, output, None)
    }

    fn present_frame(
        &mut self,
        ctx: &egui::Context,
        output: &RenderOutput,
        gpu: Option<GpuFrame>,
    ) -> Option<PresentedFrame> {
        if output.frame.rgba8().is_some_and(|b| !b.is_empty()) {
            self.displayed = None;
            return self.cpu.present(ctx, output);
        }
        let frame = gpu?;
        let size = (
            frame.texture.width() as usize,
            frame.texture.height() as usize,
        );
        if size != (output.width, output.height) {
            return None;
        }
        let id = self.texture_id(&frame)?;
        // The previous frame's lease drops here: its slot is free for the worker again.
        self.displayed = Some(frame.lease);
        Some(PresentedFrame {
            texture: id,
            size: [size.0, size.1],
        })
    }

    fn clear(&mut self) {
        self.cpu.clear();
        self.displayed = None;
    }
}

/// The presenter for this window: the zero-copy one (plus the sink the engine needs) when the
/// engine's renderer is a [`GpuRenderer`] on the window's own device and `NEGUI_PRESENTER` isn't
/// `cpu`; else the readback presenter.
pub fn choose_presenter(
    render_state: Option<&egui_wgpu::RenderState>,
    renderer: &Arc<dyn Renderer>,
) -> (Box<dyn FramePresenter>, Option<TextureSink>) {
    let forced_cpu = std::env::var(PRESENTER_ENV)
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("cpu"));
    let shared = render_state.filter(|rs| {
        renderer
            .as_any()
            .and_then(|a| a.downcast_ref::<GpuRenderer>())
            .is_some_and(|g| g.context().device() == &rs.device)
    });
    match shared {
        Some(rs) if !forced_cpu => {
            let sink = TextureSink::default();
            log::info!("presenter: zero-copy GPU textures");
            (Box::new(GpuPresenter::new(rs.clone())), Some(sink))
        }
        _ => {
            log::info!("presenter: CPU readback");
            (Box::new(CpuPresenter::default()), None)
        }
    }
}

/// The engine's renderer: eframe's wgpu device shared (what makes the zero-copy loupe possible)
/// when there is one and `NEG_GPU` isn't `off`, else the process default (headless GPU or CPU).
/// The one place outside `main.rs` that touches the window's device.
pub fn shared_or_default_renderer(
    render_state: Option<&egui_wgpu::RenderState>,
) -> Arc<dyn Renderer> {
    if std::env::var("NEG_GPU")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("off"))
    {
        return default_renderer();
    }
    if let Some(rs) = render_state {
        match GpuRenderer::from_existing(rs.device.clone(), rs.queue.clone(), rs.adapter.get_info())
        {
            Ok(gpu) => {
                log::info!("renderer: {} (sharing the window's device)", gpu.name());
                return Arc::new(gpu);
            }
            Err(e) => log::warn!("renderer: shared device refused ({e}); falling back"),
        }
    }
    default_renderer()
}

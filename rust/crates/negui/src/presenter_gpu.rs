//! The zero-copy loupe (wave 4 / U2b): display renders land in `Rgba8Unorm` wgpu textures that
//! egui-wgpu samples directly — no 4-byte-per-pixel readback, no `ColorImage`, no upload.
//!
//! **Where the render runs (the documented choice).** On the engine WORKER, like every other
//! render: the sessions (and their cache towers) live there, and a full-tier frame (~55 ms on
//! the NAVI33) must never stall the UI thread. This needs the engine's renderer to share the
//! window's `wgpu::Device` (`GpuRenderer::from_existing`, which the app already does when it
//! can); [`choose_presenter`] checks that and falls back to the [`CpuPresenter`] otherwise.
//!
//! **Hand-off.** The worker owns a [`TextureRing`] of [`RING`] textures. Each display render
//! goes into the slot that is NOT on screen ([`TextureSink::displayed`]), the finished texture
//! is parked in the sink's inbox, and the usual `Evt::Rendered` follows carrying the metadata
//! with an EMPTY RGBA8 frame. [`GpuPresenter::present`] takes the parked texture (it must match
//! the event's size — a newer frame of another size is put back for its own event), registers
//! it with egui-wgpu once per slot and epoch (`register_native_texture`, then
//! `update_egui_texture_from_wgpu_texture` when a resize recreated the slot) and marks the slot
//! displayed. GPU work is ordered on the one queue, so even a slot rewritten while a stale event
//! is in flight only ever shows a complete frame. A size change (tier swap, crop, rotation)
//! comes back once through `Session::render_into` (bytes the presenter uploads like the CPU
//! path) and the ring is recreated at the new size for the next render; a texture failure at
//! the size the readback then reports disables the ring for good (logged). Frames wider than
//! the device's `max_texture_dimension_2d` always take the readback path.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui_wgpu;
use eframe::wgpu;
use negsession::RenderOutput;
use negsession::renderer::{GpuRenderer, Renderer};

use crate::loupe::{CpuPresenter, FramePresenter, PresentedFrame};

/// Textures in the ring (front + back).
pub const RING: usize = 2;
/// "No slot on screen".
const NO_SLOT: usize = usize::MAX;
/// Environment override: `NEGUI_PRESENTER=cpu` forces the readback presenter (A/B timing).
pub const PRESENTER_ENV: &str = "NEGUI_PRESENTER";

/// A finished frame parked for the presenter.
#[derive(Clone, Debug)]
pub struct GpuFrame {
    pub slot: usize,
    /// Bumped whenever the ring recreates its textures.
    pub epoch: u64,
    pub texture: wgpu::Texture,
}

/// The worker ↔ presenter mailbox.
#[derive(Clone, Debug)]
pub struct TextureSink {
    inbox: Arc<Mutex<Option<GpuFrame>>>,
    displayed: Arc<AtomicUsize>,
}

impl Default for TextureSink {
    fn default() -> Self {
        Self {
            inbox: Arc::new(Mutex::new(None)),
            displayed: Arc::new(AtomicUsize::new(NO_SLOT)),
        }
    }
}

impl TextureSink {
    /// The slot on screen (`None` while a CPU-uploaded frame is).
    pub fn displayed(&self) -> Option<usize> {
        match self.displayed.load(Ordering::Acquire) {
            NO_SLOT => None,
            s => Some(s),
        }
    }

    fn set_displayed(&self, slot: Option<usize>) {
        self.displayed
            .store(slot.unwrap_or(NO_SLOT), Ordering::Release);
    }

    fn park(&self, frame: GpuFrame) {
        if let Ok(mut inbox) = self.inbox.lock() {
            *inbox = Some(frame);
        }
    }

    fn take(&self) -> Option<GpuFrame> {
        self.inbox.lock().ok().and_then(|mut i| i.take())
    }

    fn put_back(&self, frame: GpuFrame) {
        if let Ok(mut inbox) = self.inbox.lock()
            && inbox.is_none()
        {
            *inbox = Some(frame);
        }
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
            slots: [None, None],
            epoch: 0,
            size: None,
            failed_at: None,
            usable: true,
        })
    }

    /// The slot to render into (never the one on screen) and its texture; `None` before the
    /// first size is known or when the frame is too large for a texture.
    pub fn pick(&self) -> Option<(usize, wgpu::Texture)> {
        let slot = match self.sink.displayed() {
            Some(d) => (d + 1) % RING,
            None => 0,
        };
        self.slots
            .get(slot)
            .and_then(|t| t.clone())
            .map(|t| (slot, t))
    }

    /// Park the texture just rendered for the presenter.
    pub fn deliver(&self, slot: usize, texture: &wgpu::Texture) {
        self.sink.park(GpuFrame {
            slot,
            epoch: self.epoch,
            texture: texture.clone(),
        });
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
            self.slots = [None, None];
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
    sink: TextureSink,
    /// Per slot: (epoch registered, egui id).
    ids: [Option<(u64, egui::TextureId)>; RING],
    cpu: CpuPresenter,
}

impl GpuPresenter {
    pub fn new(render_state: egui_wgpu::RenderState, sink: TextureSink) -> Self {
        Self {
            render_state,
            sink,
            ids: [None, None],
            cpu: CpuPresenter::default(),
        }
    }

    fn texture_id(&mut self, frame: &GpuFrame) -> Option<egui::TextureId> {
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let slot = self.ids.get_mut(frame.slot)?;
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
        if output.frame.rgba8().is_some_and(|b| !b.is_empty()) {
            self.sink.set_displayed(None);
            return self.cpu.present(ctx, output);
        }
        let frame = self.sink.take()?;
        let size = (
            frame.texture.width() as usize,
            frame.texture.height() as usize,
        );
        if size != (output.width, output.height) {
            // A newer frame of another size: it belongs to the next event.
            self.sink.put_back(frame);
            return None;
        }
        let id = self.texture_id(&frame)?;
        self.sink.set_displayed(Some(frame.slot));
        Some(PresentedFrame {
            texture: id,
            size: [size.0, size.1],
        })
    }

    fn clear(&mut self) {
        self.cpu.clear();
        self.sink.set_displayed(None);
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
            (
                Box::new(GpuPresenter::new(rs.clone(), sink.clone())),
                Some(sink),
            )
        }
        _ => {
            log::info!("presenter: CPU readback");
            (Box::new(CpuPresenter::default()), None)
        }
    }
}

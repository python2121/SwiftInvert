//! The render seam — `RenderPipeline.render` / `renderDisplay` in
//! `Sources/MetalRenderKit/RenderPipeline.swift` as a trait, with the CPU reference chain as the
//! oracle and the wgpu chain behind the same trait (PLAN §2 "CPU fallback is the oracle, not a
//! second implementation").
//!
//! The session hands a renderer an uploaded-once [`RenderSource`] (the linear, oriented,
//! cropped frame — slider ticks reuse it) plus the derived [`negkit::RenderParams`] and gets
//! back either the ENCODED float image ([`Renderer::render`]: export, tests, parity) or packed
//! RGBA8 ([`Renderer::render_display`]: the canvas) with the 4 × 256 histogram. `RenderSource::id`
//! is unique per uploaded buffer within the process, so [`GpuRenderer`] keys its device-side
//! copies on it and never re-uploads for a slider change — the Metal/Vulkan "upload once per
//! (image, crop)" discipline expressed without the renderer knowing the session's cache keys;
//! the session tells it when a source is gone ([`Renderer::forget_source`]).
//!
//! **Which renderer.** [`default_renderer`] picks the GPU when a headless `GpuContext` comes
//! up (and `NEG_GPU` isn't off), else the CPU; both are pinned to each other by
//! `tests/gpu_parity.rs` at the project gates (mean < 0.01, max < 0.04 on the float path; the
//! rgba8 path within 1.5/255 of the float path). The egui app shares its own device through
//! [`GpuRenderer::from_existing`] and reaches the texture handoff by downcasting
//! ([`Renderer::as_any`]) — wgpu types stay off the trait.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use neggpu::{GpuContext, GpuError, MaskBuffer, RenderPipeline, SourceBuffer};
use negkit::reference::{self, Histogram256};
use negkit::{RenderParams, RgbImage};
use rayon::prelude::*;

use crate::mask::MaskPlane;
use crate::{Result, SessionError};

/// A linear render input the session built once and keeps: `id` changes whenever the pixels do.
#[derive(Clone, Debug)]
pub struct RenderSource {
    /// Unique per distinct buffer within a process (a GPU renderer's upload-cache key).
    pub id: u64,
    pub image: Arc<RgbImage>,
}

impl RenderSource {
    pub fn width(&self) -> usize {
        self.image.width
    }

    pub fn height(&self) -> usize {
        self.image.height
    }
}

/// Per-call render options (the kernels' flags word + the mask plane + whether to bin).
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Re-express the output as sRGB in-kernel (the flags bit 0: unmanaged canvases and the
    /// sRGB export leg). Off = Adobe-RGB-encoded working space (every parity fixture).
    pub srgb: bool,
    /// Bin the 4 × 256 histogram (over the working encode + levels remap, never the sRGB
    /// re-expression — the `histogram` kernel's contract).
    pub compute_histogram: bool,
    /// The Contrast Mask plane when the mask is on (`mask_val_scale` non-zero). A GPU renderer
    /// keys its upload on the `Arc` pointer and holds the `Arc` while the upload is cached.
    pub mask_plane: Option<Arc<MaskPlane>>,
}

/// The float path's output: the encoded image (working space or sRGB per the options).
#[derive(Clone, Debug)]
pub struct RenderResult {
    pub image: RgbImage,
    pub histogram: Option<Histogram256>,
}

/// The display path's output: tightly packed RGBA8 (alpha 255), GPU-quantized semantics.
#[derive(Clone, Debug)]
pub struct DisplayResult {
    pub rgba8: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub histogram: Option<Histogram256>,
}

/// A render backend. Implementations must be safe to call from several sessions; the CPU one
/// is stateless, a GPU one serializes internally (the Swift pipeline's lock).
pub trait Renderer: Send + Sync {
    /// Short name for logs / `negcli` (`"cpu"`, a GPU adapter name…).
    fn name(&self) -> String;

    /// The float path: normalize → print curve (+ colour pop) → encode. Export and parity.
    fn render(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<RenderResult>;

    /// The display path: the same kernels into RGBA8.
    fn render_display(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<DisplayResult>;

    /// The session dropped the source with this `id` (a crop changed, a tier was released, the
    /// session closed): a renderer holding a device-side copy frees it. No-op by default.
    fn forget_source(&self, _id: u64) {}

    /// Downcast hook for renderer-specific entry points (the egui app reaches
    /// [`GpuRenderer::render_display_to_texture`] through it). `None` by default.
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }
}

/// `negkit::reference` behind the trait — the numerical source of truth, rayon over rows.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuRenderer;

impl CpuRenderer {
    /// Run the chain into a fresh buffer. The histogram is binned on the working encode BEFORE
    /// the optional sRGB re-expression, which is exactly what the GPU's `histogram` kernel sees.
    fn run(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> RenderResult {
        let mut image = (*source.image).clone();
        let mask: Option<&dyn reference::MaskSampler> = options
            .mask_plane
            .as_deref()
            .map(|p| p as &dyn reference::MaskSampler);
        reference::normalize_in_place(&mut image, &params.final_bounds);
        reference::apply_print_curve_in_place(&mut image, params, mask);
        let histogram = if options.compute_histogram {
            reference::encode_output_in_place(&mut image, &params.levels_points, false);
            let h = reference::histogram_256(&image);
            if options.srgb {
                srgb_in_place(&mut image);
            }
            Some(h)
        } else {
            reference::encode_output_in_place(&mut image, &params.levels_points, options.srgb);
            None
        };
        RenderResult { image, histogram }
    }
}

/// The kernels' flags-bit leg applied to an already working-encoded buffer.
fn srgb_in_place(image: &mut RgbImage) {
    image.data.par_chunks_exact_mut(3).for_each(|px| {
        let e = reference::srgb_from_encoded([px[0], px[1], px[2]]);
        px.copy_from_slice(&e);
    });
}

/// Encoded float → packed RGBA8 with the `encode_u8` kernel's `pack4x8unorm` rounding
/// ([`neggpu::pack_unorm8`]: `floor(255·clamp(x) + 0.5)`, alpha 255). Built in parallel
/// straight into its final allocation — no zero-fill-then-overwrite.
pub fn rgba8_from_encoded(image: &RgbImage) -> Vec<u8> {
    let px: Vec<[u8; 4]> = image
        .data
        .par_chunks_exact(3)
        .map(|p| {
            [
                neggpu::pack_unorm8(p[0]),
                neggpu::pack_unorm8(p[1]),
                neggpu::pack_unorm8(p[2]),
                255,
            ]
        })
        .collect();
    px.into_flattened()
}

/// [`rgba8_from_encoded`] into a caller-owned buffer (the `render_into` path on the CPU):
/// `dest` must hold exactly `width × height × 4` bytes; extra bytes are left alone.
pub fn rgba8_into(image: &RgbImage, dest: &mut [u8]) {
    dest.par_chunks_exact_mut(4)
        .zip(image.data.par_chunks_exact(3))
        .for_each(|(d, p)| {
            d[0] = neggpu::pack_unorm8(p[0]);
            d[1] = neggpu::pack_unorm8(p[1]);
            d[2] = neggpu::pack_unorm8(p[2]);
            d[3] = 255;
        });
}

impl Renderer for CpuRenderer {
    fn name(&self) -> String {
        "cpu (negkit::reference)".to_owned()
    }

    fn render(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<RenderResult> {
        if source.image.pixel_count() == 0 {
            return Err(SessionError::Render("empty source".to_owned()));
        }
        Ok(self.run(source, params, options))
    }

    fn render_display(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<DisplayResult> {
        let result = self.render(source, params, options)?;
        Ok(DisplayResult {
            rgba8: rgba8_from_encoded(&result.image),
            width: result.image.width,
            height: result.image.height,
            histogram: result.histogram,
        })
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

// ───────────────────────────── GPU ─────────────────────────────

/// Most session sources kept uploaded (a safety net — the session's `forget_source` is what
/// normally bounds it: ≤ 6 live sources per session × the 2 LRU sessions, minus released tiers).
pub const SOURCE_CACHE_CAP: usize = 8;
/// Most Contrast Mask planes kept uploaded (the live one plus the one just replaced).
pub const MASK_CACHE_CAP: usize = 2;

struct CachedSource {
    id: u64,
    buffer: Arc<SourceBuffer>,
}

struct CachedMask {
    /// `Arc::as_ptr` of the plane — unique while `plane` is held below.
    key: usize,
    /// Held so the address above cannot be reused by a newer plane while this entry lives.
    _plane: Arc<MaskPlane>,
    buffer: Arc<MaskBuffer>,
}

#[derive(Default)]
struct GpuCache {
    sources: VecDeque<CachedSource>,
    masks: VecDeque<CachedMask>,
}

/// What the renderer currently keeps uploaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuRendererStats {
    /// Source ids with a device-side copy, least recent first.
    pub sources: Vec<u64>,
    /// Uploaded mask planes.
    pub masks: usize,
    /// The display pipeline's tier cache.
    pub display: neggpu::CacheStats,
    /// The export pipeline's tier cache (normally empty: large float renders use scratch).
    pub export: neggpu::CacheStats,
}

/// `neggpu` behind the trait: one device, TWO pipelines — `display` for the interactive paths
/// (its lock is held through `poll(Wait)`, so a 24 MP export on the same instance would stall
/// every slider tick for ~0.3 s) and `export` for float renders above the pipeline's cached
/// size budget. Session sources are uploaded once and kept keyed by [`RenderSource::id`] until
/// the session forgets them; mask planes are kept keyed by plane identity.
pub struct GpuRenderer {
    ctx: GpuContext,
    display: RenderPipeline,
    export: RenderPipeline,
    cache: Mutex<GpuCache>,
}

impl std::fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuRenderer")
            .field("adapter", &self.ctx.adapter_info().name)
            .field("backend", &self.ctx.backend_name())
            .finish()
    }
}

impl GpuRenderer {
    /// Two pipelines on `ctx` (display + export).
    pub fn new(ctx: GpuContext) -> Result<Self> {
        let display = RenderPipeline::new(&ctx)?;
        let export = RenderPipeline::new(&ctx)?;
        Ok(Self {
            ctx,
            display,
            export,
            cache: Mutex::new(GpuCache::default()),
        })
    }

    /// On the process-wide headless context ([`neggpu::shared_headless`]): `Ok(None)` when no
    /// GPU is usable (no adapter, or `NEG_GPU=off`) — the caller falls back to [`CpuRenderer`].
    pub fn new_headless() -> Result<Option<Self>> {
        match neggpu::shared_headless() {
            Some(ctx) => Self::new(ctx).map(Some),
            None => Ok(None),
        }
    }

    /// On a device the app already owns (eframe's `WgpuSetup::Existing`): the device must have
    /// been created with [`neggpu::required_limits`] or the full tier / full-size export return
    /// `TooLarge`. The uncaptured-error handler is installed on the device (see
    /// `GpuContext::from_existing`).
    pub fn from_existing(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: wgpu::AdapterInfo,
    ) -> Result<Self> {
        Self::new(GpuContext::from_existing(device, queue, adapter_info))
    }

    /// The device / queue / adapter this renderer runs on.
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// The adapter's name (status bars, `negcli bench`).
    pub fn adapter_name(&self) -> String {
        self.ctx.adapter_info().name.clone()
    }

    /// The interactive pipeline (the one the texture handoff renders on).
    pub fn display_pipeline(&self) -> &RenderPipeline {
        &self.display
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GpuCache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The uploaded copy of `source` on the display pipeline, uploading (and evicting the
    /// least recent beyond the cap) when missing.
    fn cached_source(&self, source: &RenderSource) -> Result<Arc<SourceBuffer>> {
        {
            let mut cache = self.lock();
            if let Some(pos) = cache.sources.iter().position(|c| c.id == source.id) {
                let hit = cache
                    .sources
                    .remove(pos)
                    .ok_or(SessionError::Invalid("source cache slot vanished"))?;
                let buffer = Arc::clone(&hit.buffer);
                cache.sources.push_back(hit);
                return Ok(buffer);
            }
        }
        // Upload outside the cache lock (a big upload must not stall a texture handoff that
        // only needs a lookup).
        let buffer = Arc::new(self.display.upload(&source.image)?);
        let mut cache = self.lock();
        cache.sources.retain(|c| c.id != source.id);
        cache.sources.push_back(CachedSource {
            id: source.id,
            buffer: Arc::clone(&buffer),
        });
        while cache.sources.len() > SOURCE_CACHE_CAP {
            cache.sources.pop_front();
        }
        Ok(buffer)
    }

    /// The uploaded copy of `plane` on the display pipeline (identity = the `Arc` pointer).
    fn cached_mask(&self, plane: &Arc<MaskPlane>) -> Result<Arc<MaskBuffer>> {
        let key = Arc::as_ptr(plane) as usize;
        {
            let mut cache = self.lock();
            if let Some(pos) = cache.masks.iter().position(|c| c.key == key) {
                let hit = cache
                    .masks
                    .remove(pos)
                    .ok_or(SessionError::Invalid("mask cache slot vanished"))?;
                let buffer = Arc::clone(&hit.buffer);
                cache.masks.push_back(hit);
                return Ok(buffer);
            }
        }
        let buffer = Arc::new(
            self.display
                .upload_mask(&plane.data, plane.width, plane.height)?,
        );
        let mut cache = self.lock();
        cache.masks.retain(|c| c.key != key);
        cache.masks.push_back(CachedMask {
            key,
            _plane: Arc::clone(plane),
            buffer: Arc::clone(&buffer),
        });
        while cache.masks.len() > MASK_CACHE_CAP {
            cache.masks.pop_front();
        }
        Ok(buffer)
    }

    /// Whether a float render of `source` belongs on the export pipeline (above the display
    /// pipeline's retained-tier budget: a scratch render that must not block the canvas).
    fn is_export_sized(source: &RenderSource) -> bool {
        (source.image.pixel_count() as u64) > neggpu::CACHE_PIXEL_BUDGET
    }

    /// Drop the device copy of a source (the session's `forget_source`).
    pub fn forget(&self, id: u64) {
        self.lock().sources.retain(|c| c.id != id);
    }

    /// Drop every uploaded source and mask plus both pipelines' tier caches (a session closed
    /// for good, the app lost focus…).
    pub fn release_cached(&self) {
        {
            let mut cache = self.lock();
            cache.sources.clear();
            cache.masks.clear();
        }
        self.display.release_cached();
        self.export.release_cached();
    }

    /// What is currently uploaded / retained.
    pub fn stats(&self) -> GpuRendererStats {
        let cache = self.lock();
        GpuRendererStats {
            sources: cache.sources.iter().map(|c| c.id).collect(),
            masks: cache.masks.len(),
            display: self.display.cache_stats(),
            export: self.export.cache_stats(),
        }
    }

    /// Zero-allocation display path (`si_render_into`): writes the packed RGBA8 frame straight
    /// from the GPU readback into `dest`. `Ok(None)` without rendering when `dest` is shorter
    /// than `width × height × 4` (size it and retry — the upload stays cached).
    pub fn render_display_into(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
        dest: &mut [u8],
    ) -> Result<Option<Histogram256>> {
        let buffer = self.cached_source(source)?;
        let mask = options
            .mask_plane
            .as_ref()
            .map(|p| self.cached_mask(p))
            .transpose()?;
        let opts = neggpu::RenderOptions {
            srgb_encode: options.srgb,
            mask: mask.as_deref(),
        };
        Ok(self
            .display
            .render_display_into(&buffer, params, opts, dest)?)
    }

    /// No-readback display path for the egui app: the frame is copied on the GPU into
    /// `texture` — an `Rgba8Unorm`, `COPY_DST` texture of exactly the source's size (e.g. one
    /// registered with egui-wgpu). Returns the histogram (its 4 KB readback is the only CPU
    /// wait). Reach it from an `Arc<dyn Renderer>` through [`Renderer::as_any`].
    pub fn render_display_to_texture(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
        texture: &wgpu::Texture,
    ) -> Result<Histogram256> {
        let buffer = self.cached_source(source)?;
        let mask = options
            .mask_plane
            .as_ref()
            .map(|p| self.cached_mask(p))
            .transpose()?;
        let opts = neggpu::RenderOptions {
            srgb_encode: options.srgb,
            mask: mask.as_deref(),
        };
        Ok(self
            .display
            .render_display_to_texture(&buffer, params, opts, texture)?)
    }
}

impl Renderer for GpuRenderer {
    fn name(&self) -> String {
        let info = self.ctx.adapter_info();
        format!("gpu ({} · {})", info.name, self.ctx.backend_name())
    }

    fn render(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<RenderResult> {
        let result = if Self::is_export_sized(source) {
            // Export: fresh uploads on the export pipeline, nothing cached, nothing held while
            // the canvas keeps rendering on `display`.
            let buffer = self.export.upload(&source.image)?;
            let mask = options
                .mask_plane
                .as_ref()
                .map(|p| self.export.upload_mask(&p.data, p.width, p.height))
                .transpose()?;
            let opts = neggpu::RenderOptions {
                srgb_encode: options.srgb,
                mask: mask.as_ref(),
            };
            self.export.render(&buffer, params, opts)?
        } else {
            let buffer = self.cached_source(source)?;
            let mask = options
                .mask_plane
                .as_ref()
                .map(|p| self.cached_mask(p))
                .transpose()?;
            let opts = neggpu::RenderOptions {
                srgb_encode: options.srgb,
                mask: mask.as_deref(),
            };
            self.display.render(&buffer, params, opts)?
        };
        Ok(RenderResult {
            image: result.image,
            histogram: options.compute_histogram.then_some(result.histogram),
        })
    }

    fn render_display(
        &self,
        source: &RenderSource,
        params: &RenderParams,
        options: &RenderOptions,
    ) -> Result<DisplayResult> {
        let buffer = self.cached_source(source)?;
        let mask = options
            .mask_plane
            .as_ref()
            .map(|p| self.cached_mask(p))
            .transpose()?;
        let opts = neggpu::RenderOptions {
            srgb_encode: options.srgb,
            mask: mask.as_deref(),
        };
        let r = self.display.render_display(&buffer, params, opts)?;
        Ok(DisplayResult {
            rgba8: r.rgba8,
            width: r.width as usize,
            height: r.height as usize,
            histogram: options.compute_histogram.then_some(r.histogram),
        })
    }

    fn forget_source(&self, id: u64) {
        self.forget(id);
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}

impl From<GpuError> for SessionError {
    fn from(e: GpuError) -> Self {
        SessionError::Gpu(e)
    }
}

/// The renderer this build defaults to: the GPU when a headless context comes up and `NEG_GPU`
/// isn't off, else the CPU chain. Logs which (and why not, when the GPU was wanted but failed).
pub fn default_renderer() -> Arc<dyn Renderer> {
    if std::env::var(neggpu::context::ENV_GPU)
        .ok()
        .is_some_and(|v| neggpu::context::gpu_disabled_by(&v))
    {
        log::info!("NEG_GPU=off: CPU renderer");
        return Arc::new(CpuRenderer);
    }
    match GpuRenderer::new_headless() {
        Ok(Some(gpu)) => {
            log::info!("renderer: {}", gpu.name());
            Arc::new(gpu)
        }
        Ok(None) => {
            log::info!(
                "renderer: cpu (no GPU: {})",
                neggpu::unavailable_reason().unwrap_or_else(|| "no adapter".to_owned())
            );
            Arc::new(CpuRenderer)
        }
        Err(e) => {
            log::warn!("renderer: cpu (GPU pipeline failed: {e})");
            Arc::new(CpuRenderer)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    /// The CPU display path quantizes like `pack4x8unorm` — round-to-nearest, never
    /// truncation (which would drift densitometer read-outs by 0.5/255 between backends).
    #[test]
    fn rgba8_rounds_like_pack4x8unorm() {
        let values: Vec<f32> = (0..=1020)
            .map(|i| i as f32 / 1020.0)
            .chain([-0.5, 1.5, 0.5 / 255.0, 1.49 / 255.0, 254.5 / 255.0])
            .collect();
        let n = values.len() / 3;
        let img = RgbImage::from_vec(n, 1, values[..n * 3].to_vec()).unwrap();
        let bytes = rgba8_from_encoded(&img);
        assert_eq!(bytes.len(), n * 4);
        for (i, v) in img.data.iter().enumerate() {
            let want = (255.0 * v.clamp(0.0, 1.0) + 0.5).floor() as u8;
            assert_eq!(bytes[i / 3 * 4 + i % 3], want, "value {v}");
            assert_eq!(bytes[i / 3 * 4 + i % 3], neggpu::pack_unorm8(*v));
        }
        assert!(bytes.chunks_exact(4).all(|p| p[3] == 255));
        // The half-step rounds up (truncation would give 0 here).
        let edge = RgbImage::from_vec(1, 1, vec![0.5 / 255.0, 0.0, 1.0]).unwrap();
        assert_eq!(&rgba8_from_encoded(&edge)[..3], &[1, 0, 255]);
    }

    #[test]
    fn cpu_renderer_downcasts_and_ignores_forget() {
        let r: Arc<dyn Renderer> = Arc::new(CpuRenderer);
        assert!(r.as_any().unwrap().downcast_ref::<CpuRenderer>().is_some());
        assert!(r.as_any().unwrap().downcast_ref::<GpuRenderer>().is_none());
        r.forget_source(42);
    }
}

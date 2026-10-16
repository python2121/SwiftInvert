//! `RenderPipeline` — the six compute pipelines and the per-size buffer tower, the wgpu
//! counterpart of `VulkanRenderPipeline.swift` / `RenderPipeline.swift`.
//!
//! One submission per render: `normalize_log` → `print_curve` → (`color_pop` when any of its
//! controls is off-default — `ColorPop::is_active`, the same gate as the CPU chain) →
//! `histogram` (after a `clear_buffer`) → `encode_f` / `encode_u8`, then a copy into a
//! persistent `MAP_READ` staging buffer, `map_async` + `device.poll(Wait)`, a memcpy into
//! caller memory, unmap. The float path (export, parity) returns an `RgbImage`; the display
//! path returns packed RGBA8 — allocating ([`RenderPipeline::render_display`]), into caller
//! memory ([`RenderPipeline::render_display_into`], the `si_render_into` zero-allocation
//! contract) or straight into a caller-owned `Rgba8Unorm` texture
//! ([`RenderPipeline::render_display_to_texture`], no CPU readback of pixels at all).
//!
//! Memory discipline (the Vulkan port's 3.9 s/frame lesson): the chain's buffers are
//! device-local and never read by the CPU; only the staging buffers are mapped, and each map
//! is held by a guard that unmaps on every exit path, so a buffer is never left mapped for the
//! next submission to trip over. Uniforms are `write_buffer`s into persistent buffers; bind
//! groups are cached with their tier, so the slider path allocates nothing on the host.
//!
//! **Tier caches** (mirroring the Swift pipeline's `intermediates` / `display8` caps):
//! - sizes up to [`CACHE_PIXEL_BUDGET`] pixels live in a small LRU of at most
//!   [`SMALL_TIER_CAP`] sizes (a crop drag changes the size every tick; without the cap VRAM
//!   grew without bound). One tier carries the float and the display targets for its size
//!   — they share the normalized/linear intermediates, which are the bulk of it;
//! - ONE larger size is retained for the DISPLAY paths (the medium/full HQ tiers: rebuilding
//!   a 24 MP tier every slider tick cost 58 ms against a 60 ms budget) until a different
//!   large size is displayed or [`RenderPipeline::release_cached`] runs;
//! - large FLOAT renders (export) use scratch intermediates dropped as the render returns.
//!
//! **Retention of caller buffers.** A cached tier's bind groups hold the last `SourceBuffer`
//! / `MaskBuffer` rendered at that size, so a dropped source's VRAM is not freed until the
//! pipeline forgets it: every `SourceBuffer`/`MaskBuffer` reports its id on drop and the next
//! `upload`, `upload_mask` or render on this pipeline drops the bind groups that referenced
//! it (`release_cached` drops everything at once).
//!
//! **Errors.** Every upload and render runs under Validation + OutOfMemory error scopes and
//! turns a captured error into `Err(GpuError::Validation / OutOfMemory)`; sizes past the
//! device limits are refused before any GPU work ([`GpuError::TooLarge`]).
//!
//! Renders are serialized by an internal mutex held through `poll(Wait)` (the Swift
//! contract; see the crate docs for why an app wants separate instances for display and
//! export).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use negkit::lab::ColorPop;
use negkit::{RenderParams, RgbImage};

use crate::uniforms::{self, FLAG_SRGB, LEVELS_BUFFER_FLOATS, Push};
use crate::{GpuContext, GpuError};

/// Kernel workgroup size (`@workgroup_size(256)` in the WGSL).
const WORKGROUP: u32 = 256;
/// Intermediates for renders up to this many pixels live in the small LRU cache; one larger
/// DISPLAY size is retained besides, larger float (export) renders use scratch.
pub const CACHE_PIXEL_BUDGET: u64 = 4_000_000;
/// Most sizes retained in the small cache (least-recently-rendered evicted first).
pub const SMALL_TIER_CAP: usize = 4;
/// Bytes of the 4 × 256 `u32` histogram.
const HIST_BYTES: u64 = 1024 * 4;
/// The WGSL source, shipped in the binary.
pub const WGSL_SOURCE: &str = include_str!("wgsl/negpipeline.wgsl");

/// The histogram kernel's bins: 4 × 256 raw counts, `[R | G | B | Rec.709 luma]`.
pub type Histogram = [u32; 1024];

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Ids of dropped source/mask buffers, drained by the pipeline to release the bind groups
/// that still reference them.
type Retired = Arc<Mutex<Vec<u64>>>;

fn retire(list: &Retired, id: u64) {
    list.lock().unwrap_or_else(PoisonError::into_inner).push(id);
}

/// A linear RGB source image uploaded once and rendered many times (the session keeps one per
/// tier; slider drags re-upload nothing).
#[derive(Debug)]
pub struct SourceBuffer {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    id: u64,
    retired: Retired,
}

impl Drop for SourceBuffer {
    fn drop(&mut self) {
        retire(&self.retired, self.id);
    }
}

impl SourceBuffer {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }
    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }
    /// `width × height`.
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
    /// Bytes a packed RGBA8 display frame of this source needs.
    pub fn rgba8_len(&self) -> usize {
        (self.pixel_count() * 4) as usize
    }
    /// This buffer's pipeline-unique id (what [`CacheStats`] reports as retained).
    pub fn id(&self) -> u64 {
        self.id
    }
}

/// An uploaded Contrast Mask plane (single-channel floats on the analysis grid).
#[derive(Debug)]
pub struct MaskBuffer {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    id: u64,
    retired: Retired,
}

impl Drop for MaskBuffer {
    fn drop(&mut self) {
        retire(&self.retired, self.id);
    }
}

impl MaskBuffer {
    /// Plane width.
    pub fn width(&self) -> u32 {
        self.width
    }
    /// Plane height.
    pub fn height(&self) -> u32 {
        self.height
    }
    /// This buffer's pipeline-unique id.
    pub fn id(&self) -> u64 {
        self.id
    }
}

/// What the pipeline currently retains (for tests, status bars and leak hunting).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Sizes in the small LRU cache, least recent first.
    pub small_sizes: Vec<(u32, u32)>,
    /// The retained large display size, if any.
    pub large_display: Option<(u32, u32)>,
    /// Source buffer ids still referenced by cached bind groups.
    pub retained_sources: Vec<u64>,
    /// Mask buffer ids still referenced by cached bind groups.
    pub retained_masks: Vec<u64>,
}

/// Per-render options.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions<'a> {
    /// Re-express the encoded output as sRGB in-kernel (flags bit 0). Off = Adobe-RGB-encoded
    /// working space, what every parity fixture holds.
    pub srgb_encode: bool,
    /// The Contrast Mask plane; only sampled when `params.mask_val_scale` is non-zero.
    pub mask: Option<&'a MaskBuffer>,
}

/// The float path's output: the encoded image (`encode_f`) and the histogram.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderResult {
    /// Encoded output, Adobe-RGB working space (or sRGB with `srgb_encode`).
    pub image: RgbImage,
    /// 4 × 256 raw counts over the encoded (non-sRGB) values.
    pub histogram: Histogram,
}

/// The display path's output: packed RGBA8 (alpha 255), row-major, tightly packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayResult {
    pub rgba8: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub histogram: Histogram,
}

/// Which encode kernel ends the chain and where it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// `encode_f` into the float buffer + staging.
    Float,
    /// `encode_u8`, tightly packed, into the u8 buffer + staging.
    U8,
    /// `encode_u8` with the given row pitch (pixels) into the padded buffer (texture copies).
    U8Padded(u32),
}

struct Kernel {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

struct Kernels {
    normalize: Kernel,
    print_curve: Kernel,
    color_pop: Kernel,
    histogram: Kernel,
    encode_f: Kernel,
    encode_u8: Kernel,
}

/// Buffers shared by every render.
struct Shared {
    norm_u: wgpu::Buffer,
    curve_u: wgpu::Buffer,
    push: wgpu::Buffer,
    levels: wgpu::Buffer,
    /// 4-byte stand-in for the mask plane when the mask is off (every declared binding must
    /// be bound; `mask_scale.w` keeps it unread).
    dummy_mask: wgpu::Buffer,
    hist: wgpu::Buffer,
    hist_staging: wgpu::Buffer,
}

/// The per-size tower: intermediates, optional encode targets and their bind groups.
struct Tier {
    width: u32,
    height: u32,
    normalized: wgpu::Buffer,
    linear: wgpu::Buffer,
    encoded_f: Option<(wgpu::Buffer, wgpu::Buffer)>,
    encoded_u8: Option<(wgpu::Buffer, wgpu::Buffer)>,
    /// (row pitch in pixels, buffer) for the texture path.
    padded_u8: Option<(u32, wgpu::Buffer)>,
    /// Keyed by the source buffer's id.
    bg_normalize: Option<(u64, wgpu::BindGroup)>,
    /// Keyed by the mask buffer's id (0 = dummy).
    bg_curve: Option<(u64, wgpu::BindGroup)>,
    bg_pop: wgpu::BindGroup,
    /// Index = colour pop active (content buffer = `linear` / `normalized`).
    bg_hist: [wgpu::BindGroup; 2],
    bg_encode_f: [Option<wgpu::BindGroup>; 2],
    bg_encode_u8: [Option<wgpu::BindGroup>; 2],
    bg_encode_padded: [Option<wgpu::BindGroup>; 2],
}

/// The tier caches (see the module docs).
#[derive(Default)]
struct Tiers {
    /// Small sizes, least-recently-rendered FIRST, at most [`SMALL_TIER_CAP`].
    small: Vec<Tier>,
    /// The one retained large display tier.
    large_display: Option<Tier>,
}

impl Tiers {
    fn for_each_mut(&mut self, mut f: impl FnMut(&mut Tier)) {
        self.small.iter_mut().for_each(&mut f);
        if let Some(t) = self.large_display.as_mut() {
            f(t);
        }
    }

    fn for_each(&self, mut f: impl FnMut(&Tier)) {
        self.small.iter().for_each(&mut f);
        if let Some(t) = self.large_display.as_ref() {
            f(t);
        }
    }
}

struct State {
    kernels: Kernels,
    shared: Shared,
    tiers: Tiers,
    /// Scratch tier for large float renders; always `None` between renders.
    scratch: Option<Tier>,
    retired: Retired,
}

/// The GPU render chain. Create once per device; `Sync` — renders serialize internally.
pub struct RenderPipeline {
    ctx: GpuContext,
    state: Mutex<State>,
}

impl std::fmt::Debug for RenderPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPipeline")
            .field("ctx", &self.ctx)
            .finish()
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn make_kernel(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry: &'static str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> Kernel {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(entry),
        entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    Kernel { layout, pipeline }
}

fn bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffers: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

fn new_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

/// A staging buffer with a `map_async` in flight or complete; unmaps on drop, so no exit
/// path — early `?`, a failed poll, a failed sibling map — can leave it mapped for the next
/// submission's copy to trip over. (Unmapping an idle buffer is itself a validation error,
/// which is why the guard exists only once `map_async` was issued.)
struct Mapped<'a> {
    buffer: &'a wgpu::Buffer,
    len: u64,
}

impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        self.buffer.unmap();
    }
}

impl Mapped<'_> {
    fn view(&self) -> Result<wgpu::BufferView, GpuError> {
        self.buffer
            .slice(..self.len)
            .get_mapped_range()
            .map_err(|e| GpuError::Readback(e.to_string()))
    }
}

/// Issue `map_async` on every (buffer, len), wait for `index`, and confirm each map resolved.
/// The guards come back mapped on success; on failure they are dropped (unmapped) here.
fn map_sync<'a>(
    device: &wgpu::Device,
    index: wgpu::SubmissionIndex,
    targets: &[(&'a wgpu::Buffer, u64)],
) -> Result<Vec<Mapped<'a>>, GpuError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut guards = Vec::with_capacity(targets.len());
    for &(buffer, len) in targets {
        let tx = tx.clone();
        buffer
            .slice(..len)
            .map_async(wgpu::MapMode::Read, move |result| {
                // A closed receiver only means the caller already gave up.
                let _ = tx.send(result);
            });
        guards.push(Mapped { buffer, len });
    }
    drop(tx);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map_err(|e| GpuError::Readback(e.to_string()))?;
    for _ in targets {
        match rx.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(GpuError::Readback(e.to_string())),
            Err(_) => {
                return Err(GpuError::Readback(
                    "staging map did not resolve after the wait".into(),
                ));
            }
        }
    }
    Ok(guards)
}

fn read_histogram(mapped: &Mapped<'_>) -> Result<Histogram, GpuError> {
    let view = mapped.view()?;
    let mut bins = [0u32; 1024];
    bytemuck::cast_slice_mut::<u32, u8>(&mut bins).copy_from_slice(&view[..HIST_BYTES as usize]);
    Ok(bins)
}

/// Run `body` under Validation + OutOfMemory error scopes; a captured error wins over the
/// body's own result (it is the root cause).
fn scoped<T>(
    device: &wgpu::Device,
    body: impl FnOnce() -> Result<T, GpuError>,
) -> Result<T, GpuError> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let result = body();
    let oom_err = pollster::block_on(oom.pop());
    let val_err = pollster::block_on(validation.pop());
    if let Some(e) = oom_err {
        return Err(GpuError::OutOfMemory(e.to_string()));
    }
    if let Some(e) = val_err {
        return Err(GpuError::Validation(e.to_string()));
    }
    result
}

impl RenderPipeline {
    /// Compile the WGSL module once and build the six compute pipelines.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuError> {
        let device = ctx.device();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("negpipeline.wgsl"),
            source: wgpu::ShaderSource::Wgsl(WGSL_SOURCE.into()),
        });
        let kernels = Kernels {
            normalize: make_kernel(
                device,
                &module,
                "normalize_log",
                &[
                    storage_entry(0, true),
                    storage_entry(1, false),
                    uniform_entry(2),
                    uniform_entry(4),
                ],
            ),
            print_curve: make_kernel(
                device,
                &module,
                "print_curve",
                &[
                    storage_entry(0, true),
                    storage_entry(1, false),
                    uniform_entry(3),
                    uniform_entry(4),
                    storage_entry(6, true),
                ],
            ),
            color_pop: make_kernel(
                device,
                &module,
                "color_pop",
                &[
                    storage_entry(0, true),
                    storage_entry(1, false),
                    uniform_entry(3),
                    uniform_entry(4),
                ],
            ),
            histogram: make_kernel(
                device,
                &module,
                "histogram",
                &[
                    storage_entry(0, true),
                    uniform_entry(4),
                    storage_entry(5, true),
                    storage_entry(7, false),
                ],
            ),
            encode_f: make_kernel(
                device,
                &module,
                "encode_f",
                &[
                    storage_entry(0, true),
                    storage_entry(1, false),
                    uniform_entry(4),
                    storage_entry(5, true),
                ],
            ),
            encode_u8: make_kernel(
                device,
                &module,
                "encode_u8",
                &[
                    storage_entry(0, true),
                    uniform_entry(4),
                    storage_entry(5, true),
                    storage_entry(8, false),
                ],
            ),
        };
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let shared = Shared {
            norm_u: new_buffer(device, "norm_u", 48, uniform),
            curve_u: new_buffer(device, "curve_u", 304, uniform),
            push: new_buffer(device, "push", 16, uniform),
            levels: new_buffer(
                device,
                "levels",
                (LEVELS_BUFFER_FLOATS * 4) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            dummy_mask: new_buffer(device, "dummy_mask", 4, wgpu::BufferUsages::STORAGE),
            hist: new_buffer(
                device,
                "hist",
                HIST_BYTES,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            hist_staging: new_buffer(
                device,
                "hist_staging",
                HIST_BYTES,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
        };
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Validation(err.to_string()));
        }
        Ok(Self {
            ctx: ctx.clone(),
            state: Mutex::new(State {
                kernels,
                shared,
                tiers: Tiers::default(),
                scratch: None,
                retired: Arc::new(Mutex::new(Vec::new())),
            }),
        })
    }

    /// The context this pipeline renders on.
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    fn check_size(&self, bytes: u64) -> Result<(), GpuError> {
        self.ctx.check_buffer_size(bytes)
    }

    /// Whether a `width × height` render fits this device's limits (the float intermediates
    /// are 12 bytes per pixel): `Err(TooLarge)` naming the binding limit otherwise. Checked
    /// again inside every upload/render, so callers may simply try and match the error.
    pub fn fits_render(&self, width: u32, height: u32) -> Result<(), GpuError> {
        self.check_size(u64::from(width) * u64::from(height) * 12)
    }

    /// What the pipeline currently retains.
    pub fn cache_stats(&self) -> CacheStats {
        let state = self.lock();
        let mut stats = CacheStats {
            small_sizes: state
                .tiers
                .small
                .iter()
                .map(|t| (t.width, t.height))
                .collect(),
            large_display: state
                .tiers
                .large_display
                .as_ref()
                .map(|t| (t.width, t.height)),
            ..CacheStats::default()
        };
        state.tiers.for_each(|t| {
            if let Some((id, _)) = &t.bg_normalize {
                stats.retained_sources.push(*id);
            }
            if let Some((id, _)) = &t.bg_curve
                && *id != 0
            {
                stats.retained_masks.push(*id);
            }
        });
        stats
    }

    /// Forget the bind groups that referenced dropped source/mask buffers.
    fn purge_retired(state: &mut State) {
        let retired: Vec<u64> =
            std::mem::take(&mut *state.retired.lock().unwrap_or_else(PoisonError::into_inner));
        if retired.is_empty() {
            return;
        }
        state.tiers.for_each_mut(|t| {
            if t.bg_normalize
                .as_ref()
                .is_some_and(|(id, _)| retired.contains(id))
            {
                t.bg_normalize = None;
            }
            if t.bg_curve
                .as_ref()
                .is_some_and(|(id, _)| retired.contains(id))
            {
                t.bg_curve = None;
            }
        });
    }

    /// One memcpy into a device buffer created mapped (no intermediate staging).
    fn upload_floats(&self, label: &str, data: &[f32]) -> Result<wgpu::Buffer, GpuError> {
        let bytes = (data.len() * 4) as u64;
        self.check_size(bytes)?;
        // Dropped buffers release their bind groups before new VRAM is taken.
        Self::purge_retired(&mut self.lock());
        let device = self.ctx.device();
        scoped(device, || {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: true,
            });
            {
                let mut view = buffer
                    .slice(..)
                    .get_mapped_range_mut()
                    .map_err(|e| GpuError::Readback(e.to_string()))?;
                view.copy_from_slice(bytemuck::cast_slice(data));
            }
            buffer.unmap();
            Ok(buffer)
        })
    }

    fn retired(&self) -> Retired {
        Arc::clone(&self.lock().retired)
    }

    /// Upload a linear RGB image (interleaved f32 — a straight memcpy).
    pub fn upload(&self, image: &RgbImage) -> Result<SourceBuffer, GpuError> {
        let want = image
            .width
            .checked_mul(image.height)
            .and_then(|n| n.checked_mul(3));
        if want != Some(image.data.len()) || image.data.is_empty() {
            return Err(GpuError::Image(negkit::NegError::Shape {
                width: image.width,
                height: image.height,
                len: image.data.len(),
            }));
        }
        let (width, height) = dims_u32(image.width, image.height)?;
        let buffer = self.upload_floats("source", &image.data)?;
        Ok(SourceBuffer {
            buffer,
            width,
            height,
            id: next_id(),
            retired: self.retired(),
        })
    }

    /// Upload a Contrast Mask plane: `plane.len()` must be `width × height` (row-major,
    /// single channel).
    pub fn upload_mask(
        &self,
        plane: &[f32],
        width: usize,
        height: usize,
    ) -> Result<MaskBuffer, GpuError> {
        if width == 0 || height == 0 || width.checked_mul(height) != Some(plane.len()) {
            return Err(GpuError::Invalid(
                "mask plane length must equal width × height",
            ));
        }
        let (width, height) = dims_u32(width, height)?;
        let buffer = self.upload_floats("mask_plane", plane)?;
        Ok(MaskBuffer {
            buffer,
            width,
            height,
            id: next_id(),
            retired: self.retired(),
        })
    }

    /// Drop every cached intermediate and bind group (e.g. when the app loses focus or a
    /// session closes) — releases any retained source/mask buffers immediately.
    pub fn release_cached(&self) {
        let mut state = self.lock();
        state.tiers = Tiers::default();
        state.scratch = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A fresh tier (base intermediates + the bind groups that depend only on them).
    fn new_tier(
        &self,
        kernels: &Kernels,
        shared: &Shared,
        width: u32,
        height: u32,
    ) -> Result<Tier, GpuError> {
        let device = self.ctx.device();
        let float_bytes = u64::from(width) * u64::from(height) * 12;
        self.check_size(float_bytes)?;
        let storage = wgpu::BufferUsages::STORAGE;
        let normalized = new_buffer(device, "normalized", float_bytes, storage);
        let linear = new_buffer(device, "linear", float_bytes, storage);
        let bg_pop = bind_group(
            device,
            "color_pop",
            &kernels.color_pop.layout,
            &[
                (0, &linear),
                (1, &normalized),
                (3, &shared.curve_u),
                (4, &shared.push),
            ],
        );
        let bg_hist = [&linear, &normalized].map(|content| {
            bind_group(
                device,
                "histogram",
                &kernels.histogram.layout,
                &[
                    (0, content),
                    (4, &shared.push),
                    (5, &shared.levels),
                    (7, &shared.hist),
                ],
            )
        });
        Ok(Tier {
            width,
            height,
            normalized,
            linear,
            encoded_f: None,
            encoded_u8: None,
            padded_u8: None,
            bg_normalize: None,
            bg_curve: None,
            bg_pop,
            bg_hist,
            bg_encode_f: [None, None],
            bg_encode_u8: [None, None],
            bg_encode_padded: [None, None],
        })
    }

    /// Pick (or create) the tier slot for this render: the small LRU for sizes within the
    /// budget, the one retained large slot for large DISPLAY renders, scratch for large
    /// float renders.
    fn slot<'s>(
        &self,
        kernels: &Kernels,
        shared: &Shared,
        tiers: &'s mut Tiers,
        scratch: &'s mut Option<Tier>,
        source: &SourceBuffer,
        target: Target,
    ) -> Result<&'s mut Tier, GpuError> {
        let (w, h) = (source.width, source.height);
        let same = |t: &Tier| t.width == w && t.height == h;
        if source.pixel_count() <= CACHE_PIXEL_BUDGET {
            if let Some(pos) = tiers.small.iter().position(same) {
                // Touch: most recently rendered goes last.
                let tier = tiers.small.remove(pos);
                tiers.small.push(tier);
            } else {
                if tiers.small.len() >= SMALL_TIER_CAP {
                    tiers.small.remove(0);
                }
                let tier = self.new_tier(kernels, shared, w, h)?;
                tiers.small.push(tier);
            }
            return tiers
                .small
                .last_mut()
                .ok_or(GpuError::Invalid("small tier cache empty"));
        }
        if matches!(target, Target::Float) {
            *scratch = Some(self.new_tier(kernels, shared, w, h)?);
            return scratch
                .as_mut()
                .ok_or(GpuError::Invalid("scratch tier missing"));
        }
        if !tiers.large_display.as_ref().is_some_and(same) {
            // Free the previous large tier BEFORE allocating the next (hundreds of MB each).
            tiers.large_display = None;
            tiers.large_display = Some(self.new_tier(kernels, shared, w, h)?);
        }
        tiers
            .large_display
            .as_mut()
            .ok_or(GpuError::Invalid("large tier missing"))
    }

    /// Ensure the tier for `source`'s size exists with the buffers and bind groups `target`
    /// needs, and that its source/mask bind groups match the buffers handed in.
    fn prepare_tier<'s>(
        &self,
        kernels: &Kernels,
        shared: &Shared,
        tiers: &'s mut Tiers,
        scratch: &'s mut Option<Tier>,
        source: &SourceBuffer,
        mask: Option<&MaskBuffer>,
        target: Target,
    ) -> Result<&'s mut Tier, GpuError> {
        let device = self.ctx.device();
        let n = source.pixel_count();
        let float_bytes = n * 12;
        let tier = self.slot(kernels, shared, tiers, scratch, source, target)?;

        if tier.bg_normalize.as_ref().map(|(id, _)| *id) != Some(source.id) {
            let bg = bind_group(
                device,
                "normalize_log",
                &kernels.normalize.layout,
                &[
                    (0, &source.buffer),
                    (1, &tier.normalized),
                    (2, &shared.norm_u),
                    (4, &shared.push),
                ],
            );
            tier.bg_normalize = Some((source.id, bg));
        }
        let mask_id = mask.map_or(0, |m| m.id);
        if tier.bg_curve.as_ref().map(|(id, _)| *id) != Some(mask_id) {
            let plane = mask.map_or(&shared.dummy_mask, |m| &m.buffer);
            let bg = bind_group(
                device,
                "print_curve",
                &kernels.print_curve.layout,
                &[
                    (0, &tier.normalized),
                    (1, &tier.linear),
                    (3, &shared.curve_u),
                    (4, &shared.push),
                    (6, plane),
                ],
            );
            tier.bg_curve = Some((mask_id, bg));
        }

        let contents = [&tier.linear, &tier.normalized];
        let encode_bgs = |layout: &wgpu::BindGroupLayout, dst_binding: u32, dst: &wgpu::Buffer| {
            contents.map(|content| {
                Some(bind_group(
                    device,
                    "encode",
                    layout,
                    &[
                        (0, content),
                        (4, &shared.push),
                        (5, &shared.levels),
                        (dst_binding, dst),
                    ],
                ))
            })
        };
        let readback = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        let src_storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        match target {
            Target::Float => {
                if tier.encoded_f.is_none() {
                    let encoded = new_buffer(device, "encoded_f", float_bytes, src_storage);
                    let staging = new_buffer(device, "staging_f", float_bytes, readback);
                    tier.bg_encode_f = encode_bgs(&kernels.encode_f.layout, 1, &encoded);
                    tier.encoded_f = Some((encoded, staging));
                }
            }
            Target::U8 => {
                if tier.encoded_u8.is_none() {
                    let bytes = n * 4;
                    let encoded = new_buffer(device, "encoded_u8", bytes, src_storage);
                    let staging = new_buffer(device, "staging_u8", bytes, readback);
                    tier.bg_encode_u8 = encode_bgs(&kernels.encode_u8.layout, 8, &encoded);
                    tier.encoded_u8 = Some((encoded, staging));
                }
            }
            Target::U8Padded(pitch) => {
                if tier.padded_u8.as_ref().map(|(p, _)| *p) != Some(pitch) {
                    let bytes = u64::from(pitch) * u64::from(source.height) * 4;
                    self.check_size(bytes)?;
                    let padded = new_buffer(device, "padded_u8", bytes, src_storage);
                    tier.bg_encode_padded = encode_bgs(&kernels.encode_u8.layout, 8, &padded);
                    tier.padded_u8 = Some((pitch, padded));
                }
            }
        }
        Ok(tier)
    }

    /// Write the uniforms and record the whole chain into a fresh encoder (compute pass +
    /// the histogram/encode staging copies). The caller adds any texture copy and submits.
    fn encode_chain(
        &self,
        state: &mut State,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
        target: Target,
    ) -> Result<(wgpu::CommandEncoder, bool), GpuError> {
        let device = self.ctx.device();
        let queue = self.ctx.queue();
        let max_groups_x = self
            .ctx
            .limits()
            .max_compute_workgroups_per_dimension
            .max(1);
        let pop_active = ColorPop::from_render_params(params).is_active();
        let mask = opts.mask.filter(|_| params.mask_val_scale != [0.0; 3]);

        // Uniforms into their persistent buffers (no host allocation).
        let norm = uniforms::pack_norm(params);
        let curve = uniforms::pack_curve(
            params,
            mask.map(|m| (m.width, m.height)),
            (source.width, source.height),
        );
        let levels = uniforms::pack_levels(params);
        let n = source.pixel_count();
        let push = Push {
            n: u32::try_from(n).map_err(|_| GpuError::Invalid("pixel count exceeds u32"))?,
            flags: if opts.srgb_encode { FLAG_SRGB } else { 0 },
            width: source.width,
            row_pitch: match target {
                Target::U8Padded(pitch) => pitch,
                Target::Float | Target::U8 => 0,
            },
        };
        {
            let sh = &state.shared;
            queue.write_buffer(&sh.norm_u, 0, bytemuck::bytes_of(&norm));
            queue.write_buffer(&sh.curve_u, 0, bytemuck::bytes_of(&curve));
            queue.write_buffer(&sh.levels, 0, bytemuck::cast_slice(&levels));
            queue.write_buffer(&sh.push, 0, bytemuck::bytes_of(&push));
        }

        Self::purge_retired(state);
        let State {
            kernels,
            shared,
            tiers,
            scratch,
            ..
        } = state;
        let tier = self.prepare_tier(kernels, shared, tiers, scratch, source, mask, target)?;
        let pop_index = usize::from(pop_active);
        let (Some((_, bg_normalize)), Some((_, bg_curve))) = (&tier.bg_normalize, &tier.bg_curve)
        else {
            return Err(GpuError::Invalid("tier bind groups missing"));
        };
        let (encode_kernel, bg_encode, copy): (
            &Kernel,
            Option<&wgpu::BindGroup>,
            Option<(&wgpu::Buffer, &wgpu::Buffer, u64)>,
        ) = match target {
            Target::Float => (
                &kernels.encode_f,
                tier.bg_encode_f.get(pop_index).and_then(Option::as_ref),
                tier.encoded_f.as_ref().map(|(e, s)| (e, s, n * 12)),
            ),
            Target::U8 => (
                &kernels.encode_u8,
                tier.bg_encode_u8.get(pop_index).and_then(Option::as_ref),
                tier.encoded_u8.as_ref().map(|(e, s)| (e, s, n * 4)),
            ),
            Target::U8Padded(_) => (
                &kernels.encode_u8,
                tier.bg_encode_padded
                    .get(pop_index)
                    .and_then(Option::as_ref),
                None,
            ),
        };
        let Some(bg_encode) = bg_encode else {
            return Err(GpuError::Invalid("encode bind group missing"));
        };
        let Some(bg_hist) = tier.bg_hist.get(pop_index) else {
            return Err(GpuError::Invalid("histogram bind group missing"));
        };

        let groups = u32::try_from(n.div_ceil(u64::from(WORKGROUP)))
            .map_err(|_| GpuError::Invalid("dispatch exceeds u32"))?;
        let gx = groups.clamp(1, max_groups_x);
        let gy = groups.div_ceil(gx).max(1);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("neggpu render"),
        });
        encoder.clear_buffer(&shared.hist, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("neggpu chain"),
                timestamp_writes: None,
            });
            let mut run = |kernel: &Kernel, bg: &wgpu::BindGroup| {
                pass.set_pipeline(&kernel.pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.dispatch_workgroups(gx, gy, 1);
            };
            run(&kernels.normalize, bg_normalize);
            run(&kernels.print_curve, bg_curve);
            if pop_active {
                // Writes into `normalized` (already consumed), which becomes the content
                // buffer — the Metal/Vulkan chains' trick.
                run(&kernels.color_pop, &tier.bg_pop);
            }
            run(&kernels.histogram, bg_hist);
            run(encode_kernel, bg_encode);
        }
        if let Some((encoded, staging, bytes)) = copy {
            encoder.copy_buffer_to_buffer(encoded, 0, staging, 0, bytes);
        }
        encoder.copy_buffer_to_buffer(&shared.hist, 0, &shared.hist_staging, 0, HIST_BYTES);
        Ok((encoder, pop_active))
    }

    /// The tier `encode_chain` just rendered into.
    fn rendered_tier<'s>(state: &'s State, source: &SourceBuffer) -> Result<&'s Tier, GpuError> {
        if let Some(t) = state.scratch.as_ref() {
            return Ok(t);
        }
        let same = |t: &&Tier| t.width == source.width && t.height == source.height;
        state
            .tiers
            .small
            .iter()
            .rev()
            .find(same)
            .or_else(|| state.tiers.large_display.as_ref().filter(same))
            .ok_or(GpuError::Invalid("render tier missing"))
    }

    /// Float path: encoded output as an `RgbImage` plus the histogram (export, parity).
    pub fn render(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
    ) -> Result<RenderResult, GpuError> {
        self.fits_render(source.width, source.height)?;
        let mut state = self.lock();
        let device = self.ctx.device();
        let result = scoped(device, || {
            let (encoder, _) =
                self.encode_chain(&mut state, source, params, opts, Target::Float)?;
            let index = self.ctx.queue().submit(Some(encoder.finish()));
            let tier = Self::rendered_tier(&state, source)?;
            let Some((_, staging)) = tier.encoded_f.as_ref() else {
                return Err(GpuError::Invalid("float staging missing"));
            };
            let bytes = source.pixel_count() * 12;
            let mapped = map_sync(
                device,
                index,
                &[(staging, bytes), (&state.shared.hist_staging, HIST_BYTES)],
            )?;
            let (Some(pixels), Some(hist)) = (mapped.first(), mapped.get(1)) else {
                return Err(GpuError::Invalid("staging maps missing"));
            };
            let image = {
                let view = pixels.view()?;
                RgbImage::new_with(tier.width as usize, tier.height as usize, |buf| {
                    bytemuck::cast_slice_mut::<f32, u8>(buf)
                        .copy_from_slice(&view[..bytes as usize]);
                })?
            };
            let histogram = read_histogram(hist)?;
            Ok(RenderResult { image, histogram })
        });
        // Large float renders never retain their intermediates.
        state.scratch = None;
        result
    }

    /// Display path, allocating: packed RGBA8 (alpha 255) + histogram.
    pub fn render_display(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
    ) -> Result<DisplayResult, GpuError> {
        let len = source.rgba8_len();
        let mut rgba8 = Vec::with_capacity(len);
        let histogram = self.display_readback(source, params, opts, |view| {
            rgba8.extend_from_slice(view);
        })?;
        Ok(DisplayResult {
            rgba8,
            width: source.width,
            height: source.height,
            histogram,
        })
    }

    /// Display path, zero-allocation: writes the packed RGBA8 frame (`source.rgba8_len()`
    /// bytes, row-major, tightly packed) straight from the mapped staging buffer into `dest`.
    /// Returns `Ok(None)` without rendering when `dest` is too small (size it from
    /// `source.rgba8_len()` and retry); `Ok(Some(histogram))` once written.
    pub fn render_display_into(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
        dest: &mut [u8],
    ) -> Result<Option<Histogram>, GpuError> {
        let len = source.rgba8_len();
        let Some(dest) = dest.get_mut(..len) else {
            return Ok(None);
        };
        self.display_readback(source, params, opts, |view| dest.copy_from_slice(view))
            .map(Some)
    }

    fn display_readback(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
        sink: impl FnOnce(&[u8]),
    ) -> Result<Histogram, GpuError> {
        self.fits_render(source.width, source.height)?;
        let mut state = self.lock();
        let device = self.ctx.device();
        scoped(device, || {
            let (encoder, _) = self.encode_chain(&mut state, source, params, opts, Target::U8)?;
            let index = self.ctx.queue().submit(Some(encoder.finish()));
            let tier = Self::rendered_tier(&state, source)?;
            let Some((_, staging)) = tier.encoded_u8.as_ref() else {
                return Err(GpuError::Invalid("display staging missing"));
            };
            let bytes = source.pixel_count() * 4;
            let mapped = map_sync(
                device,
                index,
                &[(staging, bytes), (&state.shared.hist_staging, HIST_BYTES)],
            )?;
            let (Some(pixels), Some(hist)) = (mapped.first(), mapped.get(1)) else {
                return Err(GpuError::Invalid("staging maps missing"));
            };
            sink(&pixels.view()?[..bytes as usize]);
            read_histogram(hist)
        })
    }

    /// Display path with no pixel readback: the packed RGBA8 frame is copied on the GPU into
    /// `texture` — an **`Rgba8Unorm`** texture (not `*Srgb`: the bytes are already
    /// display-encoded by the chain, Adobe-RGB or, with `srgb_encode`, sRGB, and the caller
    /// samples them as-is; an sRGB view would decode them a second time) of exactly the
    /// source's size with `COPY_DST` usage — e.g. one registered with egui-wgpu. Any other
    /// format is refused with [`GpuError::TextureFormat`]. The kernel writes rows at the
    /// 256-byte-aligned pitch `copy_buffer_to_texture` requires, so no repack happens
    /// anywhere. Returns the histogram (its 4 KB readback is the only CPU wait).
    pub fn render_display_to_texture(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
        texture: &wgpu::Texture,
    ) -> Result<Histogram, GpuError> {
        if texture.width() != source.width || texture.height() != source.height {
            return Err(GpuError::Invalid("texture size must equal the source size"));
        }
        if texture.format() != wgpu::TextureFormat::Rgba8Unorm {
            return Err(GpuError::TextureFormat(texture.format()));
        }
        if !texture.usage().contains(wgpu::TextureUsages::COPY_DST) {
            return Err(GpuError::Invalid("texture needs COPY_DST usage"));
        }
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes_per_row = (source.width * 4).div_ceil(align) * align;
        let pitch = bytes_per_row / 4;
        self.fits_render(source.width, source.height)?;
        self.check_size(u64::from(bytes_per_row) * u64::from(source.height))?;

        let mut state = self.lock();
        let device = self.ctx.device();
        scoped(device, || {
            let (mut encoder, _) =
                self.encode_chain(&mut state, source, params, opts, Target::U8Padded(pitch))?;
            let tier = Self::rendered_tier(&state, source)?;
            let Some((_, padded)) = tier.padded_u8.as_ref() else {
                return Err(GpuError::Invalid("padded buffer missing"));
            };
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: padded,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(source.height),
                    },
                },
                texture.as_image_copy(),
                wgpu::Extent3d {
                    width: source.width,
                    height: source.height,
                    depth_or_array_layers: 1,
                },
            );
            let index = self.ctx.queue().submit(Some(encoder.finish()));
            let mapped = map_sync(device, index, &[(&state.shared.hist_staging, HIST_BYTES)])?;
            let Some(hist) = mapped.first() else {
                return Err(GpuError::Invalid("histogram map missing"));
            };
            read_histogram(hist)
        })
    }
}

fn dims_u32(width: usize, height: usize) -> Result<(u32, u32), GpuError> {
    let w = u32::try_from(width).map_err(|_| GpuError::Invalid("width exceeds u32"))?;
    let h = u32::try_from(height).map_err(|_| GpuError::Invalid("height exceeds u32"))?;
    Ok((w, h))
}

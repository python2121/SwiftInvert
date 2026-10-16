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
//! device-local and never read by the CPU; only the staging buffers are mapped. Intermediates
//! are cached per (width, height) up to [`CACHE_PIXEL_BUDGET`] pixels — export sizes are
//! dropped after use. Bind groups are cached with the tier and rebuilt only when the source or
//! mask buffer identity changes, uniforms are `write_buffer`s into persistent buffers, so the
//! slider path allocates nothing on the host.
//!
//! Renders are serialized by an internal mutex (the Swift contract: intermediates are shared
//! per size, so two renders at one size must not interleave).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use negkit::lab::ColorPop;
use negkit::{RenderParams, RgbImage};

use crate::uniforms::{self, FLAG_SRGB, LEVELS_BUFFER_FLOATS, Push};
use crate::{GpuContext, GpuError};

/// Kernel workgroup size (`@workgroup_size(256)` in the WGSL).
const WORKGROUP: u32 = 256;
/// Intermediates for renders up to this many pixels stay cached; larger (export) tiers are
/// released after the render.
pub const CACHE_PIXEL_BUDGET: u64 = 4_000_000;
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

/// A linear RGB source image uploaded once and rendered many times (the session keeps one per
/// tier; slider drags re-upload nothing).
#[derive(Debug)]
pub struct SourceBuffer {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    id: u64,
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
}

/// An uploaded Contrast Mask plane (single-channel floats on the analysis grid).
#[derive(Debug)]
pub struct MaskBuffer {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    id: u64,
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

struct State {
    kernels: Kernels,
    shared: Shared,
    tiers: HashMap<(u32, u32), Tier>,
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

/// Wait for `index` and confirm every `map_async` issued on `slices` resolved.
fn map_sync(
    device: &wgpu::Device,
    index: wgpu::SubmissionIndex,
    slices: &[wgpu::BufferSlice<'_>],
) -> Result<(), GpuError> {
    let (tx, rx) = std::sync::mpsc::channel();
    for slice in slices {
        let tx = tx.clone();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // A closed receiver only means the caller already gave up.
            let _ = tx.send(result);
        });
    }
    drop(tx);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map_err(|e| GpuError::Readback(e.to_string()))?;
    for _ in slices {
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
    Ok(())
}

fn read_histogram(staging: &wgpu::Buffer) -> Result<Histogram, GpuError> {
    let view = staging
        .slice(..HIST_BYTES)
        .get_mapped_range()
        .map_err(|e| GpuError::Readback(e.to_string()))?;
    let mut bins = [0u32; 1024];
    bytemuck::cast_slice_mut::<u32, u8>(&mut bins).copy_from_slice(&view[..HIST_BYTES as usize]);
    drop(view);
    staging.unmap();
    Ok(bins)
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
                tiers: HashMap::new(),
            }),
        })
    }

    /// The context this pipeline renders on.
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    fn check_size(&self, bytes: u64) -> Result<(), GpuError> {
        let limits = self.ctx.limits();
        let limit = limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size);
        if bytes > limit {
            return Err(GpuError::TooLarge { bytes, limit });
        }
        Ok(())
    }

    /// One memcpy into a device buffer created mapped (no intermediate staging).
    fn upload_floats(&self, label: &str, data: &[f32]) -> Result<wgpu::Buffer, GpuError> {
        let bytes = (data.len() * 4) as u64;
        self.check_size(bytes)?;
        let buffer = self.ctx.device().create_buffer(&wgpu::BufferDescriptor {
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
        })
    }

    /// Drop every cached intermediate (e.g. when the app loses focus or a session closes).
    pub fn release_cached(&self) {
        self.lock().tiers.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ensure the tier for `source`'s size exists with the buffers and bind groups `target`
    /// needs, and that its source/mask bind groups match the buffers handed in.
    fn prepare_tier<'s>(
        &self,
        kernels: &Kernels,
        shared: &Shared,
        tiers: &'s mut HashMap<(u32, u32), Tier>,
        source: &SourceBuffer,
        mask: Option<&MaskBuffer>,
        target: Target,
    ) -> Result<&'s mut Tier, GpuError> {
        let device = self.ctx.device();
        let key = (source.width, source.height);
        let n = source.pixel_count();
        let float_bytes = n * 12;
        if let std::collections::hash_map::Entry::Vacant(slot) = tiers.entry(key) {
            self.check_size(float_bytes)?;
            let storage = wgpu::BufferUsages::STORAGE;
            let normalized = new_buffer(device, "normalized", float_bytes, storage);
            let linear = new_buffer(device, "linear", float_bytes, storage);
            let k = kernels;
            let sh = shared;
            let bg_pop = bind_group(
                device,
                "color_pop",
                &k.color_pop.layout,
                &[
                    (0, &linear),
                    (1, &normalized),
                    (3, &sh.curve_u),
                    (4, &sh.push),
                ],
            );
            let bg_hist = [&linear, &normalized].map(|content| {
                bind_group(
                    device,
                    "histogram",
                    &k.histogram.layout,
                    &[(0, content), (4, &sh.push), (5, &sh.levels), (7, &sh.hist)],
                )
            });
            slot.insert(Tier {
                width: source.width,
                height: source.height,
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
            });
        }
        let Some(tier) = tiers.get_mut(&key) else {
            return Err(GpuError::Invalid("render tier vanished"));
        };

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

        let State {
            kernels,
            shared,
            tiers,
        } = state;
        let tier = self.prepare_tier(kernels, shared, tiers, source, mask, target)?;
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

    /// Release the tier after an over-budget (export) render.
    fn trim(state: &mut State, source: &SourceBuffer) {
        if source.pixel_count() > CACHE_PIXEL_BUDGET {
            state.tiers.remove(&(source.width, source.height));
        }
    }

    /// Float path: encoded output as an `RgbImage` plus the histogram (export, parity).
    pub fn render(
        &self,
        source: &SourceBuffer,
        params: &RenderParams,
        opts: RenderOptions<'_>,
    ) -> Result<RenderResult, GpuError> {
        let mut state = self.lock();
        let (encoder, _) = self.encode_chain(&mut state, source, params, opts, Target::Float)?;
        let index = self.ctx.queue().submit(Some(encoder.finish()));
        let result = (|| {
            let tier = state
                .tiers
                .get(&(source.width, source.height))
                .ok_or(GpuError::Invalid("render tier missing"))?;
            let Some((_, staging)) = tier.encoded_f.as_ref() else {
                return Err(GpuError::Invalid("float staging missing"));
            };
            let bytes = source.pixel_count() * 12;
            map_sync(
                self.ctx.device(),
                index,
                &[staging.slice(..bytes), state.shared.hist_staging.slice(..)],
            )?;
            let image = {
                let view = staging
                    .slice(..bytes)
                    .get_mapped_range()
                    .map_err(|e| GpuError::Readback(e.to_string()))?;
                RgbImage::new_with(tier.width as usize, tier.height as usize, |buf| {
                    bytemuck::cast_slice_mut::<f32, u8>(buf)
                        .copy_from_slice(&view[..bytes as usize]);
                })?
            };
            staging.unmap();
            let histogram = read_histogram(&state.shared.hist_staging)?;
            Ok(RenderResult { image, histogram })
        })();
        Self::trim(&mut state, source);
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
        let mut state = self.lock();
        let (encoder, _) = self.encode_chain(&mut state, source, params, opts, Target::U8)?;
        let index = self.ctx.queue().submit(Some(encoder.finish()));
        let result = (|| {
            let tier = state
                .tiers
                .get(&(source.width, source.height))
                .ok_or(GpuError::Invalid("render tier missing"))?;
            let Some((_, staging)) = tier.encoded_u8.as_ref() else {
                return Err(GpuError::Invalid("display staging missing"));
            };
            let bytes = source.pixel_count() * 4;
            map_sync(
                self.ctx.device(),
                index,
                &[staging.slice(..bytes), state.shared.hist_staging.slice(..)],
            )?;
            {
                let view = staging
                    .slice(..bytes)
                    .get_mapped_range()
                    .map_err(|e| GpuError::Readback(e.to_string()))?;
                sink(&view[..bytes as usize]);
            }
            staging.unmap();
            read_histogram(&state.shared.hist_staging)
        })();
        Self::trim(&mut state, source);
        result
    }

    /// Display path with no pixel readback: the packed RGBA8 frame is copied on the GPU into
    /// `texture` (an `Rgba8Unorm`/`Rgba8UnormSrgb` texture of exactly the source's size with
    /// `COPY_DST` usage — e.g. one registered with egui-wgpu). The kernel writes rows at the
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
        if !matches!(
            texture.format(),
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb
        ) {
            return Err(GpuError::Invalid(
                "texture must be Rgba8Unorm or Rgba8UnormSrgb",
            ));
        }
        if !texture.usage().contains(wgpu::TextureUsages::COPY_DST) {
            return Err(GpuError::Invalid("texture needs COPY_DST usage"));
        }
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes_per_row = (source.width * 4).div_ceil(align) * align;
        let pitch = bytes_per_row / 4;

        let mut state = self.lock();
        let (mut encoder, _) =
            self.encode_chain(&mut state, source, params, opts, Target::U8Padded(pitch))?;
        let result = (|| {
            let tier = state
                .tiers
                .get(&(source.width, source.height))
                .ok_or(GpuError::Invalid("render tier missing"))?;
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
            map_sync(
                self.ctx.device(),
                index,
                &[state.shared.hist_staging.slice(..)],
            )?;
            read_histogram(&state.shared.hist_staging)
        })();
        Self::trim(&mut state, source);
        result
    }
}

fn dims_u32(width: usize, height: usize) -> Result<(u32, u32), GpuError> {
    let w = u32::try_from(width).map_err(|_| GpuError::Invalid("width exceeds u32"))?;
    let h = u32::try_from(height).map_err(|_| GpuError::Invalid("height exceeds u32"))?;
    Ok((w, h))
}

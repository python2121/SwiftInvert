//! The render seam — `RenderPipeline.render` / `renderDisplay` in
//! `Sources/MetalRenderKit/RenderPipeline.swift` as a trait, with the CPU reference chain as its
//! first implementation (PLAN §2 "CPU fallback is the oracle, not a second implementation").
//!
//! The session hands a renderer an uploaded-once [`RenderSource`] (the linear, oriented,
//! cropped frame — slider ticks reuse it) plus the derived [`negkit::RenderParams`] and gets
//! back either the ENCODED float image ([`Renderer::render`]: export, tests, parity) or packed
//! RGBA8 ([`Renderer::render_display`]: the canvas) with the 4 × 256 histogram. `RenderSource::id`
//! is unique per uploaded buffer, so a GPU renderer can key its device-side copies on it and
//! never re-upload for a slider change — the Metal/Vulkan "upload once per (image, crop)"
//! discipline expressed without the renderer knowing the session's cache keys.

use std::sync::Arc;

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
    /// The Contrast Mask plane when the mask is on (`mask_val_scale` non-zero).
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

/// Encoded float → packed RGBA8 with the GPU's rgba8unorm rounding (nearest, alpha 255).
/// Built in parallel straight into its final allocation — no zero-fill-then-overwrite.
pub fn rgba8_from_encoded(image: &RgbImage) -> Vec<u8> {
    let px: Vec<[u8; 4]> = image
        .data
        .par_chunks_exact(3)
        .map(|p| [quantize(p[0]), quantize(p[1]), quantize(p[2]), 255])
        .collect();
    px.into_flattened()
}

#[inline]
fn quantize(v: f32) -> u8 {
    // Clamped to [0, 255.5) before the truncating cast, so it is exact and never saturates.
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
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
}

/// The renderer this build defaults to. Honours `NEG_GPU=off` trivially today (there is only
/// the CPU chain); the app layer substitutes a GPU renderer behind the same trait.
pub fn default_renderer() -> Arc<dyn Renderer> {
    if std::env::var_os("NEG_GPU").is_some_and(|v| v == "off") {
        log::info!("NEG_GPU=off: CPU renderer");
    }
    Arc::new(CpuRenderer)
}

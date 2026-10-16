//! The per-image cache tower — `Sources/SwiftInvert/ImageSession.swift` (session-spec §3–§4).
//!
//! decode → orient → analyze (CPU, on the 1536 px proxy) → derive (µs) → render → display/export.
//! Slider edits re-run ONLY derive + render; every heavier tier is keyed and rebuilt only when its
//! key changes:
//!
//! | tier | key |
//! |---|---|
//! | preview decode (proxy + the free medium buffer) | once per session |
//! | full decode | lazily on the first `.full` render; released per the HQ rules |
//! | meter image (orientation-only + the angle a region was drawn at) | `MeterKey {rotation, flip, meter_angle}` — a change also clears prepared + analysis |
//! | render preview (fine-rotated) | `OrientKey {rotation, flip, fine_rotation}` — a change clears the proxy sources |
//! | `Prepared` | `PreparedKey {analysis_rect, crop_rect}` — a change clears the analysis |
//! | `ExposureAnalysis` | `None` → `finalize` (offset-independent: WP/BP drags re-run nothing) |
//! | render source | per (tier, orientation, crop): `oriented(+fine rotation, inscribed)` THEN `cropped` |
//! | mask plane | `MaskKey {orient, uncropped, spacer, meter, prep}`, always from the PROXY |
//!
//! **Fine-rotation invariant.** Analysis reads the orientation-only image, never the straightened
//! one: the inscribed auto-crop changes with the angle and re-metering it would drift the
//! conversion as the user rotates. Mac nuance kept: with a manual analysis region the meter image
//! is built at the angle the region was DRAWN (`analysis_rect_fine_rotation`); the bridge's
//! always-0° is the `meter_angle == 0` special case.
//!
//! **`uncropped` widens only what is RENDERED** (source + mask plane); the meter stays scoped by
//! the settings' own `crop_rect` (the Qt lesson: a settings substitution re-metered the rebate).
//!
//! **What-you-see export**: `export_render` decodes full but takes its analysis — and its mask
//! plane — from the proxy, so the file matches the canvas.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use negkit::constants::K;
use negkit::derive::derive_render_params;
use negkit::kernel::{self, DEFAULT_ANALYSIS_BUFFER, ExposureAnalysis, Prepared};
use negkit::rect::{NormalizedRect, PixelRoi};
use negkit::reference::Histogram256;
use negkit::{ExposureSettings, RgbImage, curve_logic};
use negraw::RawBackend;

use crate::densitometer::NegativeCharacter;
use crate::geometry;
use crate::mask::{self, MaskPlane};
use crate::renderer::{RenderOptions, RenderSource, Renderer};
use crate::{Result, SessionError};

/// Long edge of the analysis proxy (ImageSession.swift:265).
pub const PROXY_LONG_EDGE: usize = 1536;

/// Above this the retained half-size preview buffer costs more than it is worth (12 B/px → 240 MB):
/// a 24 MP Bayer body's half-size decode is 6 MP and qualifies; an X-Trans preview decodes full
/// size and does not — those bodies get no medium tier (ImageSession.swift:216).
pub const MEDIUM_TIER_PIXEL_BUDGET: usize = 20_000_000;

/// Fine rotations at or below this (degrees) are not applied — mirrors `RgbImage::oriented`.
pub const FINE_ROTATION_EPSILON: f64 = 0.005;

/// Which source a display render reads from. Analysis ALWAYS runs on the proxy whatever this
/// says — tiers differ only in pixels on screen, so switching can never move the conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderTier {
    /// The 1536 px analysis proxy.
    Proxy,
    /// The best tier that costs no decode: the retained half-size buffer when this body has one,
    /// else `Full`.
    MediumIfFree,
    /// True full resolution with the export demosaic — what export produces.
    Full,
}

/// The canvas tools (`AppModel.ToolMode`). Both render the frame OUTSIDE the crop; the crop tool
/// also shows the UNROTATED frame (the box lives in rotated space over it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToolMode {
    #[default]
    None,
    AnalysisRegion,
    Crop,
}

/// What a render should hand back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputKind {
    /// Packed RGBA8 (the canvas).
    #[default]
    Rgba8,
    /// The encoded float image (export, tests).
    Float,
}

/// One render's request (`ImageSession.render(settings:uncropped:tier:retainFull:)` + the
/// bridge's `srgb_display`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderRequest {
    pub tier: RenderTier,
    /// Render the whole frame regardless of `crop_rect` (tool modes). Never touches the meter.
    pub uncropped: bool,
    /// Keep the full tier alive across a proxy/medium render (HQ auto/on); off → it is freed.
    pub retain_full: bool,
    /// sRGB re-expression in-kernel (unmanaged canvases).
    pub srgb: bool,
    pub output: OutputKind,
    pub compute_histogram: bool,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            tier: RenderTier::Proxy,
            uncropped: false,
            retain_full: false,
            srgb: false,
            output: OutputKind::Rgba8,
            compute_histogram: true,
        }
    }
}

/// Rendered pixels.
#[derive(Clone, Debug)]
pub enum Frame {
    /// Tightly packed RGBA8, alpha 255.
    Rgba8(Vec<u8>),
    /// The encoded float image.
    Float(RgbImage),
}

impl Frame {
    /// The RGBA8 bytes when this is a display frame.
    pub fn rgba8(&self) -> Option<&[u8]> {
        match self {
            Frame::Rgba8(b) => Some(b),
            Frame::Float(_) => None,
        }
    }

    /// The float image when this is a float frame.
    pub fn image(&self) -> Option<&RgbImage> {
        match self {
            Frame::Rgba8(_) => None,
            Frame::Float(i) => Some(i),
        }
    }
}

/// `ImageSession.RenderOutput`.
#[derive(Clone, Debug)]
pub struct RenderOutput {
    pub frame: Frame,
    pub width: usize,
    pub height: usize,
    /// 4 × 256 raw counts (R, G, B, luma) when requested. Consumers draw SHAPE only.
    pub histogram: Option<Histogram256>,
    /// Orientation-only frame dims of the PROXY — the coordinate base for crop geometry.
    pub frame_size: (usize, usize),
    /// Width ÷ height the canvas should LAY OUT at (continuous, tier-independent).
    pub display_aspect: f64,
    /// Where this bitmap really sits inside the ideal display rect (nominally the full rect).
    pub content_window: NormalizedRect,
    /// Pre-offset luminance density range of the negative (the character diagnostic's input).
    pub density_range: f64,
    /// The tier that was actually rendered (`MediumIfFree` resolved).
    pub tier: RenderTier,
}

/// `si_session_info` + the negative-character row.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    /// Orientation-only frame dims of the proxy.
    pub width: usize,
    pub height: usize,
    /// Pre-offset luminance density range.
    pub density_range: f64,
    /// `CurveLogic.defaultGradeRange` — what "normal" means for the character row.
    pub default_grade_range: f64,
    pub anchor: f64,
    pub textural_range: f64,
    pub character: Option<NegativeCharacter>,
    /// Neutral-axis confidence (gates Auto cast removal); `None` = no neutral axis found.
    pub cast_confidence: Option<f64>,
}

/// A zone pin's frozen sample (`ImageSession.sampleZonePin`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZonePinSample {
    /// Normalized-log value per channel (patch mean).
    pub val_rgb: [f64; 3],
    /// Rec.709 luma of `val_rgb`.
    pub val_luma: f64,
    /// The Contrast Mask plane under the pin (0 when the mask is off).
    pub mask_val: f64,
}

/// Work counters — the cache-invariant tests and `negcli bench` read these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub preview_decodes: u32,
    pub full_decodes: u32,
    /// Orientation passes (meter image + render preview + display tiers).
    pub orients: u32,
    pub prepares: u32,
    pub finalizes: u32,
    /// Render sources built (crops / uploads).
    pub source_builds: u32,
    pub mask_builds: u32,
    pub renders: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MeterKey {
    rotation: i32,
    flip_horizontal: bool,
    /// 0 unless a manual analysis region exists — then the angle it was drawn at.
    meter_angle: f64,
}

impl MeterKey {
    fn of(s: &ExposureSettings) -> Self {
        Self {
            rotation: s.rotation,
            flip_horizontal: s.flip_horizontal,
            meter_angle: meter_angle(s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OrientKey {
    rotation: i32,
    flip_horizontal: bool,
    fine_rotation: f64,
}

impl OrientKey {
    fn of(s: &ExposureSettings) -> Self {
        Self {
            rotation: s.rotation,
            flip_horizontal: s.flip_horizontal,
            fine_rotation: s.fine_rotation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreparedKey {
    analysis_rect: Option<NormalizedRect>,
    crop_rect: Option<NormalizedRect>,
}

impl PreparedKey {
    fn of(s: &ExposureSettings) -> Self {
        Self {
            analysis_rect: s.analysis_rect,
            crop_rect: s.crop_rect,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MaskKey {
    orient: OrientKey,
    uncropped: bool,
    spacer: f64,
    meter: MeterKey,
    prep: PreparedKey,
}

struct MaskEntry {
    key: MaskKey,
    plane: Arc<MaskPlane>,
}

/// The "uploaded" render sources for one tier: the whole oriented image, or the current crop.
#[derive(Default)]
struct SourceSlots {
    full: Option<RenderSource>,
    cropped: Option<RenderSource>,
    cropped_rect: Option<NormalizedRect>,
}

impl SourceSlots {
    fn clear(&mut self) {
        self.full = None;
        self.cropped = None;
        self.cropped_rect = None;
    }
}

/// A display tier above the proxy (`ImageSession.DisplayTier`): decoded base → oriented →
/// sources for the current crop, keyed exactly like the proxy tower.
#[derive(Default)]
struct DisplayTier {
    base: Option<Arc<RgbImage>>,
    oriented: Option<Arc<RgbImage>>,
    orient_key: Option<OrientKey>,
    slots: SourceSlots,
}

impl DisplayTier {
    fn invalidate_orientation(&mut self) {
        self.oriented = None;
        self.orient_key = None;
        self.slots.clear();
    }

    fn clear(&mut self) {
        self.base = None;
        self.invalidate_orientation();
    }
}

/// Where the full-resolution decode (and the thumbnail) come from: the RAW backend for real
/// files, a canned image for the synthetic test sessions.
trait FrameSource: Send + Sync {
    fn decode_full(&self) -> Result<RgbImage>;
    fn thumbnail(&self) -> Result<Option<Vec<u8>>>;
}

struct BackendSource {
    backend: Arc<dyn RawBackend>,
    path: PathBuf,
}

impl FrameSource for BackendSource {
    fn decode_full(&self) -> Result<RgbImage> {
        Ok(self.backend.decode_full(&self.path)?.image)
    }

    fn thumbnail(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.backend.thumbnail(&self.path)?)
    }
}

struct SyntheticSource {
    full: Arc<RgbImage>,
}

impl FrameSource for SyntheticSource {
    fn decode_full(&self) -> Result<RgbImage> {
        Ok((*self.full).clone())
    }

    fn thumbnail(&self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

/// `meterAngle(settings)`: the angle the meter image is built at — a manual analysis region pins
/// the angle it was drawn at; otherwise 0.
pub fn meter_angle(settings: &ExposureSettings) -> f64 {
    if settings.analysis_rect.is_some() {
        settings.analysis_rect_fine_rotation
    } else {
        0.0
    }
}

/// Whether the render input may simply ALIAS the meter image (`ImageSession.aliasesMeterPreview`):
/// BOTH angles must be flat. The meter image is built at `meter_angle` (the angle a region was
/// drawn at), not the straighten angle, so aliasing on `fine_rotation` alone would hand the
/// renderer a tilted, inscribed picture every control reads as zero.
pub fn aliases_meter_preview(fine_rotation: f64, meter_angle: f64) -> bool {
    fine_rotation.abs() <= FINE_ROTATION_EPSILON && meter_angle.abs() <= FINE_ROTATION_EPSILON
}

/// The View-Original hold: stock settings with the CURRENT geometry that defines the frame
/// (`AppModel.baselineSettings`) — compare must show the same picture, not the same crop-less one.
pub fn baseline_settings(current: &ExposureSettings) -> ExposureSettings {
    ExposureSettings {
        analysis_rect: current.analysis_rect,
        analysis_rect_fine_rotation: current.analysis_rect_fine_rotation,
        crop_rect: current.crop_rect,
        rotation: current.rotation,
        flip_horizontal: current.flip_horizontal,
        ..ExposureSettings::default()
    }
}

/// Orientation-only frame dims for a base of `(w, h)` (axes swap for 90°/270°).
fn oriented_frame_size(base: (usize, usize), rotation: i32) -> (usize, usize) {
    if rotation.rem_euclid(360) % 180 != 0 {
        (base.1, base.0)
    } else {
        base
    }
}

/// The straighten angle the render chain actually applies, in radians.
fn applied_radians(fine_rotation: f64) -> f64 {
    if fine_rotation.abs() > FINE_ROTATION_EPSILON {
        fine_rotation.to_radians()
    } else {
        0.0
    }
}

/// `base.oriented(...)` as an `Arc`: a borrowed (unchanged) result shares the base's buffer.
fn orient_arc(base: &Arc<RgbImage>, rotation: i32, flip: bool, fine_rotation: f64) -> Arc<RgbImage> {
    match base.oriented_with_fine_rotation(rotation, flip, fine_rotation) {
        Cow::Borrowed(_) => Arc::clone(base),
        Cow::Owned(img) => Arc::new(img),
    }
}

/// `image.cropped(rect)` as an `Arc` (the image itself when the rect is degenerate).
fn crop_arc(image: &Arc<RgbImage>, rect: &NormalizedRect) -> Arc<RgbImage> {
    match image.cropped(rect) {
        Cow::Borrowed(_) => Arc::clone(image),
        Cow::Owned(img) => Arc::new(img),
    }
}

/// Per-image engine state: the cache tower + settings + render entry points.
pub struct Session {
    path: PathBuf,
    source: Box<dyn FrameSource>,
    renderer: Arc<dyn Renderer>,
    settings: ExposureSettings,
    /// As decoded (EXIF baked, no user orientation), downsampled to the proxy.
    base_preview: Arc<RgbImage>,
    /// Orientation-only (+ pinned region angle) — the analysis input.
    meter_preview: Option<Arc<RgbImage>>,
    meter_key: Option<MeterKey>,
    /// User-oriented incl. fine rotation — the proxy render input.
    preview: Option<Arc<RgbImage>>,
    orient_key: Option<OrientKey>,
    prepared: Option<Prepared>,
    prepared_key: Option<PreparedKey>,
    analysis: Option<ExposureAnalysis>,
    proxy_slots: SourceSlots,
    medium: DisplayTier,
    full: DisplayTier,
    mask: Option<MaskEntry>,
    next_source_id: u64,
    stats: SessionStats,
}

impl Session {
    /// Decode the preview tier of `path` and build the proxy tower. ~1 s on a 24 MP body with the
    /// pure-Rust decoder; call it off the UI thread. The medium tier is the native half-size
    /// buffer the preview decode already produced, kept when it is under the budget and actually
    /// larger than the proxy.
    pub fn open(
        path: impl AsRef<Path>,
        backend: Arc<dyn RawBackend>,
        renderer: Arc<dyn Renderer>,
    ) -> Result<Session> {
        let path = path.as_ref().to_path_buf();
        let native = backend.decode_preview(&path)?.image;
        let source = Box::new(BackendSource {
            backend,
            path: path.clone(),
        });
        let mut session = Self::from_native_preview(path, native, source, renderer);
        session.stats.preview_decodes = 1;
        Ok(session)
    }

    /// Test-only: a session over a canned native preview (and optional full-resolution frame,
    /// else the preview stands in) — no decoder, no file. `label` is reported as the path.
    #[doc(hidden)]
    pub fn synthetic(
        label: impl AsRef<Path>,
        native_preview: RgbImage,
        full: Option<RgbImage>,
        renderer: Arc<dyn Renderer>,
    ) -> Session {
        let full = Arc::new(full.unwrap_or_else(|| native_preview.clone()));
        Self::from_native_preview(
            label.as_ref().to_path_buf(),
            native_preview,
            Box::new(SyntheticSource { full }),
            renderer,
        )
    }

    fn from_native_preview(
        path: PathBuf,
        native: RgbImage,
        source: Box<dyn FrameSource>,
        renderer: Arc<dyn Renderer>,
    ) -> Session {
        // Decode at the sensor's native preview size and downsample HERE, so the buffer the
        // decoder already produced can be kept as the medium tier; the proxy is still ONE
        // downsample from it, so the analysis input is byte-identical to capping in the decoder.
        let native = Arc::new(native);
        let base_preview = match native.downsampled(PROXY_LONG_EDGE) {
            Cow::Borrowed(_) => Arc::clone(&native),
            Cow::Owned(img) => Arc::new(img),
        };
        let mut medium = DisplayTier::default();
        if native.pixel_count() <= MEDIUM_TIER_PIXEL_BUDGET && native.width > base_preview.width {
            medium.base = Some(native);
        }
        Session {
            path,
            source,
            renderer,
            settings: ExposureSettings::default(),
            base_preview,
            meter_preview: None,
            meter_key: None,
            preview: None,
            orient_key: None,
            prepared: None,
            prepared_key: None,
            analysis: None,
            proxy_slots: SourceSlots::default(),
            medium,
            full: DisplayTier::default(),
            mask: None,
            next_source_id: 1,
            stats: SessionStats::default(),
        }
    }

    // ───────────────────────────── accessors ─────────────────────────────

    /// The source file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings every parameterless render uses.
    pub fn settings(&self) -> &ExposureSettings {
        &self.settings
    }

    /// Replace the settings. Cheap: nothing is invalidated here — every tier re-validates its
    /// key on the next call, which is what makes a returning session unable to serve stale pixels.
    pub fn set_settings(&mut self, settings: ExposureSettings) {
        self.settings = settings;
    }

    /// Mutate the settings in place.
    pub fn update_settings(&mut self, f: impl FnOnce(&mut ExposureSettings)) {
        f(&mut self.settings);
    }

    /// Commit an analysis region drawn on the CURRENT display: stores the rect AND the angle it
    /// was drawn at (`AppModel.commitSelection`), so the meter reads what the user pointed at.
    pub fn commit_analysis_region(&mut self, rect: Option<NormalizedRect>) {
        self.settings.analysis_rect = rect;
        self.settings.analysis_rect_fine_rotation = if rect.is_some() {
            self.settings.fine_rotation
        } else {
            0.0
        };
    }

    /// The renderer in use.
    pub fn renderer(&self) -> &Arc<dyn Renderer> {
        &self.renderer
    }

    /// Proxy dims as decoded (before user orientation).
    pub fn proxy_size(&self) -> (usize, usize) {
        (self.base_preview.width, self.base_preview.height)
    }

    /// Orientation-only frame dims of the proxy for the current settings.
    pub fn frame_size(&self) -> (usize, usize) {
        oriented_frame_size(self.proxy_size(), self.settings.rotation)
    }

    /// Whether this body kept a medium tier.
    pub fn has_medium_tier(&self) -> bool {
        self.medium.base.is_some()
    }

    /// Whether the full-resolution tier is currently decoded.
    pub fn has_full_tier(&self) -> bool {
        self.full.base.is_some()
    }

    /// Work counters.
    pub fn stats(&self) -> SessionStats {
        self.stats
    }

    /// The embedded camera JPEG, if the file carries one.
    pub fn thumbnail(&self) -> Result<Option<Vec<u8>>> {
        self.source.thumbnail()
    }

    /// Bytes held by every cached image / plane, each shared buffer counted once.
    pub fn bytes_resident(&self) -> usize {
        let mut seen: HashSet<*const RgbImage> = HashSet::new();
        let mut total = 0usize;
        let mut count = |img: &Arc<RgbImage>| {
            if seen.insert(Arc::as_ptr(img)) {
                total += img.data.len() * std::mem::size_of::<f32>();
            }
        };
        count(&self.base_preview);
        for img in [&self.meter_preview, &self.preview].into_iter().flatten() {
            count(img);
        }
        for slots in [&self.proxy_slots, &self.medium.slots, &self.full.slots] {
            for src in [&slots.full, &slots.cropped].into_iter().flatten() {
                count(&src.image);
            }
        }
        for tier in [&self.medium, &self.full] {
            for img in [&tier.base, &tier.oriented].into_iter().flatten() {
                count(img);
            }
        }
        if let Some(p) = &self.prepared {
            total += p.grid.data.len() * std::mem::size_of::<f32>();
        }
        if let Some(m) = &self.mask {
            total += m.plane.bytes();
        }
        total
    }

    /// Drop the full AND medium tiers while keeping the proxy tower warm (`releaseEnhancedTiers`):
    /// for sessions the LRU retains but isn't showing, and when HQ is switched off.
    pub fn release_enhanced_tiers(&mut self) {
        self.full.clear();
        self.medium.clear();
    }

    /// Drop only the full tier (`clearFull`).
    pub fn release_full_tier(&mut self) {
        self.full.clear();
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_source_id;
        self.next_source_id = self.next_source_id.wrapping_add(1);
        id
    }

    // ───────────────────────────── the tower ─────────────────────────────

    /// Decode + orient + prepare once per key state; finalize when the analysis is missing.
    /// Returns the proxy render input and the analysis.
    fn prepare(&mut self, settings: &ExposureSettings) -> Result<(Arc<RgbImage>, ExposureAnalysis)> {
        // Analysis ignores the CURRENT fine rotation; 90° steps and flips reshuffle pixels
        // without changing the content set. A manual region pins the angle it was drawn at.
        let m_key = MeterKey::of(settings);
        if self.meter_preview.is_none() || self.meter_key != Some(m_key) {
            self.meter_preview = Some(orient_arc(
                &self.base_preview,
                settings.rotation,
                settings.flip_horizontal,
                m_key.meter_angle,
            ));
            self.meter_key = Some(m_key);
            self.prepared = None;
            self.prepared_key = None;
            self.analysis = None;
            self.stats.orients += 1;
        }
        let o_key = OrientKey::of(settings);
        if self.preview.is_none() || self.orient_key != Some(o_key) {
            // COW where both angles are flat (see `aliases_meter_preview`).
            let preview = if aliases_meter_preview(settings.fine_rotation, m_key.meter_angle) {
                self.meter_preview.clone()
            } else {
                self.stats.orients += 1;
                Some(orient_arc(
                    &self.base_preview,
                    settings.rotation,
                    settings.flip_horizontal,
                    settings.fine_rotation,
                ))
            };
            self.preview = preview;
            self.orient_key = Some(o_key);
            // Sources live in (fine-)oriented space; analysis does not.
            self.proxy_slots.clear();
        }
        let p_key = PreparedKey::of(settings);
        if self.prepared.is_none() || self.prepared_key != Some(p_key) {
            let meter = self
                .meter_preview
                .as_deref()
                .ok_or(SessionError::Invalid("meter image missing"))?;
            self.prepared = Some(kernel::prepare(
                meter,
                settings.crop_rect.as_ref(),
                settings.analysis_rect.as_ref(),
                DEFAULT_ANALYSIS_BUFFER,
            )?);
            self.prepared_key = Some(p_key);
            self.analysis = None;
            self.stats.prepares += 1;
        }
        if self.analysis.is_none() {
            let prepared = self
                .prepared
                .as_ref()
                .ok_or(SessionError::Invalid("prepared stage missing"))?;
            self.analysis = Some(kernel::finalize(prepared));
            self.stats.finalizes += 1;
        }
        match (&self.preview, &self.analysis) {
            (Some(p), Some(a)) => Ok((Arc::clone(p), a.clone())),
            _ => Err(SessionError::Invalid("prepare left the tower incomplete")),
        }
    }

    /// The analysis for the current settings (running the tower as needed).
    pub fn analysis(&mut self) -> Result<ExposureAnalysis> {
        let settings = self.settings.clone();
        Ok(self.prepare(&settings)?.1)
    }

    /// The analysis for explicit settings.
    pub fn analysis_for(&mut self, settings: &ExposureSettings) -> Result<ExposureAnalysis> {
        Ok(self.prepare(settings)?.1)
    }

    /// Resolve `MediumIfFree` against what this body actually gave us.
    fn resolve_tier(&self, requested: RenderTier) -> RenderTier {
        match requested {
            RenderTier::MediumIfFree if self.medium.base.is_none() => RenderTier::Full,
            other => other,
        }
    }

    /// True when the next render must decode or run the heavy prepared stage (the "Analyzing…"
    /// indicator). Fine-rotation-only changes are a cheap re-orient → false; the medium tier
    /// never flashes it; the full tier does while undecoded or orientation-stale.
    pub fn needs_preparation(&self, tier: RenderTier) -> bool {
        let s = &self.settings;
        if self.resolve_tier(tier) == RenderTier::Full
            && (self.full.base.is_none()
                || self.full.oriented.is_none()
                || self.full.orient_key != Some(OrientKey::of(s)))
        {
            return true;
        }
        if self.preview.is_none() || self.prepared.is_none() || self.meter_preview.is_none() {
            return true;
        }
        if self.meter_key != Some(MeterKey::of(s)) {
            return true;
        }
        self.prepared_key != Some(PreparedKey::of(s))
    }

    /// The cached render source for `image` under the current crop state + the pixel window it
    /// covers (`None` = the whole image). Building a cropped source is the only copy.
    fn source_in(
        slots: &mut SourceSlots,
        next_id: &mut u64,
        stats: &mut SessionStats,
        image: &Arc<RgbImage>,
        settings: &ExposureSettings,
        uncropped: bool,
    ) -> (RenderSource, Option<PixelRoi>) {
        let fresh_id = |next_id: &mut u64| {
            let id = *next_id;
            *next_id = next_id.wrapping_add(1);
            id
        };
        let crop = match (uncropped, settings.crop_rect) {
            (false, Some(crop)) => crop,
            _ => {
                if slots.full.is_none() {
                    slots.full = Some(RenderSource {
                        id: fresh_id(next_id),
                        image: Arc::clone(image),
                    });
                    stats.source_builds += 1;
                }
                return match &slots.full {
                    Some(src) => (src.clone(), None),
                    None => (
                        RenderSource {
                            id: fresh_id(next_id),
                            image: Arc::clone(image),
                        },
                        None,
                    ),
                };
            }
        };
        if slots.cropped.is_none() || slots.cropped_rect != Some(crop) {
            slots.cropped = Some(RenderSource {
                id: fresh_id(next_id),
                image: crop_arc(image, &crop),
            });
            slots.cropped_rect = Some(crop);
            stats.source_builds += 1;
        }
        let roi = crop.pixel_roi(image.width, image.height);
        match &slots.cropped {
            Some(src) => (src.clone(), roi),
            None => (
                RenderSource {
                    id: fresh_id(next_id),
                    image: crop_arc(image, &crop),
                },
                roi,
            ),
        }
    }

    /// Source for a display tier with the same orient → crop keying as the proxy path.
    /// `tier.base` must be decoded. Returns (source, roi, oriented image, base dims).
    fn tier_source(
        tier: &mut DisplayTier,
        next_id: &mut u64,
        stats: &mut SessionStats,
        settings: &ExposureSettings,
        uncropped: bool,
    ) -> Result<(RenderSource, Option<PixelRoi>, Arc<RgbImage>, (usize, usize))> {
        let base = tier
            .base
            .as_ref()
            .ok_or(SessionError::Invalid("display tier has no base"))?;
        let base_dims = (base.width, base.height);
        let o_key = OrientKey::of(settings);
        if tier.oriented.is_none() || tier.orient_key != Some(o_key) {
            let oriented = orient_arc(
                base,
                settings.rotation,
                settings.flip_horizontal,
                settings.fine_rotation,
            );
            tier.invalidate_orientation();
            tier.oriented = Some(oriented);
            tier.orient_key = Some(o_key);
            stats.orients += 1;
        }
        let oriented = tier
            .oriented
            .clone()
            .ok_or(SessionError::Invalid("display tier not oriented"))?;
        let (source, roi) = Self::source_in(&mut tier.slots, next_id, stats, &oriented, settings, uncropped);
        Ok((source, roi, oriented, base_dims))
    }

    /// The cached Contrast Mask plane for these settings; `None` when the mask is off (the cache
    /// is freed) or the builder declines. Always built from the PROXY at render geometry, crop
    /// applied here so the plane covers exactly the printed frame.
    fn mask_plane(
        &mut self,
        proxy: &Arc<RgbImage>,
        settings: &ExposureSettings,
        uncropped: bool,
        analysis: &ExposureAnalysis,
    ) -> Option<Arc<MaskPlane>> {
        if settings.contrast_mask == 0.0 {
            self.mask = None;
            return None;
        }
        let key = MaskKey {
            orient: OrientKey::of(settings),
            uncropped,
            spacer: settings.mask_spacer,
            meter: MeterKey::of(settings),
            prep: PreparedKey::of(settings),
        };
        if let Some(entry) = &self.mask
            && entry.key == key
        {
            return Some(Arc::clone(&entry.plane));
        }
        let source: Cow<'_, RgbImage> = match (uncropped, settings.crop_rect) {
            (false, Some(crop)) => proxy.cropped(&crop),
            _ => Cow::Borrowed(proxy),
        };
        match mask::build_plane(&source, &analysis.base_bounds, settings.mask_spacer) {
            Some(plane) => {
                self.stats.mask_builds += 1;
                let plane = Arc::new(plane);
                self.mask = Some(MaskEntry {
                    key,
                    plane: Arc::clone(&plane),
                });
                Some(plane)
            }
            None => {
                self.mask = None;
                None
            }
        }
    }

    // ───────────────────────────── render entry points ─────────────────────────────

    /// Render the current settings.
    pub fn render(&mut self, request: RenderRequest) -> Result<RenderOutput> {
        let settings = self.settings.clone();
        self.render_with(&settings, request)
    }

    /// Render explicit settings against this session's tower (baseline hold, test-strip patches,
    /// zone-placement previews — anything that renders a variant without committing it).
    pub fn render_with(
        &mut self,
        settings: &ExposureSettings,
        request: RenderRequest,
    ) -> Result<RenderOutput> {
        let (preview, analysis) = self.prepare(settings)?;
        let params = derive_render_params(settings, &analysis);
        let resolved = self.resolve_tier(request.tier);
        let (source, roi, oriented_dims, tier_base) = match resolved {
            RenderTier::Full => {
                if self.full.base.is_none() {
                    let decoded = self.source.decode_full()?;
                    self.full.base = Some(Arc::new(decoded));
                    self.stats.full_decodes += 1;
                }
                let (src, roi, oriented, base) = Self::tier_source(
                    &mut self.full,
                    &mut self.next_source_id,
                    &mut self.stats,
                    settings,
                    request.uncropped,
                )?;
                (src, roi, (oriented.width, oriented.height), base)
            }
            RenderTier::MediumIfFree => {
                if !request.retain_full {
                    self.full.clear();
                }
                let (src, roi, oriented, base) = Self::tier_source(
                    &mut self.medium,
                    &mut self.next_source_id,
                    &mut self.stats,
                    settings,
                    request.uncropped,
                )?;
                (src, roi, (oriented.width, oriented.height), base)
            }
            RenderTier::Proxy => {
                if !request.retain_full {
                    self.full.clear();
                }
                let (src, roi) = Self::source_in(
                    &mut self.proxy_slots,
                    &mut self.next_source_id,
                    &mut self.stats,
                    &preview,
                    settings,
                    request.uncropped,
                );
                (src, roi, (preview.width, preview.height), self.proxy_size())
            }
        };
        // Proxy-built whatever the tier: the same plane samples up under the normalized
        // mapping, so a tier swap cannot move the mask.
        let mask_plane = self.mask_plane(&preview, settings, request.uncropped, &analysis);
        let options = RenderOptions {
            srgb: request.srgb,
            compute_histogram: request.compute_histogram,
            mask_plane,
        };
        let renderer = Arc::clone(&self.renderer);
        let (frame, width, height, histogram) = match request.output {
            OutputKind::Rgba8 => {
                let r = renderer.render_display(&source, &params, &options)?;
                (Frame::Rgba8(r.rgba8), r.width, r.height, r.histogram)
            }
            OutputKind::Float => {
                let r = renderer.render(&source, &params, &options)?;
                let (w, h) = (r.image.width, r.image.height);
                (Frame::Float(r.image), w, h, r.histogram)
            }
        };
        self.stats.renders += 1;
        Ok(self.output(settings, request.uncropped, frame, width, height, histogram, roi, oriented_dims, tier_base, &analysis, resolved))
    }

    #[allow(clippy::too_many_arguments)]
    fn output(
        &self,
        settings: &ExposureSettings,
        uncropped: bool,
        frame: Frame,
        width: usize,
        height: usize,
        histogram: Option<Histogram256>,
        roi: Option<PixelRoi>,
        oriented_dims: (usize, usize),
        tier_base: (usize, usize),
        analysis: &ExposureAnalysis,
        tier: RenderTier,
    ) -> RenderOutput {
        let radians = applied_radians(settings.fine_rotation);
        let crop = if uncropped {
            None
        } else {
            settings.crop_rect.as_ref()
        };
        let frame_size = oriented_frame_size(self.proxy_size(), settings.rotation);
        let tier_frame = oriented_frame_size(tier_base, settings.rotation);
        // Pixel counts → f64 (exact).
        let f = |d: (usize, usize)| [d.0 as f64, d.1 as f64];
        RenderOutput {
            frame,
            width,
            height,
            histogram,
            frame_size,
            display_aspect: geometry::display_aspect(f(frame_size), radians, crop),
            content_window: geometry::content_window(f(tier_frame), radians, crop, f(oriented_dims), roi),
            density_range: analysis.base_bounds.luminance_density_range(),
            tier,
        }
    }

    /// A tool-mode render (`AppModel.scheduleRender` with `toolMode != .none`): the whole frame
    /// outside the crop, proxy tier; the crop tool also shows the UNROTATED frame (the Mac
    /// substitutes `fineRotation = 0` only in Crop & Straighten — the analysis tool keeps the
    /// live angle, and `commit_analysis_region` records it). `None` renders normally.
    pub fn render_tool_preview(&mut self, mode: ToolMode, mut request: RenderRequest) -> Result<RenderOutput> {
        let mut settings = self.settings.clone();
        request.uncropped = mode != ToolMode::None;
        if mode != ToolMode::None {
            request.tier = RenderTier::Proxy;
        }
        if mode == ToolMode::Crop {
            settings.fine_rotation = 0.0;
        }
        self.render_with(&settings, request)
    }

    /// The straighten 0°-base precompute (`renderDetached`): the same chain as `render` but
    /// touching NONE of the cache tower — the cached analysis is reused read-only when both its
    /// keys match, else computed afresh and dropped; proxy only, cropped, no tier bookkeeping.
    pub fn render_detached(&mut self, settings: &ExposureSettings, srgb: bool) -> Result<RenderOutput> {
        let m_key = MeterKey::of(settings);
        let p_key = PreparedKey::of(settings);
        let analysis = match &self.analysis {
            Some(a) if self.meter_key == Some(m_key) && self.prepared_key == Some(p_key) => a.clone(),
            _ => {
                let meter = orient_arc(
                    &self.base_preview,
                    settings.rotation,
                    settings.flip_horizontal,
                    m_key.meter_angle,
                );
                kernel::analyze(
                    &meter,
                    settings.crop_rect.as_ref(),
                    settings.analysis_rect.as_ref(),
                    DEFAULT_ANALYSIS_BUFFER,
                )?
            }
        };
        let oriented = orient_arc(
            &self.base_preview,
            settings.rotation,
            settings.flip_horizontal,
            settings.fine_rotation,
        );
        let params = derive_render_params(settings, &analysis);
        let (image, roi) = match settings.crop_rect {
            Some(crop) => (
                crop_arc(&oriented, &crop),
                crop.pixel_roi(oriented.width, oriented.height),
            ),
            None => (Arc::clone(&oriented), None),
        };
        let mask_plane = if settings.contrast_mask != 0.0 {
            mask::build_plane(&image, &analysis.base_bounds, settings.mask_spacer).map(Arc::new)
        } else {
            None
        };
        let source = RenderSource {
            id: self.next_id(),
            image,
        };
        let options = RenderOptions {
            srgb,
            compute_histogram: true,
            mask_plane,
        };
        let r = self.renderer.render_display(&source, &params, &options)?;
        self.stats.renders += 1;
        Ok(self.output(
            settings,
            false,
            Frame::Rgba8(r.rgba8),
            r.width,
            r.height,
            r.histogram,
            roi,
            (oriented.width, oriented.height),
            self.proxy_size(),
            &analysis,
            RenderTier::Proxy,
        ))
    }

    /// Full-resolution export render (`exportRender`): a full decode (the retained HQ tier when
    /// present, else a fresh one that is NOT retained) through the same orientation → fine
    /// rotation → crop chain, with the analysis AND the mask plane from the proxy — the
    /// what-you-see invariant. Returns the ENCODED float image (working space, or sRGB when
    /// `srgb` — the in-kernel export leg). No resize here.
    pub fn export_render(&mut self, srgb: bool) -> Result<RgbImage> {
        let settings = self.settings.clone();
        let (proxy, analysis) = self.prepare(&settings)?;
        let full_base = match &self.full.base {
            Some(b) => Arc::clone(b),
            None => {
                self.stats.full_decodes += 1;
                Arc::new(self.source.decode_full()?)
            }
        };
        let oriented = orient_arc(
            &full_base,
            settings.rotation,
            settings.flip_horizontal,
            settings.fine_rotation,
        );
        drop(full_base);
        let image = match settings.crop_rect {
            Some(crop) => crop_arc(&oriented, &crop),
            None => oriented,
        };
        let params = derive_render_params(&settings, &analysis);
        let mask_plane = self.mask_plane(&proxy, &settings, false, &analysis);
        let source = RenderSource {
            id: self.next_id(),
            image,
        };
        let options = RenderOptions {
            srgb,
            compute_histogram: false,
            mask_plane,
        };
        let r = self.renderer.render(&source, &params, &options)?;
        self.stats.renders += 1;
        Ok(r.image)
    }

    /// `si_session_info` + the Negative-character row, from the current settings' analysis.
    pub fn session_info(&mut self) -> Result<SessionInfo> {
        let settings = self.settings.clone();
        let (_, analysis) = self.prepare(&settings)?;
        let density_range = analysis.base_bounds.luminance_density_range();
        let (width, height) = oriented_frame_size(self.proxy_size(), settings.rotation);
        Ok(SessionInfo {
            width,
            height,
            density_range,
            default_grade_range: curve_logic::default_grade_range(),
            anchor: analysis.anchor,
            textural_range: analysis.textural_range,
            character: NegativeCharacter::of_density_range(density_range),
            cast_confidence: analysis.neutral_confidence,
        })
    }

    /// Normalized-log sample at content-normalized `(u, v)` of the DISPLAYED frame (the
    /// fine-rotated proxy under the committed crop): mean of the `(2·radius+1)²` patch of
    /// `(log10(max(p, 1e-6)) − floor) / (ceil − floor)` per channel against the OFFSET bounds
    /// — the pre-curve value a zone pin freezes (the displayed rgba8 is post-curve and can't
    /// serve). `mask_val` is the plane sample under the pin, 0 when the mask is off.
    pub fn sample_zone_pin(&mut self, u: f64, v: f64, radius: usize) -> Result<ZonePinSample> {
        let settings = self.settings.clone();
        let (image, analysis) = self.prepare(&settings)?;
        let mask_val = self
            .mask_plane(&image, &settings, false, &analysis)
            .map_or(0.0, |p| f64::from(p.sample_normalized(u, v)));
        let bounds = analysis
            .base_bounds
            .applying_offsets(settings.white_point_offset, settings.black_point_offset);
        // Map through the crop: the displayed frame is preview ∩ crop_rect.
        let (mut cu, mut cv) = (u, v);
        if let Some(crop) = settings.crop_rect {
            cu = crop.x + u * crop.width;
            cv = crop.y + v * crop.height;
        }
        let (w, h) = (image.width, image.height);
        if w == 0 || h == 0 {
            return Err(SessionError::Invalid("empty preview"));
        }
        // Truncating casts like Swift's `Int(...)`; NaN → 0; clamped to the frame.
        let cx = ((cu * w as f64) as i64).clamp(0, (w - 1) as i64) as usize;
        let cy = ((cv * h as f64) as i64).clamp(0, (h - 1) as i64) as usize;
        let (x0, x1) = (cx.saturating_sub(radius), (cx + radius).min(w - 1));
        let (y0, y1) = (cy.saturating_sub(radius), (cy + radius).min(h - 1));
        let mut sum = [0f64; 3];
        let mut n = 0f64;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Some(px) = image.pixel(x, y) else {
                    continue;
                };
                for ch in 0..3 {
                    let lg = f64::from(px[ch]).max(1e-6).log10();
                    let denom = bounds.ceils[ch] - bounds.floors[ch];
                    let denom = if denom.abs() < 1e-6 { 1e-6 } else { denom };
                    sum[ch] += (lg - bounds.floors[ch]) / denom;
                }
                n += 1.0;
            }
        }
        let n = n.max(1.0);
        let val = [sum[0] / n, sum[1] / n, sum[2] / n];
        Ok(ZonePinSample {
            val_rgb: val,
            val_luma: K::LUMA_R * val[0] + K::LUMA_G * val[1] + K::LUMA_B * val[2],
            mask_val,
        })
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("path", &self.path)
            .field("proxy", &self.proxy_size())
            .field("medium", &self.medium.base.as_ref().map(|b| (b.width, b.height)))
            .field("full", &self.full.base.as_ref().map(|b| (b.width, b.height)))
            .field("stats", &self.stats)
            .finish()
    }
}

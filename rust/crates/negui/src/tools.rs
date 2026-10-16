//! Canvas tools — the pure half of Crop & Straighten and Crop for Analysis (CLAUDE.md §"Unified
//! Crop & Straighten", session-spec §6 "Tool modes" + "Geometry commits"). SCAFFOLD (wave 4 /
//! U2, wound down before the interactive half landed): the geometry commits here are complete
//! and tested (`tests/tools.rs`); the overlay (`CropBoxOverlay` port: dim surround, thirds,
//! corner handles, clamped move, the in-canvas straighten bar) and the `ToolState` the app
//! carries are the next step — see the plan in [`ToolState`].
//!
//! Coordinates follow `negkit::crop_geometry`: a box lives in rotated space (screen-aligned
//! axes, origin at the frame centre, pixels of the orientation-only frame `frame_size`).

use negkit::NormalizedRect;
use negkit::crop_geometry::{
    RotatedBox, box_from_rect, constrain, inscribed_size, normalized_rect, remap_crop,
};
use negkit::{ExposureSettings, V2};
use negsession::history::labels;

/// Minimum box side in screen points while dragging a corner (CropBoxOverlay.swift).
pub const MIN_BOX_POINTS: f64 = 24.0;
/// Straighten range (degrees) and commit precision.
pub const STRAIGHTEN_RANGE: (f64, f64) = (-45.0, 45.0);
pub const STRAIGHTEN_STEP: f64 = 0.1;

/// What `commit_crop` decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CropCommit {
    /// Nothing changed (label disarmed).
    Unchanged,
    /// `crop_rect` set to this rect ("Crop").
    Crop(NormalizedRect),
    /// The box was (nearly) full-frame: crop cleared ("Crop cleared").
    Cleared,
}

impl CropCommit {
    /// The pending history label for this outcome.
    pub fn label(self) -> Option<&'static str> {
        match self {
            CropCommit::Unchanged => None,
            CropCommit::Crop(_) => Some(labels::CROP),
            CropCommit::Cleared => Some(labels::CROP_CLEARED),
        }
    }
}

/// The box shown when the user has not drawn one yet (`CropBoxOverlay.defaultBox`): the
/// committed crop at the angle it was committed, else the full inscribed rect at `radians`.
pub fn default_box(
    committed: Option<&NormalizedRect>,
    committed_radians: f64,
    frame: V2,
    radians: f64,
) -> RotatedBox {
    match committed {
        Some(rect) => box_from_rect(rect, frame, committed_radians),
        None => {
            let ins = inscribed_size(frame, radians);
            RotatedBox {
                center: [0.0, 0.0],
                half_extents: [ins[0] / 2.0, ins[1] / 2.0],
            }
        }
    }
}

/// The desired box constrained to the live angle — what is drawn and what gets committed.
pub fn effective_box(desired: RotatedBox, frame: V2, radians: f64) -> RotatedBox {
    constrain(desired.center, desired.half_extents, radians, frame, 1.0)
}

/// Whether a constrained box is "no crop" (`DetailView.commitCrop`): centre within 0.5 % of
/// the inscribed width and both extents within 1 % of the inscribed rect.
pub fn is_full_frame(b: &RotatedBox, frame: V2, radians: f64) -> bool {
    let ins = inscribed_size(frame, radians);
    let len = (b.center[0] * b.center[0] + b.center[1] * b.center[1]).sqrt();
    len < 0.005 * ins[0]
        && (b.half_extents[0] * 2.0 - ins[0]).abs() < 0.01 * ins[0]
        && (b.half_extents[1] * 2.0 - ins[1]).abs() < 0.01 * ins[1]
}

/// `DetailView.commitCrop`: constrain the desired box at `radians`, convert to a
/// [`NormalizedRect`] over the inscribed rect, treat a near-full box as "no crop". Writes
/// `settings.crop_rect`; the caller arms [`CropCommit::label`] before the write lands in history.
pub fn commit_crop(
    settings: &mut ExposureSettings,
    desired: RotatedBox,
    frame: V2,
    radians: f64,
) -> CropCommit {
    if !(frame[0] > 0.0 && frame[1] > 0.0) {
        return CropCommit::Unchanged;
    }
    let b = effective_box(desired, frame, radians);
    if is_full_frame(&b, frame, radians) {
        if settings.crop_rect.is_some() {
            settings.crop_rect = None;
            return CropCommit::Cleared;
        }
        return CropCommit::Unchanged;
    }
    let rect = normalized_rect(b.center, b.half_extents, frame, radians);
    if settings.crop_rect == Some(rect) {
        return CropCommit::Unchanged;
    }
    settings.crop_rect = Some(rect);
    CropCommit::Crop(rect)
}

/// `AppModel.commitFineRotation`: set the angle (rounded to [`STRAIGHTEN_STEP`], clamped to
/// [`STRAIGHTEN_RANGE`]) and remap a committed crop so its content does not drift with the
/// inscribed auto-crop. Returns whether anything changed.
pub fn commit_fine_rotation(settings: &mut ExposureSettings, degrees: f64, frame: V2) -> bool {
    let degrees = (degrees / STRAIGHTEN_STEP).round() * STRAIGHTEN_STEP;
    let degrees = degrees.clamp(STRAIGHTEN_RANGE.0, STRAIGHTEN_RANGE.1);
    let mut next = settings.clone();
    next.fine_rotation = degrees;
    if let Some(crop) = settings.crop_rect
        && frame[0] > 0.0
        && frame[1] > 0.0
    {
        next.crop_rect = Some(remap_crop(
            &crop,
            settings.fine_rotation.to_radians(),
            degrees.to_radians(),
            frame,
        ));
    }
    if next == *settings {
        return false;
    }
    *settings = next;
    true
}

/// `AppModel.commitSelection` for the analysis tool — the same rule as
/// `Session::commit_analysis_region` (the UI owns the settings mirror, so it is applied here and
/// reaches the session through the next render's `set_settings`): the rect AND the angle it was
/// drawn at. Rects thinner than 1 % are ignored (`SelectionOverlay`'s commit guard).
pub fn commit_analysis_region(settings: &mut ExposureSettings, rect: NormalizedRect) -> bool {
    if !(rect.width > 0.01 && rect.height > 0.01) {
        return false;
    }
    settings.analysis_rect = Some(rect);
    settings.analysis_rect_fine_rotation = settings.fine_rotation;
    true
}

/// Screen scale (points per frame pixel) that fits the θ-rotated bounding box of `frame` into
/// `area` (the crop tool pins the canvas to this fit; DetailView.swift:244-259).
pub fn crop_fit_scale(frame: V2, radians: f64, area: V2) -> f64 {
    let (s, c) = (radians.sin().abs(), radians.cos().abs());
    let bw = frame[0] * c + frame[1] * s;
    let bh = frame[0] * s + frame[1] * c;
    if !(bw > 0.0 && bh > 0.0) {
        return 1.0;
    }
    (area[0] / bw).min(area[1] / bh).max(1e-6)
}

/// The interactive state the app will carry (NOT yet wired — see the module docs).
///
/// Plan: `crop: Option<CropTool { snapshot: (fine_rotation, crop_rect), angle, desired:
/// Option<RotatedBox>, drag }>` entered by `Command::ToggleCrop` (snapshot for Cancel/Escape,
/// zoom pinned to fit, `HqController` tool flag set, engine renders through
/// `render_tool_preview(ToolMode::Crop)` = uncropped at 0°); the overlay paints the bitmap as a
/// `egui::Mesh` rotated by `angle` about the canvas centre at [`crop_fit_scale`], the box from
/// [`effective_box`] (dim = frame quad minus box, thirds, 4 corner handles resizing about the
/// opposite corner with [`MIN_BOX_POINTS`], interior move bisected against `box_fits`), and an
/// in-canvas bar (Straighten slider ±45°/0.1°, ⨯, Cancel, Apply). Apply = [`commit_crop`] with
/// the drawn box (or [`commit_fine_rotation`] when no box was drawn), then `fine_rotation =
/// angle`, one history entry. `analysis: Option<AnalysisTool { drag: Option<(Pos2, Pos2)> }>`
/// draws the `SelectionOverlay` rect on the plain fit and commits through
/// [`commit_analysis_region`]. The loupe hook: `canvas(.., overlay: Option<&mut dyn
/// CanvasOverlay>)` where the overlay takes over layout, painting and pointer input while a
/// tool is active.
#[derive(Debug, Default)]
pub struct ToolState {
    /// The live straighten angle while the crop tool is up (degrees).
    pub angle: f64,
    /// The user's desired box (`None` = follow the committed crop).
    pub desired: Option<RotatedBox>,
    /// Settings snapshot for Cancel/Escape.
    pub snapshot: Option<(f64, Option<NormalizedRect>)>,
}

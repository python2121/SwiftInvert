//! The HQ-preview decision — `AppModel.HQMode` / `AppModel.renderTier(mode:active:)` in
//! `Sources/SwiftInvert/AppModel.swift` (session-spec §5). Pure functions: the timing (350 ms
//! upward debounce, instant downward) belongs to the front end.
//!
//! Three states: Off (always the 1536 px proxy), Auto (proxy when fitted, the FREE medium tier
//! once magnified to the threshold — never a decode) and On (true full resolution, export-truth,
//! paying the decode). Auto is the launch default; a persisted `.on` would make every launch pay
//! full-res costs. `zoom == 1` is fit-to-window, so a "proxy is out of real pixels" rule would
//! fire at rest on any Retina display — 2× keeps the trigger a property of the gesture.

use crate::session::RenderTier;

/// Canvas magnification at which Auto engages the medium tier (`hqAutoZoomThreshold`).
pub const HQ_AUTO_ZOOM_THRESHOLD: f64 = 2.0;

/// Upward tier swaps are debounced this long; downward swaps are instant (AppModel:833).
pub const HQ_UPWARD_DEBOUNCE_MS: u64 = 350;

/// The badge's three states, in cycling order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HqMode {
    Off,
    #[default]
    Auto,
    On,
}

impl HqMode {
    /// The badge / menu order.
    pub const ALL: [HqMode; 3] = [HqMode::Off, HqMode::Auto, HqMode::On];

    /// Click / ⇧⌘P: off → auto → on → off.
    pub fn next(self) -> HqMode {
        match self {
            HqMode::Off => HqMode::Auto,
            HqMode::Auto => HqMode::On,
            HqMode::On => HqMode::Off,
        }
    }

    /// Whether HQ is ACTIVE right now (`HQMode.resolve`): suppressed with no selection and while
    /// a proxy-only tool (test strip, zone placement) owns the canvas; Off never; On always;
    /// Auto when the crop tool is idle and the zoom has reached `threshold` (inclusive).
    pub fn resolve(
        self,
        zoom: f64,
        threshold: f64,
        has_selection: bool,
        canvas_owned_by_proxy_tool: bool,
        crop_tool_active: bool,
    ) -> bool {
        if !has_selection || canvas_owned_by_proxy_tool {
            return false;
        }
        match self {
            HqMode::Off => false,
            HqMode::On => true,
            HqMode::Auto => !crop_tool_active && zoom >= threshold,
        }
    }

    /// Which source tier a render asks for (`AppModel.renderTier`): inactive → proxy; On →
    /// full; otherwise the free medium tier (Auto never requests the full decode).
    pub fn render_tier(self, active: bool) -> RenderTier {
        if !active {
            RenderTier::Proxy
        } else if self == HqMode::On {
            RenderTier::Full
        } else {
            RenderTier::MediumIfFree
        }
    }
}

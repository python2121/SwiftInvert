//! Normalized rectangles (0…1 of a frame) — port of `NormalizedRect.swift`.
//!
//! Owner: wave 1 / agent A. Struct + serde keys are the shared contract (sidecars carry these as
//! `{"x":…,"y":…,"width":…,"height":…}` — verify against `NormalizedRect.swift` and
//! `SidecarCodecTests`); `pixel_roi` (truncating) and `cropped` are to be ported with
//! `PreprocessRectTests` / `AuditRectAndGeometryTests`.

use serde::{Deserialize, Serialize};

/// A rectangle in normalized frame coordinates, origin top-left, all fields in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    /// The full frame.
    pub const FULL: NormalizedRect = NormalizedRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

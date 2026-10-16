//! Log-exposure bounds analysis — port of `BoundsAnalysis.swift` (NegPy
//! `analyze_log_exposure_bounds`, 127bcd7 same-pixel colour floors).
//!
//! Owner: wave 2 / agent B. `LogNegativeBounds` is the shared contract (also read from the
//! fixtures' `analysis.json`).

use serde::{Deserialize, Serialize};

use crate::V3;

/// Per-channel log-density floors (dense end / print whites) and ceils (thin end), floors < ceils.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogNegativeBounds {
    pub floors: V3,
    pub ceils: V3,
}

impl LogNegativeBounds {
    /// Creative white/black-point trims fold in here at derive time only (floors + wp, ceils + bp).
    pub fn applying_offsets(self, white_point: f64, black_point: f64) -> Self {
        Self {
            floors: [
                self.floors[0] + white_point,
                self.floors[1] + white_point,
                self.floors[2] + white_point,
            ],
            ceils: [
                self.ceils[0] + black_point,
                self.ceils[1] + black_point,
                self.ceils[2] + black_point,
            ],
        }
    }

    /// Rec.709-weighted luminance density range (`luminanceDensityRange`): the grade's
    /// `lumRange` and the Contrast Mask's scale read this from the PRE-offset base bounds.
    pub fn luminance_density_range(&self) -> f64 {
        crate::constants::K::LUMA_R * (self.ceils[0] - self.floors[0]).abs()
            + crate::constants::K::LUMA_G * (self.ceils[1] - self.floors[1]).abs()
            + crate::constants::K::LUMA_B * (self.ceils[2] - self.floors[2]).abs()
    }

    /// Per-channel range `ceil − floor`.
    pub fn ranges(&self) -> V3 {
        [
            self.ceils[0] - self.floors[0],
            self.ceils[1] - self.floors[1],
            self.ceils[2] - self.floors[2],
        ]
    }
}

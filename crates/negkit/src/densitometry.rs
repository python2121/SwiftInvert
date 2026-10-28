//! Darkroom read-outs — port of `Densitometry.swift`: the spot densitometer and the
//! negative-character diagnostic (NegPy `features/exposure/densitometer.py` and `stats.py`'s
//! `_negative_row`). Pure measurement — nothing here feeds the render, so it has no parity
//! surface and no kernel mirror.
//!
//! Densities are relative to this scan's normalization, not absolute.

use crate::V3;
use crate::constants::{K, WorkingOetf};
use crate::curve_logic;

// ───────────────────────────── Spot densitometer ─────────────────────────────

/// Reflection density is meaningless past paper black; NegPy clamps here.
pub const PRINT_DENSITY_MAX: f64 = 4.0;
/// Zone V is 18% reflectance by definition — the ruler's hinge.
pub const ZONE_MID_REFLECTANCE: f64 = 0.18;

/// Display-encoded lightness of Zone V (the OETF runs in `f32`, as the Swift's `WorkingOETF`).
pub fn mid_gray_encoded() -> f64 {
    // Boundary casts: the Swift encodes a Float and widens the result.
    f64::from(WorkingOetf::encode(ZONE_MID_REFLECTANCE as f32))
}

/// Rec.709 luma of a display-encoded triplet. NegPy takes luma on the ENCODED values and
/// decodes afterwards; the order matters (the OETF is non-linear, so decoding per channel
/// first gives a different number).
pub fn luma(rgb: V3) -> f64 {
    K::LUMA_R * rgb[0] + K::LUMA_G * rgb[1] + K::LUMA_B * rgb[2]
}

/// Print zone (0…10) of a display-encoded lightness. Piecewise-linear in ENCODED space with
/// V pinned to 18% gray, because stops-per-zone can't reach the top zones on a print (NegPy
/// `zone_of_encoded`).
pub fn zone_of_encoded(enc: f64) -> f64 {
    let e = enc.clamp(0.0, 1.0);
    let mid = mid_gray_encoded();
    if e <= mid {
        5.0 * e / mid
    } else {
        5.0 + 5.0 * (e - mid) / (1.0 - mid)
    }
}

/// Exact inverse of [`zone_of_encoded`]; kept beside it so the ruler can't fork (NegPy
/// `encoded_of_zone`).
pub fn encoded_of_zone(zone: f64) -> f64 {
    let z = zone.clamp(0.0, 10.0);
    let mid = mid_gray_encoded();
    if z <= 5.0 {
        mid * z / 5.0
    } else {
        mid + (1.0 - mid) * (z - 5.0) / 5.0
    }
}

/// Reflection density of a display-encoded lightness.
pub fn print_density_of_encoded(enc: f64) -> f64 {
    let e = enc.clamp(0.0, 1.0);
    // Boundary casts: the decode is the Float OETF inverse, as the Swift.
    let t = f64::from(WorkingOetf::decode(e as f32)).max(10f64.powf(-PRINT_DENSITY_MAX));
    PRINT_DENSITY_MAX.min(-t.log10())
}

/// One probed pixel, in darkroom units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Display-encoded RGB, 0…1 — what the pixel actually is on screen.
    pub rgb: V3,
    /// Reflection density of the displayed tone.
    pub print_density: f64,
    /// Print zone 0…10 (V = 18% gray).
    pub zone: f64,
}

/// Probe one display-encoded pixel: luma on the ENCODED triplet, then density and zone.
pub fn read(encoded_rgb: V3) -> Reading {
    let lum = luma(encoded_rgb);
    Reading {
        rgb: encoded_rgb,
        print_density: print_density_of_encoded(lum),
        zone: zone_of_encoded(lum),
    }
}

const ROMAN: [&str; 11] = [
    "0", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X",
];
const THIRDS: [&str; 3] = ["", "⅓", "⅔"];

/// Zone in roman numerals with ⅓-stop fractions (4.33 → "IV⅓").
pub fn zone_roman(zone: f64) -> String {
    // Clamped to 0…30 before the cast, so it is exact.
    let t = (zone.clamp(0.0, 10.0) * 3.0).round() as usize;
    let (base, frac) = (t / 3, t % 3);
    if base >= 10 {
        return ROMAN[10].to_string();
    }
    let numeral = ROMAN.get(base).copied().unwrap_or("X");
    let third = THIRDS.get(frac).copied().unwrap_or("");
    format!("{numeral}{third}")
}

// ───────────────────────────── Negative character ─────────────────────────────

/// How the negative's own contrast compares to what the grade expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NegativeCharacter {
    Flat,
    Normal,
    Contrasty,
}

impl NegativeCharacter {
    /// NegPy's wording — the grade hint is the actionable half.
    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "flat (≈N−1)",
            Self::Normal => "normal",
            Self::Contrasty => "contrasty (≈N+1)",
        }
    }

    /// The Swift enum's raw value (`"flat"` / `"normal"` / `"contrasty"`).
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Normal => "normal",
            Self::Contrasty => "contrasty",
        }
    }
}

/// Ratio below which a negative reads as flat (NegPy `stats._DIAG_FLAT`).
pub const CHARACTER_FLAT_RATIO: f64 = 0.80;
/// Ratio above which a negative reads as contrasty (NegPy `stats._DIAG_CONTRASTY`).
pub const CHARACTER_CONTRASTY_RATIO: f64 = 1.25;

/// The negative's measured density range against `default_grade_range`.
///
/// `density_range` is the PRE-offset luminance density range — NegPy's `norm_density_range`,
/// the same value `effective_grade_range` reads (see `derive_render_params`). `None` when
/// there's nothing measured yet.
pub fn character(density_range: f64) -> Option<NegativeCharacter> {
    if !density_range.is_finite() || density_range <= 0.0 {
        return None;
    }
    let ratio = density_range / curve_logic::default_grade_range();
    if ratio < CHARACTER_FLAT_RATIO {
        return Some(NegativeCharacter::Flat);
    }
    if ratio > CHARACTER_CONTRASTY_RATIO {
        return Some(NegativeCharacter::Contrasty);
    }
    Some(NegativeCharacter::Normal)
}

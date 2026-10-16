//! The spot densitometer's state — `DensitometerState` in `Sources/SwiftInvert/Densitometer.swift`
//! (session-spec §9) plus the darkroom read-out math it needs from
//! `Sources/NegativeKit/Densitometry.swift` (NegPy `densitometer.py`, `stats._negative_row`).
//!
//! The probe reads the DISPLAYED rgba8 bitmap (working-space-encoded, so the bytes already ARE
//! the working-space values) rather than a GPU metric: the state adopts the bytes once per
//! render and pointer moves touch only the reading. Mirrors NegPy's quirk of taking luma on the
//! ENCODED triplet before decoding.
//!
//! The read-out functions ([`luma`], [`zone_of_encoded`], [`print_density_of_encoded`],
//! [`NegativeCharacter`]) are a local port of the small `Densitometry` enum because
//! `negkit::densitometry` is a stub on this branch (wave 3 / agent M writes it); swap to
//! re-exports when it lands — same formulas, same constants.

use negkit::constants::{K, WorkingOetf};
use negkit::curve_logic;

/// Print densities are capped here (paper black is ~2.3 D; 4 D = the meter's floor).
pub const PRINT_DENSITY_MAX: f64 = 4.0;
/// Zone V reflectance.
pub const ZONE_MID_REFLECTANCE: f64 = 0.18;

/// The working-space encoding of 18 % gray — zone V on the ruler.
pub fn mid_gray_encoded() -> f64 {
    WorkingOetf::encode_f64(ZONE_MID_REFLECTANCE)
}

/// Rec.709 luma of an encoded triplet (NegPy's quirk: luma BEFORE decoding).
pub fn luma(rgb: [f64; 3]) -> f64 {
    K::LUMA_R * rgb[0] + K::LUMA_G * rgb[1] + K::LUMA_B * rgb[2]
}

/// Zone (0…10) of an encoded value: piecewise linear, zone V at [`mid_gray_encoded`].
pub fn zone_of_encoded(enc: f64) -> f64 {
    let e = enc.clamp(0.0, 1.0);
    let mid = mid_gray_encoded();
    if e <= mid {
        5.0 * e / mid
    } else {
        5.0 + 5.0 * (e - mid) / (1.0 - mid)
    }
}

/// Exact inverse of [`zone_of_encoded`] (the zone-placement ruler).
pub fn encoded_of_zone(zone: f64) -> f64 {
    let z = zone.clamp(0.0, 10.0);
    let mid = mid_gray_encoded();
    if z <= 5.0 {
        mid * z / 5.0
    } else {
        mid + (1.0 - mid) * (z - 5.0) / 5.0
    }
}

/// Print (reflection) density of an encoded value, capped at [`PRINT_DENSITY_MAX`].
pub fn print_density_of_encoded(enc: f64) -> f64 {
    let e = enc.clamp(0.0, 1.0);
    // The Swift decodes through the f32 `WorkingOETF.decode` — same lane here.
    let t = (WorkingOetf::decode(e as f32) as f64).max(10f64.powf(-PRINT_DENSITY_MAX));
    PRINT_DENSITY_MAX.min(-t.log10())
}

/// One probe read-out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The encoded triplet under the pointer (`bytes / 255`).
    pub rgb: [f64; 3],
    pub print_density: f64,
    pub zone: f64,
}

impl Reading {
    /// `Densitometry.read(encodedRGB:)`.
    pub fn from_encoded(rgb: [f64; 3]) -> Self {
        let lum = luma(rgb);
        Self {
            rgb,
            print_density: print_density_of_encoded(lum),
            zone: zone_of_encoded(lum),
        }
    }
}

/// Roman-numeral zone label with ⅓-stop fractions (`Densitometry.zoneRoman`).
pub fn zone_roman(zone: f64) -> String {
    const ROMAN: [&str; 11] = [
        "0", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X",
    ];
    const THIRDS: [&str; 3] = ["", "⅓", "⅔"];
    // Finite after the clamp; `round` then truncate = Swift `Int(x.rounded())`.
    let t = (zone.clamp(0.0, 10.0) * 3.0).round() as usize;
    let (base, frac) = (t / 3, t % 3);
    if base >= 10 {
        return ROMAN[10].to_owned();
    }
    let mut s = ROMAN.get(base).copied().unwrap_or("0").to_owned();
    s.push_str(THIRDS.get(frac).copied().unwrap_or(""));
    s
}

/// The negative-character diagnostic under the Grade slider ("0.90 · normal").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegativeCharacter {
    Flat,
    Normal,
    Contrasty,
}

/// Density-range ÷ default-grade-range below which a negative reads as flat.
pub const CHARACTER_FLAT_RATIO: f64 = 0.80;
/// …and above which it reads as contrasty.
pub const CHARACTER_CONTRASTY_RATIO: f64 = 1.25;

impl NegativeCharacter {
    /// The UI label.
    pub fn label(self) -> &'static str {
        match self {
            NegativeCharacter::Flat => "flat (≈N−1)",
            NegativeCharacter::Normal => "normal",
            NegativeCharacter::Contrasty => "contrasty (≈N+1)",
        }
    }

    /// Classify a measured PRE-offset luminance density range against
    /// `CurveLogic.defaultGradeRange`; `None` for a non-positive / non-finite range.
    pub fn of_density_range(density_range: f64) -> Option<Self> {
        if density_range.is_nan() || density_range <= 0.0 || !density_range.is_finite() {
            return None;
        }
        let ratio = density_range / curve_logic::default_grade_range();
        Some(if ratio < CHARACTER_FLAT_RATIO {
            NegativeCharacter::Flat
        } else if ratio > CHARACTER_CONTRASTY_RATIO {
            NegativeCharacter::Contrasty
        } else {
            NegativeCharacter::Normal
        })
    }
}

/// The adopted display bitmap + the current reading.
#[derive(Clone, Debug, Default)]
pub struct DensitometerState {
    bytes: Option<Vec<u8>>,
    width: usize,
    height: usize,
    reading: Option<Reading>,
}

impl DensitometerState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adopt a render's rgba8 bytes (tight rows, `width * height * 4`); `None` = the image was
    /// switched away. Any stale reading is dropped. A buffer whose length doesn't match the dims
    /// is treated as no bitmap.
    pub fn adopt(&mut self, bitmap: Option<(Vec<u8>, usize, usize)>) {
        self.reading = None;
        match bitmap {
            Some((bytes, w, h))
                if w > 0
                    && h > 0
                    && w.checked_mul(h).and_then(|n| n.checked_mul(4)) == Some(bytes.len()) =>
            {
                self.bytes = Some(bytes);
                self.width = w;
                self.height = h;
            }
            _ => {
                self.bytes = None;
                self.width = 0;
                self.height = 0;
            }
        }
    }

    /// Probe at normalized `(u, v)` of the displayed frame: `x = min(⌊u·w⌋, w−1)`, same for `y`;
    /// out of `[0, 1]` or no bitmap → the reading clears.
    pub fn probe(&mut self, u: f64, v: f64) {
        self.reading = self.read_at(u, v);
    }

    /// The pointer left the canvas.
    pub fn clear(&mut self) {
        self.reading = None;
    }

    /// The current reading, if the last probe landed on the bitmap.
    pub fn reading(&self) -> Option<Reading> {
        self.reading
    }

    /// Whether a bitmap is adopted.
    pub fn has_bitmap(&self) -> bool {
        self.bytes.is_some()
    }

    /// The adopted bitmap (bytes, width, height) — the zone overlay reads the same bytes.
    pub fn bitmap(&self) -> Option<(&[u8], usize, usize)> {
        self.bytes.as_deref().map(|b| (b, self.width, self.height))
    }

    fn read_at(&self, u: f64, v: f64) -> Option<Reading> {
        let bytes = self.bytes.as_deref()?;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        // Truncating casts on values in [0, dim]; the min clamps the u = 1 edge.
        let x = ((u * self.width as f64) as usize).min(self.width.saturating_sub(1));
        let y = ((v * self.height as f64) as usize).min(self.height.saturating_sub(1));
        let i = (y * self.width + x) * 4;
        let px = bytes.get(i..i + 3)?;
        let rgb = [
            f64::from(px[0]) / 255.0,
            f64::from(px[1]) / 255.0,
            f64::from(px[2]) / 255.0,
        ];
        Some(Reading::from_encoded(rgb))
    }
}

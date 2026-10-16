//! The spot densitometer's state — `DensitometerState` in `Sources/SwiftInvert/Densitometer.swift`
//! (session-spec §9) over the darkroom read-out math in [`negkit::densitometry`]
//! (`Densitometry.swift`; NegPy `densitometer.py`, `stats._negative_row`), re-exported here so
//! the front ends reach it through one module.
//!
//! The probe reads the DISPLAYED rgba8 bitmap (working-space-encoded, so the bytes already ARE
//! the working-space values) rather than a GPU metric: the state adopts the bytes once per
//! render and pointer moves touch only the reading. Mirrors NegPy's quirk of taking luma on the
//! ENCODED triplet before decoding. The zone overlay ([`DensitometerState::zone_grid`],
//! `ZoneGrid.compute`) reads the same adopted bytes, so the strip, the probe and the overlay
//! can never disagree.

pub use negkit::densitometry::{
    CHARACTER_CONTRASTY_RATIO, CHARACTER_FLAT_RATIO, NegativeCharacter, PRINT_DENSITY_MAX, Reading,
    ZONE_MID_REFLECTANCE, character, encoded_of_zone, luma, mid_gray_encoded,
    print_density_of_encoded, read, zone_of_encoded, zone_roman,
};
pub use negkit::zone_grid::{self, Grid as ZoneGrid};

/// The zone overlay for a tightly packed rgba8 display bitmap (`ZoneGrid.compute` at the
/// standard 24 cells along the long edge). `None` when the bitmap is degenerate or shorter
/// than its declared geometry.
pub fn zone_grid(rgba8: &[u8], width: usize, height: usize) -> Option<ZoneGrid> {
    let bytes_per_row = width.checked_mul(4)?;
    zone_grid::compute(rgba8, width, height, bytes_per_row)
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

    /// The zone overlay over the adopted bitmap (`None` without one). Recompute per adopt /
    /// overlay toggle, never per pointer move.
    pub fn zone_grid(&self) -> Option<ZoneGrid> {
        let (bytes, w, h) = self.bitmap()?;
        zone_grid(bytes, w, h)
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
        Some(read(rgb))
    }
}

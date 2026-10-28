//! Test support: locate and load the NegPy parity fixtures under `fixtures/`.
//!
//! `.bin` files are float32 little-endian, shape `[h, w, 3]` (or `[h, w]` for planes) given by the
//! sibling `manifest.json`. Shared by every crate's integration tests; not part of the product API.

use std::path::{Path, PathBuf};

/// `<repo>/fixtures`, resolved from this crate's manifest directory.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Absolute path of a fixture file given its manifest-relative path (e.g. `synthetic64/input.bin`).
pub fn path(rel: &str) -> PathBuf {
    root().join(rel)
}

/// Read a float32 LE `.bin` file.
pub fn read_f32(rel: &str) -> Result<Vec<f32>, String> {
    let p = path(rel);
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "{}: {} bytes is not a whole number of f32",
            p.display(),
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Read a `[h, w, 3]` float32 fixture as an [`crate::RgbImage`].
pub fn read_rgb(rel: &str, width: usize, height: usize) -> Result<crate::RgbImage, String> {
    let data = read_f32(rel)?;
    crate::RgbImage::from_vec(width, height, data).map_err(|e| format!("{rel}: {e}"))
}

/// Parse a JSON fixture (manifest, `analysis.json`, `closed_form.json`, `ramp257.json`).
pub fn read_json(rel: &str) -> Result<serde_json::Value, String> {
    let p = path(rel);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// Largest absolute difference and mean absolute difference between two equal-length slices.
pub fn max_mean_abs_diff(a: &[f32], b: &[f32]) -> (f64, f64) {
    let n = a.len().min(b.len()).max(1) as f64;
    let mut max = 0.0f64;
    let mut sum = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        let d = (f64::from(*x) - f64::from(*y)).abs();
        if d > max {
            max = d;
        }
        sum += d;
    }
    (max, sum / n)
}

/// splitmix32-style avalanche — `hash32()` in `scripts/dump_fixtures.py`.
pub fn hash32(i: u32) -> u32 {
    let mut x = i.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x21F0_AAAD);
    x ^= x >> 15;
    x = x.wrapping_mul(0x735A_2D97);
    x ^= x >> 15;
    x
}

/// The `synthetic_grid` fixture's 1600×1066 input, regenerated bit-exactly in `f32` with the
/// Swift test's expression order (`base + n` then `* scale`, clamped to `[1e-4, 1]`). Shared by
/// every test that needs the image (the fixture only stores the prefiltered grid + analysis).
pub fn synthetic_grid_input() -> Result<crate::RgbImage, String> {
    let (w, h) = (1600usize, 1066usize);
    let scales: [f32; 3] = [0.85, 0.45, 0.25];
    let inv_dim = 1.0f32 / (w + h - 2) as f32;
    crate::RgbImage::new_with(w, h, |buf| {
        for y in 0..h {
            for x in 0..w {
                let base: f32 = 0.08 + 0.75 * ((x + y) as f32 * inv_dim);
                let idx = ((y * w + x) as u32).wrapping_mul(3);
                for (c, scale) in scales.iter().enumerate() {
                    let mut n = hash32(idx.wrapping_add(c as u32)) as f32 / 4294967296.0f32;
                    n = (n - 0.5) * 0.06;
                    let v = (base + n) * scale;
                    if let Some(slot) = buf.get_mut((y * w + x) * 3 + c) {
                        *slot = v.clamp(1e-4, 1.0);
                    }
                }
            }
        }
    })
    .map_err(|e| format!("synthetic_grid_input: {e}"))
}

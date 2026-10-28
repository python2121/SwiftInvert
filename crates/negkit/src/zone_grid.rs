//! Zone-system overlay math — port of `ZoneGrid.swift` (NegPy `analysis.zone_grid` /
//! `zone_region_labels`, ae56f8b): the integer Adams zone of each cell of a fixed grid over the
//! display-encoded frame, plus one label anchor per contiguous same-zone region. Pure
//! measurement on the displayed bitmap — the same ruler as the zone strip and the spot
//! densitometer ([`crate::densitometry::zone_of_encoded`] on Rec.709 luma of the ENCODED
//! triplet), so the three read-outs can never disagree. No render involvement, no parity
//! surface.

use crate::densitometry;

/// Cells along the frame's long edge (NegPy ZONE_GRID_CELLS).
pub const CELLS_LONG_EDGE: usize = 24;

/// One numeral anchor per contiguous same-zone region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label {
    pub col: usize,
    pub row: usize,
    /// Integer zone 0…10.
    pub zone: u8,
}

/// The computed overlay: integer zones per cell plus the region labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// Integer zone 0…10 per cell, row-major, `cols × rows` entries.
    pub zones: Vec<u8>,
    /// One numeral anchor per contiguous same-zone region.
    pub labels: Vec<Label>,
}

impl Grid {
    /// The zone of cell `(col, row)`; `None` off the grid.
    pub fn zone(&self, col: usize, row: usize) -> Option<u8> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.zones.get(row * self.cols + col).copied()
    }
}

/// [`compute_with_cells`] at the standard [`CELLS_LONG_EDGE`].
pub fn compute(rgba8: &[u8], width: usize, height: usize, bytes_per_row: usize) -> Option<Grid> {
    compute_with_cells(rgba8, width, height, bytes_per_row, CELLS_LONG_EDGE)
}

/// Grid over an rgba8 display-encoded bitmap (the densitometer's cached bytes). Cells are
/// square-ish whatever the aspect; the area average over a cell is what damps grain — no
/// extra smoothing needed.
///
/// Like NegPy, the frame is stride-subsampled first (a full-res HQ buffer is millions of
/// pixels; ~4 samples per cell edge is plenty), then box-averaged per cell. NegPy uses
/// INTER_AREA for the second step; integer-boundary box means differ by at most the
/// fractional edge samples — well under the zone rounding step, and this is a read-out, not a
/// parity surface.
///
/// `None` when the frame is smaller than 2×2, `cells` is 0, or the buffer is shorter than its
/// declared geometry (`height × bytes_per_row`, with `bytes_per_row ≥ width × 4`).
pub fn compute_with_cells(
    rgba8: &[u8],
    width: usize,
    height: usize,
    bytes_per_row: usize,
    cells: usize,
) -> Option<Grid> {
    // Checked geometry: these come from a caller's bitmap description (hostile by rule).
    let row_bytes = width.checked_mul(4)?;
    if width < 2 || height < 2 || cells == 0 || bytes_per_row < row_bytes {
        return None;
    }
    // Buffer-geometry check up front: the hot loop then reads through `get` anyway.
    let needed = (height - 1)
        .checked_mul(bytes_per_row)?
        .checked_add(row_bytes)?;
    if rgba8.len() < needed {
        return None;
    }

    // Stride subsample (NegPy: step = max(1, min(h, w) // (4 * cells))).
    let step = (width.min(height) / cells.checked_mul(4)?).max(1);
    let sw = width.div_ceil(step);
    let sh = height.div_ceil(step);

    // Grid dims from the sampled aspect: long edge = `cells`, short edge proportional
    // (NegPy: scale = cells / max(dims), rounded).
    let scale = cells as f64 / sw.max(sh) as f64;
    // Non-negative and bounded by `cells`, so the casts are exact.
    let cols = ((sw as f64 * scale).round() as usize).max(1);
    let rows = ((sh as f64 * scale).round() as usize).max(1);

    // Per-cell box average of the ENCODED rgb, then luma → zone → rint.
    let mut zones = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        let y0 = r * sh / rows;
        let y1 = ((r + 1) * sh / rows).max(y0 + 1);
        for c in 0..cols {
            let x0 = c * sw / cols;
            let x1 = ((c + 1) * sw / cols).max(x0 + 1);
            let mut sum = [0.0f64; 3];
            for sy in y0..y1 {
                let row_base = (sy * step).min(height - 1) * bytes_per_row;
                for sx in x0..x1 {
                    let i = row_base + (sx * step).min(width - 1) * 4;
                    if let Some(px) = rgba8.get(i..i + 3) {
                        sum[0] += f64::from(px[0]);
                        sum[1] += f64::from(px[1]);
                        sum[2] += f64::from(px[2]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64 * 255.0;
            let zone = densitometry::zone_of_encoded(densitometry::luma([
                sum[0] / n,
                sum[1] / n,
                sum[2] / n,
            ]));
            // Clamped to 0…10 before the cast, so it is exact.
            zones.push(zone.round().clamp(0.0, 10.0) as u8);
        }
    }
    let labels = region_labels(&zones, cols, rows);
    Some(Grid {
        cols,
        rows,
        zones,
        labels,
    })
}

/// One label anchor per 4-connected same-zone region. The anchor is the region cell nearest
/// its centroid, so a concave region's numeral can't land outside it (its centroid can) —
/// NegPy `zone_region_labels`.
pub fn region_labels(zones: &[u8], cols: usize, rows: usize) -> Vec<Label> {
    if cols == 0 || rows == 0 || zones.len() != cols * rows {
        return Vec::new();
    }
    let mut seen = vec![false; zones.len()];
    let mut labels = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut region: Vec<usize> = Vec::new();
    for start in 0..zones.len() {
        if seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some(&zone) = zones.get(start) else {
            continue;
        };
        region.clear();
        stack.clear();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(i) = stack.pop() {
            region.push(i);
            let (c, r) = (i % cols, i / cols);
            let mut visit = |j: usize| {
                if zones.get(j) == Some(&zone)
                    && let Some(s) = seen.get_mut(j)
                    && !*s
                {
                    *s = true;
                    stack.push(j);
                }
            };
            if c > 0 {
                visit(i - 1);
            }
            if c + 1 < cols {
                visit(i + 1);
            }
            if r > 0 {
                visit(i - cols);
            }
            if r + 1 < rows {
                visit(i + cols);
            }
        }
        let n = region.len() as f64;
        let (mut cx, mut cy) = (0.0f64, 0.0f64);
        for &i in &region {
            cx += (i % cols) as f64;
            cy += (i / cols) as f64;
        }
        cx /= n;
        cy /= n;
        let mut best = start;
        let mut best_d = f64::INFINITY;
        for &i in &region {
            let dx = (i % cols) as f64 - cx;
            let dy = (i / cols) as f64 - cy;
            let d = dx * dx + dy * dy;
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        labels.push(Label {
            col: best % cols,
            row: best / cols,
            zone,
        });
    }
    labels
}

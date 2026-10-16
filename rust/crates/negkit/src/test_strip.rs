//! Test-strip math — port of `TestStrip.swift` (NegPy `analysis.strip_*`, e4bc450/0.45; ladder
//! rotation a2455ab): the frame printed as a 5×5 grid of real renders — Brightness across,
//! Grade down — like a split-filter darkroom test strip. Pure grid math + rgba8 mosaic
//! assembly; the renders themselves come from the session.
//!
//! Ladders are ABSOLUTE and centred on the defaults (NegPy's revised spec): a strip printed
//! off one frame is comparable to the next, and the current settings are one of the patches.
//! Deliberate presentation divergence from upstream: columns are ordered so the image
//! BRIGHTENS left → right (density descending), matching this app's "right = brighter" slider
//! convention everywhere; upstream draws density ascending.

/// Print density per column, left → right (brightness 0.4 … 1.6).
pub const DENSITIES_BY_COLUMN: [f64; 5] = [1.6, 1.3, 1.0, 0.7, 0.4];
/// ISO R grade per row, top → bottom (hard → soft, NegPy order).
pub const GRADES_BY_ROW: [f64; 5] = [75.0, 95.0, 115.0, 135.0, 155.0];

/// Columns of the strip.
pub const COLS: usize = DENSITIES_BY_COLUMN.len();
/// Rows of the strip (== [`COLS`]; the ladder rotation relies on a square grid).
pub const ROWS: usize = GRADES_BY_ROW.len();

/// One patch: its DISPLAY position and the ladder values it renders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    pub density: f64,
    pub grade: f64,
}

/// A grid position `(row, col)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RowCol {
    pub row: usize,
    pub col: usize,
}

/// Pixel rect of one patch, `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PatchRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

/// Every patch, row-major (the render order), at orientation 0.
pub fn cells() -> Vec<Cell> {
    cells_oriented(0)
}

/// The ladder turned by `orientation` × 90° clockwise (NegPy a2455ab): the 5×5 grid stays put
/// over the frame; what rotates is which axis carries density vs grade and their directions,
/// so the dense/hard end lands on a different part of the frame. `Cell { row, col }` is the
/// DISPLAY position; its density/grade come from the rotated logical grid. Row-major.
pub fn cells_oriented(orientation: i32) -> Vec<Cell> {
    (0..ROWS)
        .flat_map(|r| (0..COLS).filter_map(move |c| cell(r, c, orientation)))
        .collect()
}

/// One display cell's ladder values under `orientation`; `None` off the grid.
pub fn cell(row: usize, col: usize, orientation: i32) -> Option<Cell> {
    if row >= ROWS || col >= COLS {
        return None;
    }
    let n = COLS; // == ROWS; the rotation needs the grid square
    let o = orientation.rem_euclid(4);
    let (grade_idx, density_idx) = match o {
        1 => (n - 1 - col, row),
        2 => (n - 1 - row, n - 1 - col),
        3 => (col, n - 1 - row),
        _ => (row, col),
    };
    Some(Cell {
        row,
        col,
        density: *DENSITIES_BY_COLUMN.get(density_idx)?,
        grade: *GRADES_BY_ROW.get(grade_idx)?,
    })
}

/// Integer split of `extent` into `divisions` (NegPy `_strip_bounds`): exact tiling, no gaps
/// or overlaps whatever the rounding.
fn bounds(extent: usize, divisions: usize, index: usize) -> (usize, usize) {
    (extent * index / divisions, extent * (index + 1) / divisions)
}

/// Pixel rect of one patch inside a `width × height` frame (exclusive upper bounds). Rows and
/// columns past the grid clamp to the last one.
pub fn patch_rect(width: usize, height: usize, row: usize, col: usize) -> PatchRect {
    let (x0, x1) = bounds(width, COLS, col.min(COLS - 1));
    let (y0, y1) = bounds(height, ROWS, row.min(ROWS - 1));
    PatchRect { x0, y0, x1, y1 }
}

/// Content-normalized position (0…1) → `(row, col)`; edges clamp.
pub fn cell_at(nx: f64, ny: f64) -> RowCol {
    // Swift `Int(Double)` truncates toward zero; the clamp then bounds it (NaN → 0).
    let row = ((ny * ROWS as f64) as i64).clamp(0, ROWS as i64 - 1) as usize;
    let col = ((nx * COLS as f64) as i64).clamp(0, COLS as i64 - 1) as usize;
    RowCol { row, col }
}

/// Index of the ladder rung nearest `value` (first wins on ties, as Swift's `min(by:)`).
fn nearest_index(ladder: &[f64], value: f64) -> usize {
    let mut best = 0usize;
    let mut best_d = f64::INFINITY;
    for (i, v) in ladder.iter().enumerate() {
        let d = (v - value).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// The rung nearest the given settings (the current-settings accent).
pub fn nearest_cell(density: f64, grade: f64) -> RowCol {
    RowCol {
        row: nearest_index(&GRADES_BY_ROW, grade),
        col: nearest_index(&DENSITIES_BY_COLUMN, density),
    }
}

/// Copy one tile's own patch region into the mosaic (rgba8, shared `bytes_per_row`). Called
/// per render so only mosaic + one tile are ever held; each tile contributes only the frame
/// area its patch occupies, so the mosaic reads as ONE print (NegPy `strip_mosaic`). A tile
/// or mosaic too short for the patch, or mismatched lengths, leaves the mosaic untouched.
pub fn copy_patch(
    tile: &[u8],
    mosaic: &mut [u8],
    width: usize,
    height: usize,
    bytes_per_row: usize,
    row: usize,
    col: usize,
) {
    let r = patch_rect(width, height, row, col);
    if r.x1 <= r.x0 || r.y1 <= r.y0 || mosaic.len() != tile.len() {
        return;
    }
    if tile.len() < (r.y1 - 1) * bytes_per_row + r.x1 * 4 {
        return;
    }
    let row_bytes = (r.x1 - r.x0) * 4;
    for y in r.y0..r.y1 {
        let offset = y * bytes_per_row + r.x0 * 4;
        if let (Some(src), Some(dst)) = (
            tile.get(offset..offset + row_bytes),
            mosaic.get_mut(offset..offset + row_bytes),
        ) {
            dst.copy_from_slice(src);
        }
    }
}

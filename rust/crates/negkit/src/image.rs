//! Interleaved-RGB float image — port of `PlanarImage.swift` (`RGBImage`).
//!
//! Owner: wave 1 / agent A. The struct and constructors below are the shared contract (other
//! crates and agents compile against them); the geometry ops are to be ported from the Swift file
//! with its tests (`OrientationTests`, `ImagePipelineSeamTests`, `DisplayAspectTests`' image parts).
//!
//! Allocation note (the "big-buffer discipline"): `vec![0.0f32; n]` is fine in Rust — it goes
//! through `alloc_zeroed`, so fresh pages are never touched until written. What costs is a
//! non-zero fill or a `resize` after the fact. Prefer building with iterators / `extend` or writing
//! every lane of a zeroed buffer exactly once.

use crate::NegError;

/// Interleaved RGB `f32`, row-major, nominally scene-linear in `[0, 1]`.
///
/// Layout is `data[(y * width + x) * 3 + channel]` — the same bytes the GPU storage buffers hold,
/// so uploads and readbacks are memcpys.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl RgbImage {
    /// Wrap an interleaved buffer; the length must be `width * height * 3`.
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Result<Self, NegError> {
        let want = width.checked_mul(height).and_then(|n| n.checked_mul(3));
        match want {
            Some(n) if n == data.len() && n > 0 => Ok(Self {
                width,
                height,
                data,
            }),
            Some(_) | None if width == 0 || height == 0 => Err(NegError::Empty),
            _ => Err(NegError::Shape {
                width,
                height,
                len: data.len(),
            }),
        }
    }

    /// A zero-filled image (`alloc_zeroed`: pages are not touched until written).
    pub fn zeros(width: usize, height: usize) -> Result<Self, NegError> {
        let n = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(3))
            .ok_or(NegError::Invalid("image too large"))?;
        if n == 0 {
            return Err(NegError::Empty);
        }
        Ok(Self {
            width,
            height,
            data: vec![0.0; n],
        })
    }

    /// Build by filling a zeroed buffer in place (every lane written exactly once by `fill`).
    pub fn new_with(
        width: usize,
        height: usize,
        fill: impl FnOnce(&mut [f32]),
    ) -> Result<Self, NegError> {
        let mut img = Self::zeros(width, height)?;
        fill(&mut img.data);
        Ok(img)
    }

    #[inline]
    pub fn pixel_count(&self) -> usize {
        self.width * self.height
    }

    /// The RGB triple at `(x, y)`; `None` outside the image (never panics on hostile coordinates).
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y * self.width + x) * 3;
        self.data.get(i..i + 3).map(|p| [p[0], p[1], p[2]])
    }

    /// One image row as an interleaved slice.
    #[inline]
    pub fn row(&self, y: usize) -> Option<&[f32]> {
        let w3 = self.width * 3;
        self.data.get(y * w3..(y + 1) * w3)
    }

    /// Long edge in pixels.
    #[inline]
    pub fn long_edge(&self) -> usize {
        self.width.max(self.height)
    }
}

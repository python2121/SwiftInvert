#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negkit` — the film-negative conversion kernel as pure math.
//!
//! A port of SwiftInvert's `NegativeKit` (itself a ~10 % rewrite of the Python app NegPy, which
//! remains the numerical reference; the fixtures under `Tests/Fixtures` were dumped from NegPy's
//! engine and are the oracle for every stage boundary). Nothing here does I/O, touches a GPU or a
//! UI: Foundation-free, Accelerate-free, `unsafe`-free, builds for wasm32 with `--no-default-features`.
//!
//! Pipeline (see `rust/PLAN.md` §4 and the repository `CLAUDE.md`):
//! decode → orient → **analyze** (CPU: [`prefilter`] → [`bounds`] → [`meters`] → [`kernel`]) →
//! **derive** ([`mod@derive`], µs) → render ([`mod@reference`] on the CPU; `neggpu` on the GPU) → encode.
//!
//! Precision: analysis and derivation run in `f64` (Swift `Double`); pixels are `f32`.
//! Every order statistic funnels through [`stats`] (radix sort), never `.sort()` on pixel data.

pub mod bounds;
pub mod constants;
pub mod contrast_mask;
pub mod crop_geometry;
pub mod curve_logic;
pub mod densitometry;
pub mod derive;
#[doc(hidden)]
pub mod fixtures;
pub mod image;
pub mod kernel;
pub mod lab;
pub mod meters;
pub mod prefilter;
pub mod radix_sort;
pub mod rect;
pub mod reference;
pub mod settings;
pub mod stats;
pub mod test_strip;
pub mod zone_grid;
pub mod zone_placement;

pub use bounds::LogNegativeBounds;
pub use derive::RenderParams;
pub use image::RgbImage;
pub use kernel::{ExposureAnalysis, Prepared};
pub use rect::NormalizedRect;
pub use settings::ExposureSettings;

/// Errors from kernel entry points. Pixel math itself is total; errors come from shapes
/// (zero-sized images, rects that collapse, buffers whose length doesn't match their dims).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NegError {
    #[error("image dimensions {width}×{height} don't match {len} samples")]
    Shape {
        width: usize,
        height: usize,
        len: usize,
    },
    #[error("empty image")]
    Empty,
    #[error("degenerate region: {0}")]
    Region(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
}

/// A `Vec3`-shaped triple for RGB / CMY quantities in `f64` (Swift `SIMD3<Double>`).
pub type V3 = [f64; 3];
/// Four lanes (Swift `SIMD4<Double>`): the colour-mixer bands R/Y/G/B.
pub type V4 = [f64; 4];
/// A 2-D point (Swift `SIMD2<Double>`): levels anchors `(input, output)`.
pub type V2 = [f64; 2];

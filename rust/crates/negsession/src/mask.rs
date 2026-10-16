//! The Contrast Mask plane as the session caches it and the renderers sample it — a thin
//! façade over [`negkit::contrast_mask`] (`ContrastMask.swift`, NegPy 515c1f5).
//!
//! The plane is per-image DATA, not a render parameter: a blurred zero-mean normalized-luma
//! field built from the PRINTED proxy frame (crop applied), σ = `maskSpacer` % × the analysis
//! grid's short side. Every tier and the export sample the same plane through the half-pixel
//! bilinear mapping in [`MaskPlane::sample_at`] — that is what makes a tier swap unable to
//! move the mask. The builder, the sampler and the `val_scale` all live in negkit; this module
//! only names them for the session and adds the memory accounting the session's
//! `bytes_resident` needs.
//!
//! GPU identity: a renderer keys its uploaded copy on the `Arc<MaskPlane>` pointer and HOLDS
//! that `Arc` for as long as the upload is cached, so the address cannot be reused by a newer
//! plane while the cache entry lives (see `renderer::GpuRenderer`).

pub use negkit::contrast_mask::{
    GAMMA_LIMIT, Plane as MaskPlane, SPACER_DEFAULT, SPACER_MAX, SPACER_MIN, build_plane, val_scale,
};

/// Resident bytes of a plane (the session's memory accounting).
pub fn plane_bytes(plane: &MaskPlane) -> usize {
    plane.data.len() * std::mem::size_of::<f32>()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn plane_sampling_is_negkits() {
        // The re-exported plane carries the kernels' half-pixel bilinear sampler.
        let plane = MaskPlane::new(2, 2, vec![0.0, 1.0, 0.0, 1.0]).unwrap();
        assert_eq!(plane.sample_at(0, 0, 2, 2), 0.0);
        assert_eq!(plane.sample_at(1, 0, 2, 2), 1.0);
        assert!((plane.sample_at(1, 0, 4, 2) - 0.25).abs() < 1e-6);
        assert!((plane.sample_normalized(0.5, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(plane_bytes(&plane), 16);
        assert!(MaskPlane::new(2, 2, vec![0.0; 3]).is_err());
    }
}

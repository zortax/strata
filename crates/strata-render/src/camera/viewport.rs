//! Viewport: physical pixel size + scale factor, logical-size helper.

use glam::{DVec2, UVec2};

/// Render-target size in **physical** pixels plus the device scale factor.
/// All camera math runs in logical pixels (`physical / scale_factor`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    size_px: UVec2,
    scale_factor: f32,
}

impl Viewport {
    /// Clamps the size to at least 1×1 and the scale factor to a sane
    /// positive value so projection math never divides by zero.
    pub fn new(size_px: UVec2, scale_factor: f32) -> Self {
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        Self {
            size_px: size_px.max(UVec2::ONE),
            scale_factor,
        }
    }

    /// Physical pixels.
    pub fn size_px(&self) -> UVec2 {
        self.size_px
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Logical pixels (`physical / scale_factor`).
    pub fn logical_size(&self) -> DVec2 {
        DVec2::new(self.size_px.x as f64, self.size_px.y as f64) / self.scale_factor as f64
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new(UVec2::ONE, 1.0)
    }
}

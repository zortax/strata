//! Terrain tint style: how the grayscale hillshade is mapped into the theme
//! palette (`mix(shadow_tint, light_tint, value) * opacity` in
//! `raster_tile.wgsl`).

use bytemuck::{Pod, Zeroable};

/// Tint colors and opacity for the hillshade layer. Colors are **linear**
/// RGBA (the render target is sRGB; fragments output linear), alpha 1 — the
/// effective alpha comes from `opacity` (and the per-tile fade), which also
/// premultiplies the color in the shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainStyle {
    /// Color of fully shadowed terrain (`value == 0`).
    pub shadow_tint: [f32; 4],
    /// Color of fully lit terrain (`value == 1`).
    pub light_tint: [f32; 4],
    /// Layer opacity in `0..=1`; ~0.5 so the basemap reads through.
    pub opacity: f32,
}

impl Default for TerrainStyle {
    /// Tuned to the high-contrast dark palette: shadows pull toward deep
    /// brown-black, lights toward a warm grey.
    fn default() -> Self {
        Self {
            shadow_tint: tint_from_srgb8(0x1a, 0x12, 0x0c),
            light_tint: tint_from_srgb8(0x8c, 0x84, 0x78),
            opacity: 0.5,
        }
    }
}

/// An sRGB-encoded 8-bit color as a linear RGBA tint (alpha 1).
pub fn tint_from_srgb8(r: u8, g: u8, b: u8) -> [f32; 4] {
    fn linear(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    [linear(r), linear(g), linear(b), 1.0]
}

/// CPU mirror of the `TerrainStyle` uniform in `raster_tile.wgsl`
/// (vec4 + vec4 + f32, padded to 48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct TerrainStyleUniform {
    pub shadow_tint: [f32; 4],
    pub light_tint: [f32; 4],
    pub opacity: f32,
    pub pad: [f32; 3],
}

impl From<TerrainStyle> for TerrainStyleUniform {
    fn from(style: TerrainStyle) -> Self {
        Self {
            shadow_tint: style.shadow_tint,
            light_tint: style.light_tint,
            opacity: style.opacity.clamp(0.0, 1.0),
            pad: [0.0; 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_style_is_sane() {
        let style = TerrainStyle::default();
        assert!((0.0..=1.0).contains(&style.opacity));
        for channel in style.shadow_tint.iter().chain(style.light_tint.iter()) {
            assert!((0.0..=1.0).contains(channel), "channel {channel} of range");
        }
        assert_eq!(style.shadow_tint[3], 1.0);
        assert_eq!(style.light_tint[3], 1.0);
        // Shadows must actually be darker than lights, channel-wise.
        for (s, l) in style.shadow_tint[..3].iter().zip(&style.light_tint[..3]) {
            assert!(s < l, "shadow {s} not darker than light {l}");
        }
    }

    #[test]
    fn tint_from_srgb8_endpoints() {
        assert_eq!(tint_from_srgb8(0, 0, 0), [0.0, 0.0, 0.0, 1.0]);
        let white = tint_from_srgb8(255, 255, 255);
        for c in &white[..3] {
            assert!((c - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn uniform_is_48_bytes_and_clamps_opacity() {
        assert_eq!(std::mem::size_of::<TerrainStyleUniform>(), 48);
        let uniform = TerrainStyleUniform::from(TerrainStyle {
            opacity: 7.0,
            ..TerrainStyle::default()
        });
        assert_eq!(uniform.opacity, 1.0);
        let uniform = TerrainStyleUniform::from(TerrainStyle {
            opacity: -1.0,
            ..TerrainStyle::default()
        });
        assert_eq!(uniform.opacity, 0.0);
    }
}

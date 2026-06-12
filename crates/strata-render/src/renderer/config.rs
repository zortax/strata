//! Renderer configuration.

use crate::map_theme::MapTheme;
use crate::tiles::TileSource;

use std::sync::Arc;

/// Configuration for [`crate::renderer::MapRenderer::new`].
#[derive(Clone)]
pub struct RendererConfig {
    /// Color format of the offscreen target the app's surface samples.
    pub format: wgpu::TextureFormat,
    /// Vector basemap tiles (MVT). `None` renders no basemap.
    pub basemap_source: Option<Arc<dyn TileSource>>,
    /// Terrain hillshade tiles (PNG). `None` renders no terrain.
    pub terrain_source: Option<Arc<dyn TileSource>>,
    /// Deepest basemap source level when the source does not report one
    /// itself ([`TileSource::max_zoom`]); deeper views overzoom.
    pub max_basemap_zoom: u8,
    /// Deepest terrain source level; deeper views overzoom.
    pub max_terrain_zoom: u8,
    /// Basemap zoom-selection bias: source tiles are picked at
    /// `floor(camera_zoom + bias)`. `0.0` switches levels exactly at integer
    /// camera zooms; negative values bring detail in later (calmer map),
    /// positive earlier (sharper but busier). Adjustable at runtime via
    /// [`crate::renderer::MapRenderer::set_basemap_detail_bias`].
    pub basemap_detail_bias: f64,
    /// Initial map color theme (default: [`MapTheme::oldworld`]). All map
    /// colors — basemap palette, airspace/symbol/weather styles, terrain
    /// tint and the renderer clear color — derive from it; switch at
    /// runtime via [`crate::renderer::MapRenderer::set_map_theme`].
    pub theme: MapTheme,
    /// Worker threads for decode/tessellation. `None` = derived from
    /// available parallelism.
    pub worker_threads: Option<usize>,
    /// LRU byte budget (vertex + index bytes) of the persistent airspace
    /// feature-mesh cache. A single country's airspaces stay fully resident
    /// well below the default ~128 MiB; multi-country datasets evict by
    /// recency instead of growing unbounded.
    pub airspace_mesh_cache_bytes: usize,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            basemap_source: None,
            terrain_source: None,
            // Ingest defaults: basemap extracted to z13, terrain tiles z5–11.
            max_basemap_zoom: 13,
            max_terrain_zoom: 11,
            basemap_detail_bias: crate::tiles::DEFAULT_BASEMAP_DETAIL_BIAS,
            theme: MapTheme::oldworld(),
            worker_threads: None,
            airspace_mesh_cache_bytes: crate::layers::DEFAULT_AIRSPACE_MESH_CACHE_BYTES,
        }
    }
}

/// A premultiplied `[f32; 4]` theme color (display-space, see
/// [`crate::map_theme::srgb8`]) as a `wgpu::Color` clear value.
pub fn clear_color_from_palette(color: [f32; 4]) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(color[0]),
        g: f64::from(color[1]),
        b: f64::from(color[2]),
        a: f64::from(color[3]),
    }
}

/// An sRGB-encoded 8-bit color as a linear `wgpu::Color` clear value
/// (alpha 1, effectively premultiplied).
pub fn clear_color_from_srgb8(r: u8, g: u8, b: u8) -> wgpu::Color {
    fn linear(c: u8) -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    wgpu::Color {
        r: linear(r),
        g: linear(g),
        b: linear(b),
        a: 1.0,
    }
}

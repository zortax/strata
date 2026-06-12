//! XYZ tile math (slippy-map scheme over normalized Web-Mercator world
//! space) and the [`TileSource`] injection point for tile bytes.

use crate::camera::Camera;

use glam::DVec2;

/// Terrain zoom-selection bias: tiles are chosen at
/// `floor(zoom + TILE_PICK_BIAS)`. Hillshade is low-frequency, so picking
/// the next level slightly early reads as sharper, not busier.
pub const TILE_PICK_BIAS: f64 = 0.3;

/// Default basemap detail bias (see [`display_level`]'s `bias`). Negative:
/// the next tile level — and with it roads/landuse detail — arrives roughly
/// half a zoom level *after* the integer boundary instead of 0.3 before it,
/// keeping the map calm while zooming in. Runtime-adjustable via
/// `MapRenderer::set_basemap_detail_bias`.
pub const DEFAULT_BASEMAP_DETAIL_BIAS: f64 = -0.5;

/// Hard ceiling on tile tree depth (x/y fit u32 up to z 31; 30 is plenty).
pub const MAX_TILE_ZOOM: u8 = 30;

/// An XYZ tile address. `y` grows southward (slippy-map convention), which
/// matches world-space `y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// `None` if `x`/`y` are out of range for `z` or `z > MAX_TILE_ZOOM`.
    pub fn new(z: u8, x: u32, y: u32) -> Option<Self> {
        if z > MAX_TILE_ZOOM {
            return None;
        }
        let n = Self::tiles_across(z);
        (x < n && y < n).then_some(Self { z, x, y })
    }

    /// Number of tiles along one axis at level `z` (`2^z`).
    pub fn tiles_across(z: u8) -> u32 {
        1u32 << z.min(MAX_TILE_ZOOM)
    }

    /// World-space bounds `(min, max)` of this tile in `[0, 1]^2`.
    pub fn world_bounds(&self) -> (DVec2, DVec2) {
        let n = Self::tiles_across(self.z) as f64;
        let min = DVec2::new(self.x as f64 / n, self.y as f64 / n);
        let max = DVec2::new((self.x + 1) as f64 / n, (self.y + 1) as f64 / n);
        (min, max)
    }

    /// World-space side length (`1 / 2^z`).
    pub fn world_size(&self) -> f64 {
        1.0 / Self::tiles_across(self.z) as f64
    }

    /// The containing tile one level up; `None` at the root.
    pub fn parent(&self) -> Option<TileId> {
        (self.z > 0).then(|| TileId {
            z: self.z - 1,
            x: self.x / 2,
            y: self.y / 2,
        })
    }

    /// The containing tile at level `z` (`z <= self.z`); `None` otherwise.
    pub fn ancestor(&self, z: u8) -> Option<TileId> {
        if z > self.z {
            return None;
        }
        let shift = self.z - z;
        Some(TileId {
            z,
            x: self.x >> shift,
            y: self.y >> shift,
        })
    }

    /// The four child tiles; `None` at [`MAX_TILE_ZOOM`].
    pub fn children(&self) -> Option<[TileId; 4]> {
        if self.z >= MAX_TILE_ZOOM {
            return None;
        }
        let (z, x, y) = (self.z + 1, self.x * 2, self.y * 2);
        Some([
            TileId { z, x, y },
            TileId { z, x: x + 1, y },
            TileId { z, x, y: y + 1 },
            TileId {
                z,
                x: x + 1,
                y: y + 1,
            },
        ])
    }

    /// True if `self` contains `other` in the tile tree (or equals it).
    pub fn is_ancestor_of(&self, other: TileId) -> bool {
        other.ancestor(self.z) == Some(*self)
    }

    /// The tile at level `z` containing a world-space point (clamped to the
    /// world square).
    pub fn containing(z: u8, world: DVec2) -> TileId {
        let z = z.min(MAX_TILE_ZOOM);
        let n = Self::tiles_across(z);
        let clamp_axis = |v: f64| ((v * n as f64) as i64).clamp(0, (n - 1) as i64) as u32;
        TileId {
            z,
            x: clamp_axis(world.x),
            y: clamp_axis(world.y),
        }
    }
}

/// The tile level displayed for a continuous camera zoom:
/// `floor(zoom + bias)`, clamped to `[0, max_source_zoom]`. Beyond
/// `max_source_zoom` the source tiles are *overzoomed*: rendered under the
/// deeper view transform (meshes scale with the view matrix). `bias` shifts
/// when the next level (and its detail) arrives: positive = earlier,
/// negative = later.
pub fn display_level(zoom: f64, max_source_zoom: u8, bias: f64) -> u8 {
    let level = (zoom + bias).floor().max(0.0) as i64;
    level.clamp(0, max_source_zoom.min(MAX_TILE_ZOOM) as i64) as u8
}

/// The set of source tiles covering the camera viewport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileCoverage {
    /// Source tile level actually used (post-clamp).
    pub level: u8,
    /// Covering tiles in row-major order.
    pub tiles: Vec<TileId>,
    /// True when the view is deeper than `max_source_zoom` and the tiles are
    /// rendered overzoomed.
    pub overzoomed: bool,
}

/// Tiles covering the camera's visible world rectangle at
/// [`display_level`]`(camera.zoom(), max_source_zoom, bias)`.
pub fn viewport_coverage(camera: &Camera, max_source_zoom: u8, bias: f64) -> TileCoverage {
    let level = display_level(camera.zoom(), max_source_zoom, bias);
    let overzoomed =
        (camera.zoom() + bias).floor() > max_source_zoom.min(MAX_TILE_ZOOM) as f64;
    let (min, max) = camera.visible_world_bounds();
    let n = TileId::tiles_across(level);
    let lo = TileId::containing(level, min);
    let hi = TileId::containing(level, max);
    let mut tiles = Vec::with_capacity(((hi.x - lo.x + 1) as usize) * ((hi.y - lo.y + 1) as usize));
    for y in lo.y..=hi.y.min(n - 1) {
        for x in lo.x..=hi.x.min(n - 1) {
            tiles.push(TileId { z: level, x, y });
        }
    }
    TileCoverage {
        level,
        tiles,
        overzoomed,
    }
}

/// Injected tile-byte supplier (PMTiles / SQLite live behind this in the
/// app). **Blocking** — must only be called from worker threads, never the
/// render thread.
pub trait TileSource: Send + Sync {
    /// Encoded tile bytes (MVT / PNG depending on the source), or `None` if
    /// the source has no tile at this address.
    fn tile(&self, id: TileId) -> Option<Vec<u8>>;

    /// Deepest zoom level this source holds tiles for, when the source
    /// knows it (e.g. the MBTiles `metadata.maxzoom` row). The renderer
    /// clamps its source-tile selection here and *overzooms* deeper views;
    /// `None` falls back to the renderer's configured default.
    fn max_zoom(&self) -> Option<u8> {
        None
    }
}

#[cfg(test)]
mod tests;

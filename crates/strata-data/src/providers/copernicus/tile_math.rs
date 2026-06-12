//! Pure slippy-map (XYZ / web-mercator) tile geometry. No IO.
//!
//! Conventions: tile `y` grows southward, normalized mercator coordinates
//! are in `0..1` with `(0, 0)` at the north-west corner of the world.

use std::f64::consts::PI;

use crate::domain::BoundingBox;

/// Output hillshade tiles are 256×256 px (spec 4.2 default).
pub(crate) const TILE_SIZE: u32 = 256;

/// WGS84 equatorial circumference in meters.
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_49;

/// Web-mercator latitude clamp (tiles never extend past this).
const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_59;

pub(crate) fn lon_to_x_norm(lon: f64) -> f64 {
    (lon + 180.0) / 360.0
}

pub(crate) fn lat_to_y_norm(lat: f64) -> f64 {
    let phi = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    (1.0 - phi.tan().asinh() / PI) / 2.0
}

pub(crate) fn x_norm_to_lon(x: f64) -> f64 {
    x * 360.0 - 180.0
}

pub(crate) fn y_norm_to_lat(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// Ground meters covered by one output pixel at `lat` (web-mercator is
/// conformal, so the value applies to both axes locally).
pub(crate) fn ground_resolution_m_per_px(zoom: u8, lat: f64) -> f64 {
    let world_px = (TILE_SIZE as u64) << zoom;
    EARTH_CIRCUMFERENCE_M * lat.to_radians().cos() / world_px as f64
}

/// Geographic extent of tile `(z, x, y)` as `(west, south, east, north)`
/// degrees.
pub(crate) fn tile_extent_deg(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
    let n = (1u64 << z) as f64;
    let west = x_norm_to_lon(x as f64 / n);
    let east = x_norm_to_lon((x + 1) as f64 / n);
    let north = y_norm_to_lat(y as f64 / n);
    let south = y_norm_to_lat((y + 1) as f64 / n);
    (west, south, east, north)
}

/// An inclusive rectangle of tile indices at one zoom level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileRange {
    pub z: u8,
    pub x_min: u32,
    pub x_max: u32,
    pub y_min: u32,
    pub y_max: u32,
}

impl TileRange {
    /// The tiles at `z` whose extent intersects `bbox`.
    pub(crate) fn covering(z: u8, bbox: &BoundingBox) -> Self {
        let n = 1u64 << z;
        let clamp = |v: f64| -> u32 {
            // Tile indices are < 2^z <= 2^31, so the cast is lossless.
            (v.floor().max(0.0) as u64).min(n - 1) as u32
        };
        Self {
            z,
            x_min: clamp(lon_to_x_norm(bbox.west()) * n as f64),
            x_max: clamp(lon_to_x_norm(bbox.east()) * n as f64),
            y_min: clamp(lat_to_y_norm(bbox.north()) * n as f64),
            y_max: clamp(lat_to_y_norm(bbox.south()) * n as f64),
        }
    }

    pub(crate) fn count(&self) -> usize {
        let w = (self.x_max - self.x_min + 1) as usize;
        let h = (self.y_max - self.y_min + 1) as usize;
        w * h
    }

    /// Row-major iteration (north to south, west to east) — keeps the DEM
    /// working set local while sweeping.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (self.y_min..=self.y_max)
            .flat_map(move |y| (self.x_min..=self.x_max).map(move |x| (x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(w: f64, s: f64, e: f64, n: f64) -> BoundingBox {
        BoundingBox::new(w, s, e, n).expect("valid test bbox")
    }

    #[test]
    fn norm_round_trips() {
        for lat in [-70.0, -10.0, 0.0, 47.0, 55.2, 80.0] {
            let back = y_norm_to_lat(lat_to_y_norm(lat));
            assert!((back - lat).abs() < 1e-9, "lat {lat} -> {back}");
        }
        for lon in [-179.0, -5.5, 0.0, 10.0, 15.5] {
            let back = x_norm_to_lon(lon_to_x_norm(lon));
            assert!((back - lon).abs() < 1e-9, "lon {lon} -> {back}");
        }
    }

    #[test]
    fn germany_z5_range() {
        // Hand-derived: x = floor((lon+180)/360 * 32) -> 16 (5.5°E), 17 (15.5°E);
        // y = floor(mercator_y * 32) -> 10 (55.2°N), 11 (47°N).
        let r = TileRange::covering(5, &bbox(5.5, 47.0, 15.5, 55.2));
        assert_eq!(
            r,
            TileRange { z: 5, x_min: 16, x_max: 17, y_min: 10, y_max: 11 }
        );
        assert_eq!(r.count(), 4);
    }

    #[test]
    fn small_bbox_z8_count() {
        // 10.0..10.9°E is one x column (135), 50.0..50.9°N spans y 85..86.
        let r = TileRange::covering(8, &bbox(10.0, 50.0, 10.9, 50.9));
        assert_eq!(r.count(), 2);
        assert_eq!((r.x_min, r.x_max), (135, 135));
        assert_eq!((r.y_min, r.y_max), (85, 86));
    }

    #[test]
    fn point_bbox_is_one_tile() {
        let r = TileRange::covering(11, &bbox(10.0, 50.0, 10.0, 50.0));
        assert_eq!(r.count(), 1);
    }

    #[test]
    fn tile_extent_contains_origin_point() {
        let r = TileRange::covering(9, &bbox(8.0, 50.0, 8.0, 50.0));
        let (w, s, e, n) = tile_extent_deg(9, r.x_min, r.y_min);
        assert!(w <= 8.0 && 8.0 < e);
        assert!(s < 50.0 && 50.0 <= n);
        assert!(s < n, "south {s} must be below north {n}");
    }

    #[test]
    fn ground_resolution_latitude_correction() {
        // Equator, z0: circumference / 256 px.
        let eq = ground_resolution_m_per_px(0, 0.0);
        assert!((eq - 156_543.033_928).abs() < 1e-3);
        // cos(60°) = 0.5: pixels at 60° cover half the ground distance.
        let mid = ground_resolution_m_per_px(8, 60.0);
        let ratio = mid / ground_resolution_m_per_px(8, 0.0);
        assert!((ratio - 0.5).abs() < 1e-12);
    }

    #[test]
    fn iteration_is_row_major_and_complete() {
        let r = TileRange { z: 3, x_min: 2, x_max: 4, y_min: 1, y_max: 2 };
        let tiles: Vec<_> = r.iter().collect();
        assert_eq!(tiles.len(), r.count());
        assert_eq!(tiles[0], (2, 1));
        assert_eq!(tiles[1], (3, 1));
        assert_eq!(tiles[5], (4, 2));
    }
}

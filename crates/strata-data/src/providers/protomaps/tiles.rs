//! Slippy-map (XYZ) tile arithmetic: which tiles cover a [`BoundingBox`]
//! at each zoom level.

use crate::domain::BoundingBox;

use super::TileXyz;

/// Web-Mercator latitude limit; no tiles exist beyond ±this.
const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_59;

/// Tile math is only meaningful while `2^z` fits the coordinate types;
/// matches the PMTiles format limit.
pub(super) const MAX_TILE_ZOOM: u8 = 31;

/// Inclusive rectangle of XYZ tile coordinates at a single zoom level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TileRect {
    pub z: u8,
    pub x_min: u32,
    pub x_max: u32,
    pub y_min: u32,
    pub y_max: u32,
}

impl TileRect {
    /// The tiles intersecting `bbox` at zoom `z` (clamped to
    /// [`MAX_TILE_ZOOM`]). Inclusive of tiles the bbox edge falls on.
    pub(super) fn covering(bbox: &BoundingBox, z: u8) -> Self {
        let z = z.min(MAX_TILE_ZOOM);
        let n = 1u64 << z;
        Self {
            z,
            x_min: lon_to_x(bbox.west(), n),
            x_max: lon_to_x(bbox.east(), n),
            // XYZ y grows southward: the north edge has the smaller row.
            y_min: lat_to_y(bbox.north(), n),
            y_max: lat_to_y(bbox.south(), n),
        }
    }

    pub(super) fn count(&self) -> u64 {
        u64::from(self.x_max - self.x_min + 1) * u64::from(self.y_max - self.y_min + 1)
    }

    pub(super) fn coords(self) -> impl Iterator<Item = TileXyz> {
        (self.y_min..=self.y_max)
            .flat_map(move |y| (self.x_min..=self.x_max).map(move |x| TileXyz { z: self.z, x, y }))
    }
}

/// One [`TileRect`] per zoom level, `0..=max_zoom`.
pub(super) fn pyramid(bbox: &BoundingBox, max_zoom: u8) -> Vec<TileRect> {
    (0..=max_zoom.min(MAX_TILE_ZOOM))
        .map(|z| TileRect::covering(bbox, z))
        .collect()
}

fn lon_to_x(lon: f64, n: u64) -> u32 {
    clamp_to_axis((lon + 180.0) / 360.0 * n as f64, n)
}

fn lat_to_y(lat: f64, n: u64) -> u32 {
    let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    clamp_to_axis(
        (1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * n as f64,
        n,
    )
}

/// Floors a fractional tile coordinate into the valid `0..n` axis range
/// (the ±180°/±85° edges land exactly on `n` and belong to the last tile).
fn clamp_to_axis(v: f64, n: u64) -> u32 {
    v.floor().clamp(0.0, (n - 1) as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn germany() -> BoundingBox {
        BoundingBox::new(5.5, 47.0, 15.5, 55.2).expect("valid bbox")
    }

    #[test]
    fn zoom_zero_is_one_tile() {
        let rect = TileRect::covering(&germany(), 0);
        assert_eq!(
            rect,
            TileRect {
                z: 0,
                x_min: 0,
                x_max: 0,
                y_min: 0,
                y_max: 0
            }
        );
        assert_eq!(rect.count(), 1);
    }

    #[test]
    fn germany_fits_one_tile_at_z1() {
        let rect = TileRect::covering(&germany(), 1);
        assert_eq!(
            rect,
            TileRect {
                z: 1,
                x_min: 1,
                x_max: 1,
                y_min: 0,
                y_max: 0
            }
        );
    }

    #[test]
    fn germany_z5_rect() {
        // Germany straddles the 11.25°E column boundary and one row
        // boundary at z5 — exactly 2×2 tiles.
        let rect = TileRect::covering(&germany(), 5);
        assert_eq!(
            rect,
            TileRect {
                z: 5,
                x_min: 16,
                x_max: 17,
                y_min: 10,
                y_max: 11
            }
        );
        assert_eq!(rect.count(), 4);
    }

    #[test]
    fn world_bbox_covers_everything_at_z2() {
        let world = BoundingBox::new(-180.0, -85.0, 180.0, 85.0).expect("valid bbox");
        let rect = TileRect::covering(&world, 2);
        assert_eq!(
            rect,
            TileRect {
                z: 2,
                x_min: 0,
                x_max: 3,
                y_min: 0,
                y_max: 3
            }
        );
        assert_eq!(rect.count(), 16);
    }

    #[test]
    fn tiny_bbox_yields_few_tiles_per_zoom() {
        let aachen = BoundingBox::new(6.08, 50.77, 6.09, 50.78).expect("valid bbox");
        for z in 0..=13 {
            let rect = TileRect::covering(&aachen, z);
            assert!(rect.count() <= 4, "z{z}: {} tiles", rect.count());
            let n = 1u64 << z;
            for t in rect.coords() {
                assert!(u64::from(t.x) < n && u64::from(t.y) < n);
            }
        }
    }

    #[test]
    fn pyramid_counts_accumulate() {
        let rects = pyramid(&germany(), 1);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects.iter().map(TileRect::count).sum::<u64>(), 2);

        // Every zoom level present, counts grow monotonically with zoom.
        let rects = pyramid(&germany(), 8);
        assert_eq!(rects.len(), 9);
        for pair in rects.windows(2) {
            assert!(pair[1].count() >= pair[0].count());
        }
    }

    #[test]
    fn coords_enumerates_full_rect() {
        let rect = TileRect {
            z: 3,
            x_min: 2,
            x_max: 4,
            y_min: 1,
            y_max: 2,
        };
        let coords: Vec<_> = rect.coords().collect();
        assert_eq!(coords.len() as u64, rect.count());
        assert_eq!(coords[0], TileXyz { z: 3, x: 2, y: 1 });
        assert_eq!(coords[coords.len() - 1], TileXyz { z: 3, x: 4, y: 2 });
    }
}

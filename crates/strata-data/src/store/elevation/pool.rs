//! Max-pooling DEM rasters into elevation tiles — the ingest-side builder.
//!
//! Safety contract: a pooled cell value is **never below any DEM sample
//! inside the cell** (samples are assigned by the shared [`grid`] floor
//! math and aggregated with `max`, then rounded *up* to whole meters).
//! Property-tested below over synthetic DEMs.
//!
//! [`grid`]: super::grid

use crate::domain::BoundingBox;
use crate::providers::DemTile;

use super::grid::{Cell, ElevationTileId, SIDE, cell_x_of, cell_y_of};
use super::{CELLS_PER_TILE, ELEVATION_NO_DATA, ELEVATION_TILE_SIDE, ElevationTile};

/// Accumulates the per-cell maximum of DEM samples over a tile-aligned
/// region, then slices the result into [`ElevationTile`]s.
///
/// Memory: two bytes per covered cell — ≈65 MB for the Germany region.
/// The pooler is meant for region-bbox ingests, not world-sized boxes.
pub struct ElevationPooler {
    /// South-west corner cell (tile-aligned).
    origin: Cell,
    /// Covered cells per axis (multiples of the tile side).
    width: usize,
    height: usize,
    /// Row-major from the south-west cell, [`ELEVATION_NO_DATA`] initial.
    cells: Vec<i16>,
}

impl ElevationPooler {
    /// A pooler covering every elevation tile that intersects `bbox`.
    pub fn covering(bbox: BoundingBox) -> Self {
        let side = SIDE;
        let x0 = (cell_x_of(bbox.west()) / side) * side;
        let y0 = (cell_y_of(bbox.south()) / side) * side;
        let x1 = (cell_x_of(bbox.east()) / side) * side + side;
        let y1 = (cell_y_of(bbox.north()) / side) * side + side;
        let width = (x1 - x0) as usize;
        let height = (y1 - y0) as usize;
        Self {
            origin: Cell { x: x0, y: y0 },
            width,
            height,
            cells: vec![ELEVATION_NO_DATA; width * height],
        }
    }

    /// Max-pools every sample of a decoded DEM raster into its cell.
    ///
    /// Sample positions follow the GLO-30 point-registered convention
    /// (sample `(0, 0)` on the tile's north-west integer corner, steps of
    /// `1/width` × `1/height` degrees). `NaN` samples (no data) are
    /// skipped; samples outside the pooler's coverage are ignored.
    pub fn pool_dem_tile(&mut self, tile: &DemTile) {
        let width = tile.width as usize;
        let height = tile.height as usize;
        if width == 0 || height == 0 {
            return;
        }
        if tile.elevations_m.len() != width * height {
            // Defensive: the DemTile contract promises width*height samples.
            tracing::warn!(
                tile = %tile.id,
                got = tile.elevations_m.len(),
                expected = width * height,
                "DEM tile sample count mismatch; skipping it for elevation pooling"
            );
            return;
        }

        let origin_lat = f64::from(tile.id.lat_sw) + 1.0;
        let origin_lon = f64::from(tile.id.lon_sw);
        // Column → covered-cell x offset, hoisted out of the row loop.
        let columns: Vec<Option<usize>> = (0..width)
            .map(|c| {
                let x = cell_x_of(origin_lon + c as f64 / width as f64);
                (x >= self.origin.x && x < self.origin.x + self.width as u32)
                    .then(|| (x - self.origin.x) as usize)
            })
            .collect();

        for (r, row) in tile.elevations_m.chunks_exact(width).enumerate() {
            let y = cell_y_of(origin_lat - r as f64 / height as f64);
            if y < self.origin.y || y >= self.origin.y + self.height as u32 {
                continue;
            }
            let base = (y - self.origin.y) as usize * self.width;
            for (column, &sample) in columns.iter().zip(row) {
                if let (Some(dx), Some(value)) = (column, pool_value(sample)) {
                    let slot = &mut self.cells[base + dx];
                    if value > *slot {
                        *slot = value;
                    }
                }
            }
        }
    }

    /// Slices the accumulated grid into elevation tiles, dropping tiles
    /// that hold no data at all (e.g. open sea).
    pub fn into_tiles(self) -> Vec<ElevationTile> {
        let side = ELEVATION_TILE_SIDE;
        let tiles_x = self.width / side;
        let tiles_y = self.height / side;
        let mut tiles = Vec::new();
        for by in 0..tiles_y {
            for bx in 0..tiles_x {
                let id = ElevationTileId {
                    tx: self.origin.x / SIDE + bx as u32,
                    ty: self.origin.y / SIDE + by as u32,
                };
                let mut cells = vec![ELEVATION_NO_DATA; CELLS_PER_TILE];
                let mut any_data = false;
                for row in 0..side {
                    let src = (by * side + row) * self.width + bx * side;
                    let slice = &self.cells[src..src + side];
                    if !any_data {
                        any_data = slice.iter().any(|&v| v != ELEVATION_NO_DATA);
                    }
                    cells[row * side..(row + 1) * side].copy_from_slice(slice);
                }
                if any_data {
                    // Invariant: CELLS_PER_TILE cells by construction.
                    if let Ok(tile) = ElevationTile::new(id, cells) {
                        tiles.push(tile);
                    }
                }
            }
        }
        tiles
    }
}

/// Conservative meters→cell conversion: round **up** to whole meters so the
/// stored value is never below the sample; clamp into the i16 range while
/// keeping [`ELEVATION_NO_DATA`] unreachable for real data. `NaN` (no DEM
/// data) contributes nothing.
fn pool_value(meters: f32) -> Option<i16> {
    if meters.is_nan() {
        return None;
    }
    // `as` saturates, the clamp keeps the sentinel reserved.
    let value = (meters.ceil() as i32).clamp(i32::from(i16::MIN) + 1, i32::from(i16::MAX));
    Some(value as i16)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::providers::DemTileId;

    use super::super::ElevationTileSet;
    use super::*;

    fn bbox(w: f64, s: f64, e: f64, n: f64) -> BoundingBox {
        BoundingBox::new(w, s, e, n).expect("valid test bbox")
    }

    /// A synthetic point-registered DEM tile from an analytic function,
    /// mirroring the GLO-30 layout (north-west origin, row-major).
    fn dem_tile(
        lat_sw: i16,
        lon_sw: i16,
        width: u32,
        height: u32,
        f: impl Fn(f64, f64) -> f64,
    ) -> DemTile {
        let (w, h) = (width as usize, height as usize);
        let mut elevations_m = Vec::with_capacity(w * h);
        for r in 0..h {
            let lat = (f64::from(lat_sw) + 1.0) - r as f64 / h as f64;
            for c in 0..w {
                let lon = f64::from(lon_sw) + c as f64 / w as f64;
                elevations_m.push(f(lat, lon) as f32);
            }
        }
        DemTile { id: DemTileId { lat_sw, lon_sw }, width, height, elevations_m }
    }

    /// Sample position `(r, c)` of a DEM tile — the exact same expressions
    /// the pooler evaluates, so queries land in the pooled cell.
    fn sample_pos(lat_sw: i16, lon_sw: i16, w: u32, h: u32, r: usize, c: usize) -> (f64, f64) {
        (
            (f64::from(lat_sw) + 1.0) - r as f64 / h as f64,
            f64::from(lon_sw) + c as f64 / w as f64,
        )
    }

    fn ramp(lat: f64, lon: f64) -> f64 {
        1000.0 * (lat - 50.0) + 500.0 * (lon - 10.0)
    }

    #[test]
    fn one_sample_per_cell_pools_the_exact_ceiled_value() {
        // 600×600 samples on a 1° square: exactly one sample per 6″ cell.
        let tile = dem_tile(50, 10, 600, 600, ramp);
        let mut pooler = ElevationPooler::covering(bbox(9.9, 49.9, 11.1, 51.1));
        pooler.pool_dem_tile(&tile);
        let set = ElevationTileSet::new(pooler.into_tiles());

        for (r, c) in [(300, 300), (1, 1), (599, 599), (37, 451)] {
            let (lat, lon) = sample_pos(50, 10, 600, 600, r, c);
            let want = (ramp(lat, lon) as f32).ceil() as f64;
            let got = set.max_elevation_at(lat, lon).expect("pooled cell has data");
            assert_eq!(got.0, want, "cell at sample ({r}, {c})");
        }
    }

    #[test]
    fn coarse_dem_leaves_unsampled_cells_as_no_data() {
        // 60×60 samples per degree: only every 10th cell receives one.
        let tile = dem_tile(50, 10, 60, 60, |_, _| 800.0);
        let mut pooler = ElevationPooler::covering(bbox(9.9, 49.9, 11.1, 51.1));
        pooler.pool_dem_tile(&tile);
        let set = ElevationTileSet::new(pooler.into_tiles());

        let (lat, lon) = sample_pos(50, 10, 60, 60, 30, 30);
        assert_eq!(set.max_elevation_at(lat, lon), Some(crate::domain::MetersAmsl(800.0)));
        // Mid-way between samples: a cell no sample fell into.
        assert_eq!(set.max_elevation_at(lat + 0.5 / 600.0 + 3.0 / 600.0, lon), None);
    }

    #[test]
    fn all_nan_dem_produces_no_tiles() {
        let tile = dem_tile(50, 10, 60, 60, |_, _| f64::NAN);
        let mut pooler = ElevationPooler::covering(bbox(9.9, 49.9, 11.1, 51.1));
        pooler.pool_dem_tile(&tile);
        assert!(pooler.into_tiles().is_empty());
    }

    #[test]
    fn samples_outside_coverage_are_ignored() {
        // Coverage far away from the DEM square: nothing lands.
        let tile = dem_tile(50, 10, 60, 60, |_, _| 800.0);
        let mut pooler = ElevationPooler::covering(bbox(0.0, 0.0, 1.0, 1.0));
        pooler.pool_dem_tile(&tile);
        assert!(pooler.into_tiles().is_empty());
    }

    #[test]
    fn an_off_grid_ridge_can_only_raise_cells_never_hide() {
        // A narrow 2000 m ridge along lon ≈ 10.345 on an 800 m plateau.
        let ridge = |_lat: f64, lon: f64| {
            if (10.3449..10.3451).contains(&lon) { 2000.0 } else { 800.0 }
        };
        let (w, h) = (3600, 3600);
        let tile = dem_tile(50, 10, w, h, ridge);
        let mut pooler = ElevationPooler::covering(bbox(9.9, 49.9, 11.1, 51.1));
        pooler.pool_dem_tile(&tile);
        let set = ElevationTileSet::new(pooler.into_tiles());

        // Every sample on the ridge reports at least the ridge height.
        let mut ridge_samples = 0;
        for c in 0..w as usize {
            let (lat, lon) = sample_pos(50, 10, w, h, 1800, c);
            if ridge(lat, lon) == 2000.0 {
                ridge_samples += 1;
                let got = set.max_elevation_at(lat, lon).expect("ridge cell");
                assert!(got.0 >= 2000.0, "ridge reported at {} m", got.0);
            }
        }
        assert!(ridge_samples > 0, "test ridge must hit at least one sample");
    }

    #[test]
    fn pool_value_is_conservative_and_keeps_the_sentinel_reserved() {
        assert_eq!(pool_value(f32::NAN), None);
        assert_eq!(pool_value(799.2), Some(800));
        assert_eq!(pool_value(-0.5), Some(0));
        assert_eq!(pool_value(-430.9), Some(-430));
        assert_eq!(pool_value(0.0), Some(0));
        assert_eq!(pool_value(f32::INFINITY), Some(i16::MAX));
        assert_eq!(pool_value(f32::NEG_INFINITY), Some(i16::MIN + 1));
        assert_ne!(pool_value(-40000.0), Some(ELEVATION_NO_DATA));
    }

    /// `(width, height, samples)` with NaN holes mixed in.
    fn dem_samples() -> impl Strategy<Value = (u32, u32, Vec<f32>)> {
        (4u32..=48, 4u32..=48).prop_flat_map(|(w, h)| {
            proptest::collection::vec(
                prop_oneof![
                    8 => (-500.0f64..4000.0).prop_map(|v| v as f32),
                    1 => Just(f32::NAN),
                ],
                (w * h) as usize,
            )
            .prop_map(move |samples| (w, h, samples))
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// THE safety property: after pooling, querying any DEM sample
        /// position never reports lower than the sample itself, and never
        /// reports "no data" where a sample existed.
        #[test]
        fn pooled_max_is_never_below_any_finer_sample(
            (w, h, samples) in dem_samples()
        ) {
            let tile = DemTile {
                id: DemTileId { lat_sw: 50, lon_sw: 10 },
                width: w,
                height: h,
                elevations_m: samples.clone(),
            };
            let mut pooler = ElevationPooler::covering(bbox(9.5, 49.5, 11.5, 51.5));
            pooler.pool_dem_tile(&tile);
            let set = ElevationTileSet::new(pooler.into_tiles());

            for r in 0..h as usize {
                for c in 0..w as usize {
                    let sample = samples[r * w as usize + c];
                    if sample.is_nan() {
                        continue;
                    }
                    let (lat, lon) = sample_pos(50, 10, w, h, r, c);
                    let got = set.max_elevation_at(lat, lon);
                    prop_assert!(
                        got.is_some(),
                        "sample ({r}, {c}) at ({lat}, {lon}) pooled to no-data"
                    );
                    let got = got.expect("checked above").0;
                    prop_assert!(
                        got >= f64::from(sample),
                        "pooled {got} m below sample {sample} m at ({r}, {c})"
                    );
                }
            }
        }
    }
}

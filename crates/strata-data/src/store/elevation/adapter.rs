//! Thin sampling adapter over prefetched elevation tiles.
//!
//! [`ElevationTileSet`] is pure data (`Send + Sync`, no store handle):
//! read the tiles for a corridor/route bounding box once via
//! [`Store::elevation_tiles_in_bbox`] (or [`ElevationTileSet::from_store`])
//! and sample lock-free from worker threads. This is the building block
//! for an app-side `ElevationSource` trait impl (the trait lives in
//! strata-plan; strata-data only provides the primitives).
//!
//! [`Store::elevation_tiles_in_bbox`]: crate::store::Store::elevation_tiles_in_bbox

use std::collections::HashMap;

use crate::domain::{BoundingBox, MetersAmsl};
use crate::store::{Store, StoreError};

use super::grid::{ElevationTileId, cell_of};
use super::{ElevationTile, meters};

/// An immutable, in-memory set of elevation tiles with point sampling.
#[derive(Debug, Clone, Default)]
pub struct ElevationTileSet {
    tiles: HashMap<ElevationTileId, ElevationTile>,
}

impl ElevationTileSet {
    pub fn new(tiles: impl IntoIterator<Item = ElevationTile>) -> Self {
        Self {
            tiles: tiles.into_iter().map(|t| (t.id(), t)).collect(),
        }
    }

    /// Loads every stored tile intersecting `bbox`.
    pub fn from_store(store: &Store, bbox: BoundingBox) -> Result<Self, StoreError> {
        Ok(Self::new(store.elevation_tiles_in_bbox(bbox)?))
    }

    /// Max-pooled elevation of the 6 arc-second cell containing
    /// `(lat, lon)` (finite degrees, WGS84) — same semantics as
    /// [`Store::max_elevation_at`], minus the per-call store round trip.
    /// `None` when the cell has no data or its tile is not in the set.
    ///
    /// [`Store::max_elevation_at`]: crate::store::Store::max_elevation_at
    pub fn max_elevation_at(&self, lat: f64, lon: f64) -> Option<MetersAmsl> {
        let cell = cell_of(lat, lon);
        self.tiles
            .get(&ElevationTileId::of_cell(cell))?
            .value_at(cell)
            .map(meters)
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CELLS_PER_TILE, ELEVATION_NO_DATA};
    use super::*;

    fn uniform(id: ElevationTileId, value: i16) -> ElevationTile {
        ElevationTile::new(id, vec![value; CELLS_PER_TILE]).expect("cell count")
    }

    #[test]
    fn samples_route_to_the_right_tile() {
        let a = ElevationTileId::containing(50.5, 10.5);
        let b = ElevationTileId::containing(48.0, 12.0);
        assert_ne!(a, b);
        let set = ElevationTileSet::new([uniform(a, 100), uniform(b, 2000)]);

        assert_eq!(set.max_elevation_at(50.5, 10.5), Some(MetersAmsl(100.0)));
        assert_eq!(set.max_elevation_at(48.0, 12.0), Some(MetersAmsl(2000.0)));
        assert_eq!(set.tile_count(), 2);
    }

    #[test]
    fn missing_tiles_and_sentinel_cells_sample_as_none() {
        let id = ElevationTileId::containing(50.5, 10.5);
        let set = ElevationTileSet::new([uniform(id, ELEVATION_NO_DATA)]);

        assert_eq!(set.max_elevation_at(50.5, 10.5), None, "sentinel cell");
        assert_eq!(set.max_elevation_at(0.0, 0.0), None, "tile not in set");
        assert!(ElevationTileSet::default().is_empty());
        assert_eq!(
            ElevationTileSet::default().max_elevation_at(50.5, 10.5),
            None
        );
    }
}

//! [`ElevationSource`] over the store's max-pooled elevation grid.

use std::sync::Arc;

use strata_data::domain::{BoundingBox, LatLon, MetersAmsl};
use strata_data::store::{ElevationTileSet, Store};
use strata_plan::sources::{ElevationSource, SourceError};

/// Store-backed elevation: tiles for the route's prefetch bbox are bulk-read
/// and decoded **once at construction** (on the compute thread), so the
/// corridor's thousands of per-station samples are lock-free hash lookups.
/// Points outside the prefetched coverage (an alternate far off the route)
/// fall back to a per-call store lookup — correct, just slower, and rare by
/// construction since the prefetch bbox covers route + alternates.
///
/// The decoded tile set is behind an `Arc` so consecutive compute runs can
/// reuse it instead of re-reading and re-inflating ~17 MB of tiles per
/// keystroke (see the compute orchestration's elevation cache).
pub struct StoreElevationSource {
    store: Arc<Store>,
    coverage: BoundingBox,
    tiles: Arc<ElevationTileSet>,
}

impl StoreElevationSource {
    /// Bulk-reads every stored elevation tile intersecting `bbox`.
    pub fn prefetch(store: Arc<Store>, bbox: BoundingBox) -> Result<Self, SourceError> {
        let tiles = ElevationTileSet::from_store(&store, bbox)
            .map_err(|err| SourceError::with_source("prefetching elevation tiles", err))?;
        tracing::debug!(tiles = tiles.tile_count(), "elevation tiles prefetched");
        Ok(Self {
            store,
            coverage: bbox,
            tiles: Arc::new(tiles),
        })
    }

    /// Wraps a previously prefetched tile set (`tiles` were bulk-read for
    /// `coverage`) — the cache-hit path; semantics are identical to a
    /// fresh [`Self::prefetch`] over the same bbox.
    pub fn with_tiles(store: Arc<Store>, coverage: BoundingBox, tiles: Arc<ElevationTileSet>) -> Self {
        Self {
            store,
            coverage,
            tiles,
        }
    }

    /// The coverage bbox and decoded tile set, for reuse by the next run.
    pub fn parts(&self) -> (BoundingBox, Arc<ElevationTileSet>) {
        (self.coverage, Arc::clone(&self.tiles))
    }
}

impl ElevationSource for StoreElevationSource {
    fn max_elevation_at(&self, p: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        if self.coverage.contains(p) {
            // Inside the prefetched coverage `None` is authoritative: the
            // cell genuinely has no data (sea, void, not ingested).
            return Ok(self.tiles.max_elevation_at(p.lat(), p.lon()));
        }
        self.store
            .max_elevation_at(p.lat(), p.lon())
            .map_err(|err| SourceError::with_source("elevation lookup", err))
    }
}

#[cfg(test)]
mod tests {
    use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId};

    use super::*;

    /// A store with one uniform elevation tile around (50°N, 10°E). The
    /// tempdir guard rides along to keep the directory alive.
    fn store_with_tile(value: i16) -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("store.sqlite")).unwrap();
        let id = ElevationTileId::containing(50.0, 10.0);
        let tile =
            ElevationTile::new(id, vec![value; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE]).unwrap();
        store.put_elevation_tile(&tile).unwrap();
        (dir, Arc::new(store))
    }

    #[test]
    fn samples_prefetched_tiles_inside_coverage() {
        let (_dir, store) = store_with_tile(321);
        let bbox = BoundingBox::new(9.9, 49.9, 10.1, 50.1).unwrap();
        let source = StoreElevationSource::prefetch(store, bbox).unwrap();
        let elevation = source
            .max_elevation_at(LatLon::new(50.0, 10.0).unwrap())
            .unwrap();
        assert_eq!(elevation, Some(MetersAmsl(321.0)));
    }

    #[test]
    fn outside_coverage_falls_back_to_the_store() {
        let (_dir, store) = store_with_tile(1234);
        // Prefetch a bbox far away from the stored tile…
        let bbox = BoundingBox::new(6.0, 47.0, 6.5, 47.5).unwrap();
        let source = StoreElevationSource::prefetch(store, bbox).unwrap();
        // …points inside it have no data,
        assert_eq!(
            source
                .max_elevation_at(LatLon::new(47.2, 6.2).unwrap())
                .unwrap(),
            None
        );
        // …while the out-of-coverage point still resolves via the store.
        assert_eq!(
            source
                .max_elevation_at(LatLon::new(50.0, 10.0).unwrap())
                .unwrap(),
            Some(MetersAmsl(1234.0))
        );
    }
}

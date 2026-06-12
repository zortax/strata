//! Synthetic in-memory [`TerrainProvider`] for unit tests. No network.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::Error;
use crate::domain::BoundingBox;
use crate::providers::{DemTile, DemTileId, TerrainProvider};

use super::dem_tiles_covering;

/// Serves DEM tiles computed from an analytic elevation function
/// `f(lat, lon) -> meters`, using the same point-registered grid layout as
/// real GLO-30 tiles.
pub(crate) struct SyntheticDem {
    elevation: fn(f64, f64) -> f64,
    width: u32,
    height: u32,
    pub(crate) fetches: AtomicUsize,
}

impl SyntheticDem {
    pub(crate) fn new(elevation: fn(f64, f64) -> f64, width: u32, height: u32) -> Self {
        Self {
            elevation,
            width,
            height,
            fetches: AtomicUsize::new(0),
        }
    }

    pub(crate) fn make_tile(&self, id: DemTileId) -> DemTile {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut elevations_m = Vec::with_capacity(w * h);
        for row in 0..h {
            let lat = (id.lat_sw as f64 + 1.0) - row as f64 / h as f64;
            for col in 0..w {
                let lon = id.lon_sw as f64 + col as f64 / w as f64;
                elevations_m.push((self.elevation)(lat, lon) as f32);
            }
        }
        DemTile {
            id,
            width: self.width,
            height: self.height,
            elevations_m,
        }
    }
}

#[async_trait]
impl TerrainProvider for SyntheticDem {
    fn tiles_for(&self, bbox: BoundingBox) -> Vec<DemTileId> {
        dem_tiles_covering(bbox)
    }

    async fn fetch_tile(&self, tile: DemTileId) -> Result<DemTile, Error> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(self.make_tile(tile))
    }
}

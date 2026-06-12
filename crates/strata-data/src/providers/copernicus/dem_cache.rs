//! In-memory cache of decoded (and optionally downsampled) DEM grids with
//! bilinear elevation sampling at arbitrary geographic positions.
//!
//! Grids are point-registered: sample `(0, 0)` sits exactly on the tile's
//! north-west integer corner (verified GLO-30 convention, see `geotiff.rs`).
//!
//! No-data discipline: `NaN` samples mean "the DEM has no data here" (e.g.
//! an unpublished all-sea GLO-30 square). Downsampling and bilinear
//! interpolation ignore NaN neighbours and only yield NaN where *no*
//! contributing sample has data, so holes stay holes but their edges keep
//! real elevations.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use futures::{StreamExt, TryStreamExt, stream};
use parking_lot::Mutex;

use crate::Error;
use crate::domain::LatLon;
use crate::providers::{DemTile, DemTileId, TerrainProvider};

/// Parallel in-flight DEM downloads during a prefetch.
const MAX_CONCURRENT_FETCHES: usize = 4;

/// One decoded DEM raster, downsampled by an integer factor, ready for
/// sampling. Pure data — safe to share across render threads via `Arc`.
#[derive(Debug)]
pub(crate) struct DemGrid {
    width: usize,
    height: usize,
    /// Geo position of sample (0, 0), the north-west sample center.
    origin_lat: f64,
    origin_lon: f64,
    /// Positive degrees between adjacent sample centers.
    step_lat: f64,
    step_lon: f64,
    data: Vec<f32>,
}

impl DemGrid {
    /// Builds a grid from a full-resolution tile, box-averaging
    /// `factor`×`factor` blocks (factor 1 keeps the data as-is).
    pub(crate) fn from_tile(tile: DemTile, factor: u32) -> Self {
        let width = tile.width.max(1) as usize;
        let height = tile.height.max(1) as usize;
        let f = (factor.max(1) as usize).min(width).min(height);

        let step_full_lat = 1.0 / height as f64;
        let step_full_lon = 1.0 / width as f64;
        let half = (f - 1) as f64 * 0.5;

        let mut data = tile.elevations_m;
        // Defensive: the DemTile contract promises width*height samples.
        if data.len() != width * height {
            tracing::warn!(
                tile = %tile.id,
                got = data.len(),
                expected = width * height,
                "DEM tile sample count mismatch; padding with no-data"
            );
            data.resize(width * height, f32::NAN);
        }

        let (g_width, g_height, g_data) = if f == 1 {
            (width, height, data)
        } else {
            let gw = width.div_ceil(f);
            let gh = height.div_ceil(f);
            let mut out = Vec::with_capacity(gw * gh);
            for gy in 0..gh {
                let y0 = gy * f;
                let y1 = (y0 + f).min(height);
                for gx in 0..gw {
                    let x0 = gx * f;
                    let x1 = (x0 + f).min(width);
                    let mut sum = 0.0f64;
                    let mut count = 0usize;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let v = data[y * width + x];
                            if !v.is_nan() {
                                sum += v as f64;
                                count += 1;
                            }
                        }
                    }
                    // Blocks with no data at all stay no-data.
                    out.push(if count == 0 { f32::NAN } else { (sum / count as f64) as f32 });
                }
            }
            (gw, gh, out)
        };

        Self {
            width: g_width,
            height: g_height,
            origin_lat: (tile.id.lat_sw as f64 + 1.0) - half * step_full_lat,
            origin_lon: tile.id.lon_sw as f64 + half * step_full_lon,
            step_lat: f as f64 * step_full_lat,
            step_lon: f as f64 * step_full_lon,
            data: g_data,
        }
    }

    /// Bilinear elevation in meters at `(lat, lon)` degrees, clamped to the
    /// grid extent (callers route points to the tile that contains them, so
    /// clamping only affects the outermost half sample). Returns `NaN` when
    /// every contributing sample is no-data; NaN corners with nonzero
    /// weight are skipped and the remaining weights renormalized, so data
    /// edges interpolate from real elevations only.
    pub(crate) fn sample_deg(&self, lat: f64, lon: f64) -> f64 {
        let u = ((lon - self.origin_lon) / self.step_lon).clamp(0.0, (self.width - 1) as f64);
        let v = ((self.origin_lat - lat) / self.step_lat).clamp(0.0, (self.height - 1) as f64);

        let x0 = (u.floor() as usize).min(self.width.saturating_sub(2));
        let y0 = (v.floor() as usize).min(self.height.saturating_sub(2));
        let fx = (u - x0 as f64).clamp(0.0, 1.0);
        let fy = (v - y0 as f64).clamp(0.0, 1.0);
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let at = |x: usize, y: usize| self.data[y * self.width + x] as f64;
        let corners = [
            (at(x0, y0), (1.0 - fx) * (1.0 - fy)),
            (at(x1, y0), fx * (1.0 - fy)),
            (at(x0, y1), (1.0 - fx) * fy),
            (at(x1, y1), fx * fy),
        ];
        let mut sum = 0.0;
        let mut weight = 0.0;
        for (value, w) in corners {
            if !value.is_nan() {
                sum += value * w;
                weight += w;
            }
        }
        if weight > 0.0 { sum / weight } else { f64::NAN }
    }

    fn approx_bytes(&self) -> usize {
        self.data.len() * size_of::<f32>() + size_of::<Self>()
    }
}

/// A pinned, immutable set of grids for lock-free sampling on render
/// threads. Positions without DEM data — outside every pinned grid, or on
/// no-data (NaN) samples — report `NaN` from [`Self::elevation_or_nan_deg`]
/// so the hillshade tiler can render them transparent; the plain accessors
/// substitute sea level (0 m) for callers that just want a number.
#[derive(Debug, Clone)]
pub struct ElevationSampler {
    grids: HashMap<DemTileId, Arc<DemGrid>>,
}

impl ElevationSampler {
    /// Elevation in meters AMSL at raw degree coordinates, or `NaN` where
    /// the DEM has no data.
    pub fn elevation_or_nan_deg(&self, lat: f64, lon: f64) -> f64 {
        self.grids
            .get(&containing_tile(lat, lon))
            .map_or(f64::NAN, |grid| grid.sample_deg(lat, lon))
    }

    /// Elevation in meters AMSL at raw degree coordinates; no-data
    /// positions sample as sea level (0 m).
    pub fn elevation_deg(&self, lat: f64, lon: f64) -> f64 {
        let v = self.elevation_or_nan_deg(lat, lon);
        if v.is_nan() { 0.0 } else { v }
    }

    /// Elevation in meters AMSL at a validated position (no-data → 0 m).
    pub fn elevation_at(&self, position: LatLon) -> f64 {
        self.elevation_deg(position.lat(), position.lon())
    }
}

/// The 1°×1° DEM tile whose extent `[sw, sw+1)` contains the point.
fn containing_tile(lat: f64, lon: f64) -> DemTileId {
    DemTileId {
        lat_sw: (lat.floor() as i32).clamp(-90, 89) as i16,
        lon_sw: (lon.floor() as i32).clamp(-180, 179) as i16,
    }
}

type GridKey = (DemTileId, u32);

#[derive(Default)]
struct CacheInner {
    grids: HashMap<GridKey, Arc<DemGrid>>,
    /// Least-recently-used at the front.
    lru: VecDeque<GridKey>,
    bytes: usize,
}

impl CacheInner {
    fn touch(&mut self, key: GridKey) {
        if let Some(pos) = self.lru.iter().position(|k| *k == key) {
            self.lru.remove(pos);
        }
        self.lru.push_back(key);
    }

    fn insert(&mut self, key: GridKey, grid: Arc<DemGrid>) {
        self.bytes += grid.approx_bytes();
        if let Some(old) = self.grids.insert(key, grid) {
            self.bytes = self.bytes.saturating_sub(old.approx_bytes());
        }
        self.touch(key);
    }

    /// Evicts LRU entries above `budget`, never touching `keep` (the set a
    /// caller is about to render from). The budget is therefore soft.
    fn evict_over(&mut self, budget: usize, keep: &[GridKey]) {
        let mut idx = 0;
        while self.bytes > budget && idx < self.lru.len() {
            let key = self.lru[idx];
            if keep.contains(&key) {
                idx += 1;
                continue;
            }
            self.lru.remove(idx);
            if let Some(grid) = self.grids.remove(&key) {
                self.bytes = self.bytes.saturating_sub(grid.approx_bytes());
            }
        }
    }
}

/// Byte-budgeted LRU of decoded DEM grids keyed by `(tile, factor)`.
/// Loading is async (provider fetch + decode); sampling is sync via
/// [`ElevationSampler`] snapshots.
pub struct DemCache {
    budget_bytes: usize,
    inner: Mutex<CacheInner>,
}

impl DemCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self { budget_bytes, inner: Mutex::new(CacheInner::default()) }
    }

    /// Makes every `(id, factor)` grid resident, fetching missing tiles
    /// from `provider` (up to [`MAX_CONCURRENT_FETCHES`] in flight).
    pub async fn ensure_loaded(
        &self,
        provider: &dyn TerrainProvider,
        ids: &[DemTileId],
        factor: u32,
    ) -> Result<(), Error> {
        let factor = factor.max(1);
        let missing: Vec<DemTileId> = {
            let mut inner = self.inner.lock();
            let mut missing = Vec::new();
            for &id in ids {
                if inner.grids.contains_key(&(id, factor)) {
                    inner.touch((id, factor));
                } else if !missing.contains(&id) {
                    missing.push(id);
                }
            }
            missing
        };
        if missing.is_empty() {
            return Ok(());
        }

        let fetched: Vec<DemTile> = stream::iter(missing)
            .map(|id| provider.fetch_tile(id))
            .buffer_unordered(MAX_CONCURRENT_FETCHES)
            .try_collect()
            .await?;

        let keep: Vec<GridKey> = ids.iter().map(|&id| (id, factor)).collect();
        let mut inner = self.inner.lock();
        for tile in fetched {
            let key = (tile.id, factor);
            inner.insert(key, Arc::new(DemGrid::from_tile(tile, factor)));
        }
        inner.evict_over(self.budget_bytes, &keep);
        Ok(())
    }

    /// Snapshot of the resident grids for `ids` at `factor`. Tiles that are
    /// not resident are omitted and sample as sea level.
    pub fn sampler(&self, ids: &[DemTileId], factor: u32) -> ElevationSampler {
        let factor = factor.max(1);
        let inner = self.inner.lock();
        let grids = ids
            .iter()
            .filter_map(|&id| inner.grids.get(&(id, factor)).map(|g| (id, Arc::clone(g))))
            .collect();
        ElevationSampler { grids }
    }

    /// Convenience sampling for callers outside the tiler: uses the finest
    /// resident grid of the containing tile, `None` when nothing is loaded
    /// for that tile or the DEM has no data at the position.
    pub fn elevation_at(&self, position: LatLon) -> Option<f64> {
        let id = containing_tile(position.lat(), position.lon());
        let inner = self.inner.lock();
        inner
            .grids
            .iter()
            .filter(|((tile, _), _)| *tile == id)
            .min_by_key(|((_, factor), _)| *factor)
            .map(|(_, grid)| grid.sample_deg(position.lat(), position.lon()))
            .filter(|v| !v.is_nan())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::super::testutil::SyntheticDem;
    use super::*;

    /// Linear ramp in meters: bilinear interpolation reproduces it exactly
    /// away from clamped edges.
    fn ramp(lat: f64, lon: f64) -> f64 {
        1000.0 * (lat - 50.0) + 500.0 * (lon - 10.0)
    }

    fn ramp_tile() -> DemTile {
        SyntheticDem::new(ramp, 120, 100).make_tile(DemTileId { lat_sw: 50, lon_sw: 10 })
    }

    #[test]
    fn full_resolution_sampling_matches_analytic_ramp() {
        let grid = DemGrid::from_tile(ramp_tile(), 1);
        for (lat, lon) in [(50.5, 10.5), (50.123, 10.871), (50.9, 10.05)] {
            let got = grid.sample_deg(lat, lon);
            let want = ramp(lat, lon);
            assert!((got - want).abs() < 1e-3, "({lat},{lon}): {got} vs {want}");
        }
    }

    #[test]
    fn downsampled_sampling_matches_analytic_ramp() {
        let grid = DemGrid::from_tile(ramp_tile(), 4);
        for (lat, lon) in [(50.5, 10.5), (50.25, 10.75)] {
            let got = grid.sample_deg(lat, lon);
            let want = ramp(lat, lon);
            assert!((got - want).abs() < 1e-2, "({lat},{lon}): {got} vs {want}");
        }
    }

    /// Plateau at 800 m with a no-data (NaN) hole over lon 10.4–10.6,
    /// lat 50.4–50.6.
    fn holed_plateau(lat: f64, lon: f64) -> f64 {
        if (10.4..10.6).contains(&lon) && (50.4..50.6).contains(&lat) {
            f64::NAN
        } else {
            800.0
        }
    }

    fn holed_tile() -> DemTile {
        SyntheticDem::new(holed_plateau, 120, 100).make_tile(DemTileId { lat_sw: 50, lon_sw: 10 })
    }

    #[test]
    fn no_data_holes_sample_as_nan_but_edges_keep_real_values() {
        let grid = DemGrid::from_tile(holed_tile(), 1);
        assert!(grid.sample_deg(50.5, 10.5).is_nan(), "hole center must be no-data");
        assert_eq!(grid.sample_deg(50.8, 10.8), 800.0);
        // Right at the data/no-data boundary the renormalized weights pick
        // up the real samples instead of bleeding NaN outward.
        let edge = grid.sample_deg(50.5, 10.602);
        assert!((edge - 800.0).abs() < 1e-6, "edge sample {edge} should be real data");
    }

    #[test]
    fn downsampling_ignores_no_data_and_keeps_all_nan_blocks_nan() {
        let grid = DemGrid::from_tile(holed_tile(), 4);
        // Fully inside the hole: every contributing sample is NaN.
        assert!(grid.sample_deg(50.5, 10.5).is_nan());
        // Mixed blocks at the hole edge average only the real samples.
        let near_edge = grid.sample_deg(50.5, 10.62);
        assert!((near_edge - 800.0).abs() < 1e-6, "mixed block {near_edge} should stay 800");
    }

    #[tokio::test]
    async fn elevation_at_reports_none_inside_no_data_holes() {
        let provider = SyntheticDem::new(holed_plateau, 60, 60);
        let cache = DemCache::new(64 * 1024 * 1024);
        let ids = [DemTileId { lat_sw: 50, lon_sw: 10 }];
        cache.ensure_loaded(&provider, &ids, 1).await.expect("load");

        let hole = LatLon::new(50.5, 10.5).expect("valid");
        let data = LatLon::new(50.8, 10.8).expect("valid");
        assert_eq!(cache.elevation_at(hole), None);
        assert_eq!(cache.elevation_at(data), Some(800.0));

        let sampler = cache.sampler(&ids, 1);
        assert!(sampler.elevation_or_nan_deg(50.5, 10.5).is_nan());
        assert_eq!(sampler.elevation_deg(50.5, 10.5), 0.0, "plain accessor falls back to 0 m");
        // Outside every grid: no data.
        assert!(sampler.elevation_or_nan_deg(51.5, 10.5).is_nan());
    }

    #[test]
    fn sampling_clamps_at_grid_edges() {
        let grid = DemGrid::from_tile(ramp_tile(), 1);
        // Far outside the tile: clamped to the nearest edge sample, finite.
        let v = grid.sample_deg(49.0, 9.0);
        assert!(v.is_finite());
        assert!((v - ramp(50.0 + 1.0 / 100.0, 10.0)).abs() < 1.0);
    }

    #[tokio::test]
    async fn cache_loads_once_and_samples() {
        let provider = SyntheticDem::new(ramp, 60, 60);
        let cache = DemCache::new(64 * 1024 * 1024);
        let ids = [DemTileId { lat_sw: 50, lon_sw: 10 }];

        cache.ensure_loaded(&provider, &ids, 1).await.expect("load");
        cache.ensure_loaded(&provider, &ids, 1).await.expect("reload");
        assert_eq!(provider.fetches.load(Ordering::SeqCst), 1);

        let sampler = cache.sampler(&ids, 1);
        let p = LatLon::new(50.5, 10.5).expect("valid");
        assert!((sampler.elevation_at(p) - ramp(50.5, 10.5)).abs() < 1e-3);
        // Unloaded tile -> sea level.
        assert_eq!(sampler.elevation_deg(51.5, 10.5), 0.0);
        // Convenience accessor mirrors the sampler.
        assert!(cache.elevation_at(p).is_some());
    }

    #[tokio::test]
    async fn tiny_budget_evicts_older_tiles() {
        let provider = SyntheticDem::new(ramp, 60, 60);
        let cache = DemCache::new(1); // every entry exceeds the budget
        let a = DemTileId { lat_sw: 50, lon_sw: 10 };
        let b = DemTileId { lat_sw: 50, lon_sw: 11 };

        cache.ensure_loaded(&provider, &[a], 1).await.expect("load a");
        cache.ensure_loaded(&provider, &[b], 1).await.expect("load b");

        // `b` was kept (it was the requested set), `a` was evicted.
        let in_a = LatLon::new(50.5, 10.5).expect("valid");
        let in_b = LatLon::new(50.5, 11.5).expect("valid");
        assert!(cache.elevation_at(in_a).is_none());
        assert!(cache.elevation_at(in_b).is_some());

        // Re-requesting `a` refetches.
        cache.ensure_loaded(&provider, &[a], 1).await.expect("reload a");
        assert_eq!(provider.fetches.load(Ordering::SeqCst), 3);
    }
}

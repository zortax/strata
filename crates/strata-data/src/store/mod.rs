//! Local SQLite store: feature tables with R*Tree bbox indexes, geometry
//! as postcard blobs, hillshade raster tiles, and per-(dataset, country)
//! meta. Feature rows are tagged with the country they were ingested for
//! (replace-all scope); reads stay country-agnostic.
//!
//! Concurrency: the connection sits behind a [`parking_lot::Mutex`], so
//! `Store` is `Send + Sync` and can be shared across reader threads; every
//! call holds the lock only for its own statement(s). Opening additional
//! per-thread `Store`s on the same path is equally fine (WAL journal —
//! readers don't block the ingest writer).

mod elevation;
mod hit_test;
mod meta;
mod records;
mod schema;
mod search;
mod terrain;
#[cfg(test)]
mod tests;

pub use elevation::{
    ELEVATION_CELLS_PER_DEGREE, ELEVATION_NO_DATA, ELEVATION_TILE_SIDE, ElevationPooler,
    ElevationTile, ElevationTileId, ElevationTileSet,
};

use std::fmt;
use std::path::Path;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    AiracCycle, Airport, Airspace, BoundingBox, Country, LatLon, MetersAmsl, Navaid, Obstacle,
    ReportingPoint,
};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("geometry codec: {0}")]
    Codec(#[from] postcard::Error),
    #[error("store schema corrupt or incompatible: {0}")]
    Schema(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Any feature the store can return from hit-testing or search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Feature {
    Airspace(Airspace),
    Airport(Airport),
    Navaid(Navaid),
    ReportingPoint(ReportingPoint),
    Obstacle(Obstacle),
}

impl Feature {
    /// Human-readable primary name.
    pub fn name(&self) -> &str {
        match self {
            Self::Airspace(a) => &a.name,
            Self::Airport(a) => &a.name,
            Self::Navaid(n) => &n.name,
            Self::ReportingPoint(p) => &p.name,
            Self::Obstacle(o) => o.name.as_deref().unwrap_or("Obstacle"),
        }
    }
}

/// One result of [`Store::search`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Display label, e.g. "EDDB — Berlin/Brandenburg Intl".
    pub label: String,
    /// Fly-to target (label point for area features).
    pub position: LatLon,
    pub feature: Feature,
}

/// Identifies a dataset for the meta table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dataset {
    Airspaces,
    Airports,
    Navaids,
    ReportingPoints,
    Obstacles,
    TerrainTiles,
    ElevationTiles,
    /// Vector basemap extraction (the shared MBTiles archive); a meta row
    /// per country records which countries' extracts completed.
    BasemapTiles,
}

impl Dataset {
    pub const ALL: [Dataset; 8] = [
        Self::Airspaces,
        Self::Airports,
        Self::Navaids,
        Self::ReportingPoints,
        Self::Obstacles,
        Self::TerrainTiles,
        Self::ElevationTiles,
        Self::BasemapTiles,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Airspaces => "airspaces",
            Self::Airports => "airports",
            Self::Navaids => "navaids",
            Self::ReportingPoints => "reporting_points",
            Self::Obstacles => "obstacles",
            Self::TerrainTiles => "terrain_tiles",
            Self::ElevationTiles => "elevation_tiles",
            Self::BasemapTiles => "basemap_tiles",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == s)
    }
}

impl fmt::Display for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Ingest bookkeeping per (dataset, country) — drives the AIRAC staleness
/// UI and the per-country ingest-needs inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetMeta {
    pub dataset: Dataset,
    /// The country this ingest covered (schema v3; pre-v3 rows are
    /// migrated as [`Country::DE`]).
    pub country: Country,
    /// Source label, e.g. "openaip".
    pub source: String,
    /// AIRAC cycle the data belongs to; `None` for non-cyclic datasets.
    pub airac: Option<AiracCycle>,
    pub ingested_at: DateTime<Utc>,
}

pub struct Store {
    conn: Mutex<rusqlite::Connection>,
}

impl Store {
    /// Opens (creating/migrating as needed) the store at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = schema::open(path)?;
        tracing::debug!(path = %path.display(), "store opened");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // --- batch ingest (each call replaces one country's slice of the
    //     dataset atomically; other countries' rows are untouched) ---

    pub fn insert_airspaces(
        &mut self,
        country: Country,
        items: &[Airspace],
    ) -> Result<usize, StoreError> {
        records::replace_all(&mut self.conn.lock(), country, items)
    }

    pub fn insert_airports(
        &mut self,
        country: Country,
        items: &[Airport],
    ) -> Result<usize, StoreError> {
        records::replace_all(&mut self.conn.lock(), country, items)
    }

    pub fn insert_navaids(
        &mut self,
        country: Country,
        items: &[Navaid],
    ) -> Result<usize, StoreError> {
        records::replace_all(&mut self.conn.lock(), country, items)
    }

    pub fn insert_reporting_points(
        &mut self,
        country: Country,
        items: &[ReportingPoint],
    ) -> Result<usize, StoreError> {
        records::replace_all(&mut self.conn.lock(), country, items)
    }

    pub fn insert_obstacles(
        &mut self,
        country: Country,
        items: &[Obstacle],
    ) -> Result<usize, StoreError> {
        records::replace_all(&mut self.conn.lock(), country, items)
    }

    // --- bbox queries (R*Tree-backed) ---

    pub fn airspaces_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airspace>, StoreError> {
        records::query_bbox(&self.conn.lock(), bbox)
    }

    /// Number of airspaces intersecting `bbox`, counted on the R*Tree alone
    /// (no geometry decoding — cheap even for multi-country stores). Pass a
    /// country bounding box, e.g. `Country::DE.bounding_box()`; drives
    /// the app's warm-feed threshold.
    pub fn airspace_count_in_bbox(&self, bbox: BoundingBox) -> Result<usize, StoreError> {
        records::count_bbox::<Airspace>(&self.conn.lock(), bbox)
    }

    pub fn airports_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airport>, StoreError> {
        records::query_bbox(&self.conn.lock(), bbox)
    }

    pub fn navaids_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Navaid>, StoreError> {
        records::query_bbox(&self.conn.lock(), bbox)
    }

    pub fn reporting_points_in_bbox(
        &self,
        bbox: BoundingBox,
    ) -> Result<Vec<ReportingPoint>, StoreError> {
        records::query_bbox(&self.conn.lock(), bbox)
    }

    pub fn obstacles_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Obstacle>, StoreError> {
        records::query_bbox(&self.conn.lock(), bbox)
    }

    // --- hit-testing & search ---

    /// All features at `point` (within `tolerance_deg` for point features;
    /// exact point-in-polygon for airspaces). Overlapping airspaces are the
    /// norm — the result lists every hit.
    pub fn feature_at(
        &self,
        point: LatLon,
        tolerance_deg: f64,
    ) -> Result<Vec<Feature>, StoreError> {
        hit_test::feature_at(&self.conn.lock(), point, tolerance_deg)
    }

    /// Case-insensitive prefix/substring search over idents and names.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StoreError> {
        search::search(&self.conn.lock(), query, limit)
    }

    // --- hillshade raster tiles (slippy XYZ addressing, y grows south) ---

    /// PNG bytes of a hillshade tile, if present.
    pub fn terrain_tile(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>, StoreError> {
        terrain::get(&self.conn.lock(), z, x, y)
    }

    pub fn put_terrain_tile(&mut self, z: u8, x: u32, y: u32, png: &[u8]) -> Result<(), StoreError> {
        terrain::put(&self.conn.lock(), z, x, y, png)
    }

    // --- max-pooled elevation grid (6 arc-second cells; see `elevation`
    //     module docs for the exact tile/blob format) ---

    /// One decoded elevation tile, if stored.
    pub fn elevation_tile(
        &self,
        id: ElevationTileId,
    ) -> Result<Option<ElevationTile>, StoreError> {
        elevation::get(&self.conn.lock(), id)
    }

    /// Stores (replacing) one elevation tile. Writers merging partial
    /// coverage should read + [`ElevationTile::merge_max`] first.
    pub fn put_elevation_tile(&mut self, tile: &ElevationTile) -> Result<(), StoreError> {
        elevation::put(&self.conn.lock(), tile)
    }

    /// Every stored elevation tile intersecting `bbox` — the bulk read for
    /// samplers (wrap the result in an [`ElevationTileSet`]). The raw
    /// blobs are collected under the connection lock, but the zlib
    /// decoding (tens of ms for a long route's ~100 tiles) runs after the
    /// guard drops, so concurrent users of a shared connection (hit-test,
    /// search) never queue behind the decompression.
    pub fn elevation_tiles_in_bbox(
        &self,
        bbox: BoundingBox,
    ) -> Result<Vec<ElevationTile>, StoreError> {
        let blobs = elevation::blobs_in_bbox(&self.conn.lock(), bbox)?;
        elevation::decode_blobs(blobs)
    }

    /// Max-pooled GLO-30 elevation of the 6 arc-second (~180 m) grid cell
    /// containing `(lat, lon)` (finite degrees, WGS84). `Ok(None)` means
    /// the cell has no data: open sea, a DEM void, or never ingested.
    ///
    /// Conservative by construction: the value is the **maximum** DEM
    /// sample inside the cell, rounded up to whole meters — never below
    /// the DEM-resolved terrain anywhere in the cell.
    ///
    /// Cost: one tile fetch + zlib decode per call. Fine for point
    /// lookups; bulk samplers (corridor profiles) should read once via
    /// [`Store::elevation_tiles_in_bbox`] / [`ElevationTileSet`] instead.
    pub fn max_elevation_at(
        &self,
        lat: f64,
        lon: f64,
    ) -> Result<Option<MetersAmsl>, StoreError> {
        elevation::max_at(&self.conn.lock(), lat, lon)
    }

    // --- dataset meta (per (dataset, country)) ---

    pub fn dataset_meta(
        &self,
        dataset: Dataset,
        country: Country,
    ) -> Result<Option<DatasetMeta>, StoreError> {
        meta::read(&self.conn.lock(), dataset, country)
    }

    /// Every country's meta row for `dataset`, ordered by country code.
    pub fn dataset_metas(&self, dataset: Dataset) -> Result<Vec<DatasetMeta>, StoreError> {
        meta::read_all(&self.conn.lock(), dataset)
    }

    /// Cross-country summary row for `dataset`: the row with the **oldest
    /// AIRAC cycle** (the honest staleness for a multi-country store), or —
    /// when no row carries a cycle — the most recently ingested row.
    /// `None` when the dataset was never ingested for any country.
    pub fn dataset_meta_summary(
        &self,
        dataset: Dataset,
    ) -> Result<Option<DatasetMeta>, StoreError> {
        let rows = meta::read_all(&self.conn.lock(), dataset)?;
        let oldest_cycle = rows
            .iter()
            .filter(|m| m.airac.is_some())
            .min_by_key(|m| m.airac.as_ref().map(|c| c.effective_date()))
            .cloned();
        Ok(oldest_cycle.or_else(|| rows.into_iter().max_by_key(|m| m.ingested_at)))
    }

    pub fn put_dataset_meta(&mut self, meta: &DatasetMeta) -> Result<(), StoreError> {
        meta::write(&self.conn.lock(), meta)
    }
}

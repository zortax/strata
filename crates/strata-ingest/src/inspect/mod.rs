//! Read-only staleness/necessity inspection so the app can decide whether
//! to auto-trigger ingestion. Fast: a few meta-table lookups and a handful
//! of tile counts — no network, no writes (an absent store is *not*
//! created; opening an existing store may run its cheap, additive schema
//! migration).
//!
//! Needs are computed **per (dataset family, country)** over the requested
//! country set, plus aggregated worst-case fields per family (what the
//! app's plan logic consumes — running a stage covers every requested
//! country, so the worst country decides).

use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use strata_data::domain::{AiracCycle, BoundingBox, Country};
use strata_data::providers::protomaps::Mbtiles;
use strata_data::store::{Dataset, ElevationTileId, Store};

use crate::config::IngestConfig;

/// The five openAIP datasets `aero` ingests as one unit.
const AERO_DATASETS: [Dataset; 5] = [
    Dataset::Airspaces,
    Dataset::Airports,
    Dataset::Navaids,
    Dataset::ReportingPoints,
    Dataset::Obstacles,
];

/// What `inspect` found: aggregated per dataset family (worst country
/// wins — these drive the run/don't-run plan), plus the per-country
/// breakdown.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestNeeds {
    pub aero: AeroNeed,
    pub basemap: BasemapNeed,
    pub terrain: TerrainNeed,
    pub elevation: ElevationNeed,
    /// Per-country breakdown, in the requested country order.
    pub countries: Vec<CountryNeeds>,
}

/// One country's needs across the dataset families.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryNeeds {
    pub country: Country,
    pub aero: AeroNeed,
    /// Whether the shared basemap archive covers this country (recorded
    /// per-country on extraction; pre-multi-country archives count as DE
    /// coverage).
    pub basemap: BasemapNeed,
    pub terrain: TerrainNeed,
    pub elevation: ElevationNeed,
}

impl IngestNeeds {
    /// `true` if any dataset of any requested country is missing, partial
    /// or stale — the auto-trigger predicate.
    pub fn any_needed(&self) -> bool {
        !matches!(self.aero, AeroNeed::Current(_))
            || matches!(self.basemap, BasemapNeed::Missing)
            || !matches!(self.terrain, TerrainNeed::Present { .. })
            || !matches!(self.elevation, ElevationNeed::Present { .. })
    }
}

/// State of the five openAIP datasets, treated as one unit: all five must
/// be present, and the **oldest** AIRAC cycle among them is reported.
#[derive(Debug, Clone, PartialEq)]
pub enum AeroNeed {
    /// At least one dataset has never been ingested (or the store is
    /// unreadable).
    Missing,
    /// Ingested, but a newer AIRAC cycle is in effect.
    Stale(AiracInfo),
    /// Ingested and the AIRAC cycle is current.
    Current(AiracInfo),
}

/// AIRAC provenance of the aero datasets (oldest cycle wins).
#[derive(Debug, Clone, PartialEq)]
pub struct AiracInfo {
    pub cycle: AiracCycle,
    pub ingested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BasemapNeed {
    Missing,
    /// The archive file exists and covers the country (possibly a
    /// resumable partial extraction — MBTiles cannot tell). `maxzoom`
    /// comes from the archive metadata, `None` if absent or unreadable.
    Present { maxzoom: Option<u8> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TerrainNeed {
    Missing,
    /// Tiles exist but no terrain run ever completed for **any** country
    /// (the dataset meta is only written after a pass's last tile) — an
    /// interrupted pre-completion run, deliberately not auto-restarted.
    Partial { tiles: u64 },
    /// A terrain run completed for this country; `tiles` counts all
    /// stored hillshade tiles (the table is global).
    Present { tiles: u64 },
}

/// State of the max-pooled elevation grid (written by the terrain stage,
/// backfillable via [`Ingestion::elevation`](crate::Ingestion::elevation)
/// for installs that pre-date it). An interrupted pooling run reports
/// `Missing` — the pass is cheap and idempotent, so it simply reruns.
///
/// Coverage-aware: the per-country completion meta row alone is **not**
/// trusted, because a bbox-limited run (`--bbox` smoke) writes it too.
/// `Present` additionally requires the stored tiles to reach
/// [`ELEVATION_PRESENT_PERCENT`] of the country's expected tile grid;
/// anything less is `Partial` and auto-ingest reruns the pass.
#[derive(Debug, Clone, PartialEq)]
pub enum ElevationNeed {
    /// Never ingested for this country, or the run never completed (no
    /// meta row).
    Missing,
    /// A run completed but the stored tiles cover less than
    /// [`ELEVATION_PRESENT_PERCENT`] of the country's tile-grid
    /// expectation (bbox-limited smoke runs, coverage extensions) — needs
    /// a full run.
    Partial { tiles: u64 },
    /// A completed run with (near-)full country coverage; `tiles` counts
    /// the stored tiles inside the country's tile range.
    Present { tiles: u64 },
}

/// Minimum stored share of a country's expected elevation tiles for
/// [`ElevationNeed::Present`], in percent. All-sea tiles are legitimately
/// never written — Germany stores 486 of the 500 grid tiles intersecting
/// its bbox (97.2%, the gaps are the North/Baltic Sea corners) — so the
/// threshold must absorb sea gaps while still flagging bbox-limited runs.
/// (A mostly-sea country box — none of the curated mainland boxes is —
/// would need a provider-aware expectation instead.)
const ELEVATION_PRESENT_PERCENT: u64 = 95;

/// Inspects the data dir and reports, per requested country, which
/// datasets are missing or stale, plus the aggregated worst case.
pub fn inspect(data_dir: &Path, countries: &[Country]) -> IngestNeeds {
    inspect_at(data_dir, countries, Utc::now().date_naive())
}

fn inspect_at(data_dir: &Path, countries: &[Country], today: NaiveDate) -> IngestNeeds {
    let config = IngestConfig::new(data_dir, countries.to_vec());
    let store_path = config.store_path();

    let store = if store_path.exists() {
        match Store::open(&store_path) {
            Ok(store) => Some(store),
            Err(err) => {
                tracing::warn!(%err, path = %store_path.display(),
                    "inspect: store unreadable — reporting its datasets as missing");
                None
            }
        }
    } else {
        None
    };

    // The shared archive: present (with maxzoom) under either the current
    // or the legacy name — inspection is read-only, the rename happens on
    // the next ingest/app start.
    let archive = archive_state(&config);
    let terrain_tiles = store
        .as_ref()
        .and_then(|_| terrain_tile_count(&store_path))
        .unwrap_or(0);
    // "Any completed terrain pass at all" separates an interrupted
    // pre-completion run (Partial, not auto-restarted) from a country
    // that simply was never covered (Missing).
    let any_terrain_meta = store
        .as_ref()
        .map(|s| {
            s.dataset_metas(Dataset::TerrainTiles)
                .map(|rows| !rows.is_empty())
                .unwrap_or(false)
        })
        .unwrap_or(false);

    let per_country: Vec<CountryNeeds> = countries
        .iter()
        .map(|&country| match &store {
            Some(store) => CountryNeeds {
                country,
                aero: aero_need(store, country, today),
                basemap: basemap_need(store, country, &archive),
                terrain: terrain_need(store, country, terrain_tiles, any_terrain_meta),
                elevation: elevation_need(store, &store_path, country),
            },
            None => CountryNeeds {
                country,
                aero: AeroNeed::Missing,
                basemap: match (&archive, country) {
                    // Pre-multi-country compat without a store: an archive
                    // is, by construction, a German extract.
                    (ArchiveState::Present { maxzoom }, Country::DE) => {
                        BasemapNeed::Present { maxzoom: *maxzoom }
                    }
                    _ => BasemapNeed::Missing,
                },
                terrain: TerrainNeed::Missing,
                elevation: ElevationNeed::Missing,
            },
        })
        .collect();

    IngestNeeds {
        aero: aggregate_aero(&per_country),
        basemap: aggregate_basemap(&per_country, &archive),
        terrain: aggregate_terrain(&per_country),
        elevation: aggregate_elevation(&per_country),
        countries: per_country,
    }
}

// --- per-country needs --------------------------------------------------------

fn aero_need(store: &Store, country: Country, today: NaiveDate) -> AeroNeed {
    let mut oldest: Option<AiracInfo> = None;
    for dataset in AERO_DATASETS {
        let meta = match store.dataset_meta(dataset, country) {
            Ok(Some(meta)) => meta,
            Ok(None) => return AeroNeed::Missing,
            Err(err) => {
                tracing::warn!(%err, %dataset, %country, "inspect: dataset meta unreadable");
                return AeroNeed::Missing;
            }
        };
        // Aero datasets are always AIRAC-stamped; one without a cycle
        // cannot be assessed and counts as missing.
        let Some(cycle) = meta.airac else {
            return AeroNeed::Missing;
        };
        let info = AiracInfo {
            cycle,
            ingested_at: meta.ingested_at,
        };
        let is_older = oldest
            .as_ref()
            .is_none_or(|o| info.cycle.effective_date() < o.cycle.effective_date());
        if is_older {
            oldest = Some(info);
        }
    }
    let Some(oldest) = oldest else {
        return AeroNeed::Missing;
    };
    if oldest.cycle.is_stale_at(today) {
        AeroNeed::Stale(oldest)
    } else {
        AeroNeed::Current(oldest)
    }
}

/// Shared-archive state, independent of countries.
enum ArchiveState {
    Missing,
    Present { maxzoom: Option<u8> },
}

fn archive_state(config: &IngestConfig) -> ArchiveState {
    let path = [config.basemap_path(), config.legacy_basemap_path()]
        .into_iter()
        .find(|p| p.exists());
    match path {
        None => ArchiveState::Missing,
        Some(path) => {
            let maxzoom = Mbtiles::open(&path)
                .ok()
                .and_then(|archive| archive.metadata("maxzoom").ok().flatten())
                .and_then(|z| z.parse::<u8>().ok());
            ArchiveState::Present { maxzoom }
        }
    }
}

fn basemap_need(store: &Store, country: Country, archive: &ArchiveState) -> BasemapNeed {
    let ArchiveState::Present { maxzoom } = archive else {
        return BasemapNeed::Missing;
    };
    let covered = matches!(
        store.dataset_meta(Dataset::BasemapTiles, country),
        Ok(Some(_))
    );
    // Pre-multi-country compat: archives predating the coverage meta rows
    // were German extracts by construction.
    let de_fallback = country == Country::DE
        && store
            .dataset_metas(Dataset::BasemapTiles)
            .map(|rows| rows.is_empty())
            .unwrap_or(true);
    if covered || de_fallback {
        BasemapNeed::Present { maxzoom: *maxzoom }
    } else {
        BasemapNeed::Missing
    }
}

fn terrain_need(
    store: &Store,
    country: Country,
    tiles: u64,
    any_terrain_meta: bool,
) -> TerrainNeed {
    let completed = matches!(
        store.dataset_meta(Dataset::TerrainTiles, country),
        Ok(Some(_))
    );
    match (completed, any_terrain_meta, tiles) {
        (true, _, tiles) => TerrainNeed::Present { tiles },
        // No pass completed for any country, but tiles exist: an
        // interrupted run, not auto-restarted (pre-v3 semantics).
        (false, false, tiles) if tiles > 0 => TerrainNeed::Partial { tiles },
        _ => TerrainNeed::Missing,
    }
}

fn elevation_need(store: &Store, store_path: &Path, country: Country) -> ElevationNeed {
    if !matches!(
        store.dataset_meta(Dataset::ElevationTiles, country),
        Ok(Some(_))
    ) {
        return ElevationNeed::Missing;
    }
    let (sw, ne) = country_tile_corners(country.bounding_box());
    let expected = expected_elevation_tiles(sw, ne);
    let tiles = elevation_tile_count_in(store_path, sw, ne).unwrap_or(0);
    if tiles * 100 >= expected * ELEVATION_PRESENT_PERCENT {
        ElevationNeed::Present { tiles }
    } else {
        ElevationNeed::Partial { tiles }
    }
}

// --- aggregation (worst country wins) ------------------------------------------

fn aggregate_aero(countries: &[CountryNeeds]) -> AeroNeed {
    let mut worst: Option<&AeroNeed> = None;
    for needs in countries {
        match &needs.aero {
            AeroNeed::Missing => return AeroNeed::Missing,
            need => {
                let replace = match (worst, need) {
                    (None, _) => true,
                    (Some(AeroNeed::Current(_)), AeroNeed::Stale(_)) => true,
                    (Some(AeroNeed::Current(old)), AeroNeed::Current(new))
                    | (Some(AeroNeed::Stale(old)), AeroNeed::Stale(new)) => {
                        new.cycle.effective_date() < old.cycle.effective_date()
                    }
                    _ => false,
                };
                if replace {
                    worst = Some(need);
                }
            }
        }
    }
    worst.cloned().unwrap_or(AeroNeed::Missing)
}

fn aggregate_basemap(countries: &[CountryNeeds], archive: &ArchiveState) -> BasemapNeed {
    if countries
        .iter()
        .any(|n| matches!(n.basemap, BasemapNeed::Missing))
    {
        return BasemapNeed::Missing;
    }
    match archive {
        ArchiveState::Present { maxzoom } => BasemapNeed::Present { maxzoom: *maxzoom },
        ArchiveState::Missing => BasemapNeed::Missing,
    }
}

fn aggregate_terrain(countries: &[CountryNeeds]) -> TerrainNeed {
    if countries
        .iter()
        .any(|n| matches!(n.terrain, TerrainNeed::Missing))
    {
        return TerrainNeed::Missing;
    }
    if let Some(partial) = countries
        .iter()
        .find(|n| matches!(n.terrain, TerrainNeed::Partial { .. }))
    {
        return partial.terrain.clone();
    }
    countries
        .iter()
        .map(|n| n.terrain.clone())
        .next()
        .unwrap_or(TerrainNeed::Missing)
}

fn aggregate_elevation(countries: &[CountryNeeds]) -> ElevationNeed {
    if countries
        .iter()
        .any(|n| matches!(n.elevation, ElevationNeed::Missing))
    {
        return ElevationNeed::Missing;
    }
    if let Some(partial) = countries
        .iter()
        .find(|n| matches!(n.elevation, ElevationNeed::Partial { .. }))
    {
        return partial.elevation.clone();
    }
    // All Present: report the summed per-country tile counts (overlap
    // tiles count once per country — display only).
    let mut total = 0u64;
    let mut any = false;
    for needs in countries {
        if let ElevationNeed::Present { tiles } = needs.elevation {
            total += tiles;
            any = true;
        }
    }
    if any {
        ElevationNeed::Present { tiles: total }
    } else {
        ElevationNeed::Missing
    }
}

// --- tile-count plumbing --------------------------------------------------------

/// Inclusive south-west / north-east elevation-tile corners of `bbox` —
/// the same floor math the pooler writes with
/// (`strata_data::store::elevation`), so expectation and storage agree on
/// edge tiles.
fn country_tile_corners(bbox: BoundingBox) -> (ElevationTileId, ElevationTileId) {
    (
        ElevationTileId::containing(bbox.south(), bbox.west()),
        ElevationTileId::containing(bbox.north(), bbox.east()),
    )
}

/// Tiles in the inclusive corner rectangle — what a complete country run
/// would store if every tile held land (Germany: 25 × 20 = 500).
fn expected_elevation_tiles(sw: ElevationTileId, ne: ElevationTileId) -> u64 {
    u64::from(ne.tx - sw.tx + 1) * u64::from(ne.ty - sw.ty + 1)
}

/// Counts stored hillshade tiles with a read-only connection. The Store API
/// has no row counts, so this peeks at the `terrain_tiles` table directly;
/// any error (missing table, locked db) degrades to `None`.
pub fn terrain_tile_count(store_path: &Path) -> Option<u64> {
    table_count(store_path, "terrain_tiles")
}

/// Counts stored elevation tiles — same read-only peek as
/// [`terrain_tile_count`], over `elevation_tiles`.
pub fn elevation_tile_count(store_path: &Path) -> Option<u64> {
    table_count(store_path, "elevation_tiles")
}

/// Counts stored elevation tiles inside the inclusive tile rectangle —
/// the coverage numerator. Read-only; errors degrade to `None`.
fn elevation_tile_count_in(
    store_path: &Path,
    sw: ElevationTileId,
    ne: ElevationTileId,
) -> Option<u64> {
    let conn = rusqlite::Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT count(*) FROM elevation_tiles
         WHERE tx BETWEEN ?1 AND ?2 AND ty BETWEEN ?3 AND ?4",
        rusqlite::params![
            i64::from(sw.tx),
            i64::from(ne.tx),
            i64::from(sw.ty),
            i64::from(ne.ty)
        ],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .map(|n| n.max(0) as u64)
}

fn table_count(store_path: &Path, table: &str) -> Option<u64> {
    let conn = rusqlite::Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use chrono::Days;
    use strata_data::store::DatasetMeta;
    use tempfile::TempDir;

    use super::*;

    const DE: &[Country] = &[Country::DE];
    const DE_AT: &[Country] = &[Country::DE, Country::AT];

    fn open_store(dir: &TempDir) -> Store {
        Store::open(&dir.path().join("store.sqlite")).expect("open store")
    }

    fn put_aero_meta(
        store: &mut Store,
        country: Country,
        datasets: &[Dataset],
        cycle: &AiracCycle,
    ) {
        for &dataset in datasets {
            store
                .put_dataset_meta(&DatasetMeta {
                    dataset,
                    country,
                    source: "openAIP".to_string(),
                    airac: Some(cycle.clone()),
                    ingested_at: Utc::now(),
                })
                .expect("put meta");
        }
    }

    fn put_marker_meta(store: &mut Store, dataset: Dataset, country: Country, source: &str) {
        store
            .put_dataset_meta(&DatasetMeta {
                dataset,
                country,
                source: source.to_string(),
                airac: None,
                ingested_at: Utc::now(),
            })
            .expect("put meta");
    }

    fn stale_cycle() -> AiracCycle {
        // In effect two months ago — superseded at most 28 days later.
        let then = Utc::now()
            .date_naive()
            .checked_sub_days(Days::new(60))
            .expect("valid date");
        AiracCycle::current_for(then)
    }

    /// The per-country breakdown entry for `country`.
    fn country_needs(needs: &IngestNeeds, country: Country) -> &CountryNeeds {
        needs
            .countries
            .iter()
            .find(|n| n.country == country)
            .expect("country in breakdown")
    }

    #[test]
    fn empty_data_dir_is_all_missing_and_creates_nothing() {
        let dir = TempDir::new().unwrap();

        let needs = inspect(dir.path(), DE);

        assert_eq!(needs.aero, AeroNeed::Missing);
        assert_eq!(needs.basemap, BasemapNeed::Missing);
        assert_eq!(needs.terrain, TerrainNeed::Missing);
        assert_eq!(needs.elevation, ElevationNeed::Missing);
        assert_eq!(needs.countries.len(), 1);
        assert!(needs.any_needed());
        // Inspection must not conjure a store into existence.
        assert!(!dir.path().join("store.sqlite").exists());
    }

    #[test]
    fn current_airac_reports_current() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        let cycle = AiracCycle::current();
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS, &cycle);
        drop(store);

        let needs = inspect(dir.path(), DE);

        match needs.aero {
            AeroNeed::Current(info) => assert_eq!(info.cycle, cycle),
            other => panic!("expected Current, got {other:?}"),
        }
    }

    #[test]
    fn stale_airac_reports_stale() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        let cycle = stale_cycle();
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS, &cycle);
        drop(store);

        let needs = inspect(dir.path(), DE);

        assert!(needs.any_needed());
        match needs.aero {
            AeroNeed::Stale(info) => assert_eq!(info.cycle, cycle),
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn partially_ingested_aero_is_missing() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS[..3], &AiracCycle::current());
        drop(store);

        assert_eq!(inspect(dir.path(), DE).aero, AeroNeed::Missing);
    }

    #[test]
    fn oldest_cycle_determines_staleness() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS[..4], &AiracCycle::current());
        let old = stale_cycle();
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS[4..], &old);
        drop(store);

        match inspect(dir.path(), DE).aero {
            AeroNeed::Stale(info) => assert_eq!(info.cycle, old),
            other => panic!("expected Stale with the oldest cycle, got {other:?}"),
        }
    }

    /// The country matrix for aero: DE complete + AT absent → AT missing,
    /// DE current, aggregate missing (a run is needed).
    #[test]
    fn aero_needs_are_per_country_and_aggregate_to_the_worst() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        let cycle = AiracCycle::current();
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS, &cycle);
        drop(store);

        let needs = inspect(dir.path(), DE_AT);
        assert!(matches!(
            country_needs(&needs, Country::DE).aero,
            AeroNeed::Current(_)
        ));
        assert_eq!(country_needs(&needs, Country::AT).aero, AeroNeed::Missing);
        assert_eq!(needs.aero, AeroNeed::Missing, "worst country wins");
        assert!(needs.any_needed());

        // Both ingested, one stale → aggregate is stale with that cycle.
        let mut store = open_store(&dir);
        let old = stale_cycle();
        put_aero_meta(&mut store, Country::AT, &AERO_DATASETS, &old);
        drop(store);
        let needs = inspect(dir.path(), DE_AT);
        match &needs.aero {
            AeroNeed::Stale(info) => assert_eq!(info.cycle, old),
            other => panic!("expected aggregate Stale, got {other:?}"),
        }
        assert!(matches!(
            country_needs(&needs, Country::DE).aero,
            AeroNeed::Current(_)
        ));
    }

    #[test]
    fn terrain_tiles_without_meta_are_partial() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        store.put_terrain_tile(5, 16, 10, &[1, 2, 3]).unwrap();
        store.put_terrain_tile(5, 17, 10, &[4, 5, 6]).unwrap();
        drop(store);

        assert_eq!(
            inspect(dir.path(), DE).terrain,
            TerrainNeed::Partial { tiles: 2 }
        );
    }

    #[test]
    fn terrain_with_meta_is_present() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        store.put_terrain_tile(5, 16, 10, &[1, 2, 3]).unwrap();
        put_marker_meta(
            &mut store,
            Dataset::TerrainTiles,
            Country::DE,
            "Copernicus GLO-30 hillshade",
        );
        drop(store);

        assert_eq!(
            inspect(dir.path(), DE).terrain,
            TerrainNeed::Present { tiles: 1 }
        );
    }

    /// Once any country completed terrain, an uncovered country is
    /// Missing (auto-ingested), not Partial (which would never restart).
    #[test]
    fn terrain_uncovered_country_is_missing_not_partial() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        store.put_terrain_tile(5, 16, 10, &[1]).unwrap();
        put_marker_meta(
            &mut store,
            Dataset::TerrainTiles,
            Country::DE,
            "Copernicus GLO-30 hillshade",
        );
        drop(store);

        let needs = inspect(dir.path(), DE_AT);
        assert!(matches!(
            country_needs(&needs, Country::DE).terrain,
            TerrainNeed::Present { .. }
        ));
        assert_eq!(country_needs(&needs, Country::AT).terrain, TerrainNeed::Missing);
        assert_eq!(needs.terrain, TerrainNeed::Missing, "worst country wins");
    }

    fn put_elevation_tile(store: &mut Store) {
        use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId};
        let id = ElevationTileId::containing(50.5, 10.5);
        let cells = vec![100i16; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE];
        store
            .put_elevation_tile(&ElevationTile::new(id, cells).unwrap())
            .unwrap();
    }

    fn put_elevation_meta(store: &mut Store, country: Country) {
        put_marker_meta(
            store,
            Dataset::ElevationTiles,
            country,
            "Copernicus GLO-30 max-pooled",
        );
    }

    /// Fills the country's elevation tile rectangle, skipping every
    /// `skip_every`-th tile (0 = skip none) — the skips model open-sea
    /// tiles a real run never writes. Returns the number written.
    fn put_elevation_coverage(store: &mut Store, country: Country, skip_every: u64) -> u64 {
        use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId};
        let bbox = country.bounding_box();
        let sw = ElevationTileId::containing(bbox.south(), bbox.west());
        let ne = ElevationTileId::containing(bbox.north(), bbox.east());
        let cells = vec![100i16; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE];
        let mut written = 0u64;
        let mut index = 0u64;
        for ty in sw.ty..=ne.ty {
            for tx in sw.tx..=ne.tx {
                index += 1;
                if skip_every != 0 && index.is_multiple_of(skip_every) {
                    continue;
                }
                let tile = ElevationTile::new(ElevationTileId { tx, ty }, cells.clone()).unwrap();
                store.put_elevation_tile(&tile).unwrap();
                written += 1;
            }
        }
        written
    }

    #[test]
    fn elevation_tiles_without_meta_are_missing() {
        // The run never completed — rerun it (cheap and idempotent).
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_tile(&mut store);
        drop(store);

        let needs = inspect(dir.path(), DE);
        assert_eq!(needs.elevation, ElevationNeed::Missing);
        assert!(needs.any_needed());
    }

    /// The phase-4 gate's trap: a bbox-limited smoke run writes the meta
    /// row too, so trusting it would mark elevation "ok" forever. Coverage
    /// makes it Partial — auto-ingest reruns the pass.
    #[test]
    fn bbox_limited_elevation_run_is_partial_despite_the_meta_row() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_tile(&mut store);
        put_elevation_meta(&mut store, Country::DE);
        drop(store);

        let needs = inspect(dir.path(), DE);
        assert_eq!(needs.elevation, ElevationNeed::Partial { tiles: 1 });
        assert!(needs.any_needed());
    }

    /// A completed run with zero stored tiles cannot be full coverage
    /// over a land country — Partial, not the old blanket Present.
    #[test]
    fn completed_zero_tile_elevation_run_is_partial() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_meta(&mut store, Country::DE);
        drop(store);

        assert_eq!(
            inspect(dir.path(), DE).elevation,
            ElevationNeed::Partial { tiles: 0 }
        );
    }

    /// Coverage classification across the 95% threshold: Germany's tile
    /// rectangle is 25 × 20 = 500 tiles; 90% stays Partial, 97% (the real
    /// store's sea-gap shape) is Present.
    #[test]
    fn elevation_coverage_threshold_separates_partial_from_present() {
        // Skip every 10th tile: 450 of 500 (90%) → below the threshold.
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_meta(&mut store, Country::DE);
        assert_eq!(put_elevation_coverage(&mut store, Country::DE, 10), 450);
        drop(store);
        let needs = inspect(dir.path(), DE);
        assert_eq!(needs.elevation, ElevationNeed::Partial { tiles: 450 });
        assert!(needs.any_needed());

        // Skip every 33rd tile: 485 of 500 (97%) → Present (the real
        // Germany store keeps 486 of 500; the gaps are open sea).
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_meta(&mut store, Country::DE);
        assert_eq!(put_elevation_coverage(&mut store, Country::DE, 33), 485);
        drop(store);
        assert_eq!(
            inspect(dir.path(), DE).elevation,
            ElevationNeed::Present { tiles: 485 }
        );
    }

    /// Per-country elevation: DE fully covered, AT enabled later → AT
    /// Missing, DE Present, aggregate Missing.
    #[test]
    fn elevation_needs_are_per_country() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_elevation_meta(&mut store, Country::DE);
        put_elevation_coverage(&mut store, Country::DE, 0);
        drop(store);

        let needs = inspect(dir.path(), DE_AT);
        assert!(matches!(
            country_needs(&needs, Country::DE).elevation,
            ElevationNeed::Present { .. }
        ));
        assert_eq!(
            country_needs(&needs, Country::AT).elevation,
            ElevationNeed::Missing
        );
        assert_eq!(needs.elevation, ElevationNeed::Missing);
        assert!(needs.any_needed());
    }

    #[test]
    fn basemap_archive_reports_its_maxzoom() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("basemap.mbtiles");
        let mut archive = Mbtiles::open(&path).unwrap();
        archive.set_metadata(&[("maxzoom", "13")]).unwrap();
        drop(archive);

        assert_eq!(
            inspect(dir.path(), DE).basemap,
            BasemapNeed::Present { maxzoom: Some(13) }
        );
    }

    #[test]
    fn basemap_without_maxzoom_metadata_is_present_with_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("basemap.mbtiles");
        drop(Mbtiles::open(&path).unwrap());

        assert_eq!(
            inspect(dir.path(), DE).basemap,
            BasemapNeed::Present { maxzoom: None }
        );
    }

    /// A pre-multi-country install: legacy archive name, no coverage meta
    /// rows. It must count as German coverage (no pointless re-extract)
    /// but not as coverage of any other country.
    #[test]
    fn legacy_basemap_archive_counts_as_german_coverage_only() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("basemap-de.mbtiles");
        let mut archive = Mbtiles::open(&path).unwrap();
        archive.set_metadata(&[("maxzoom", "13")]).unwrap();
        drop(archive);

        let needs = inspect(dir.path(), DE);
        assert_eq!(needs.basemap, BasemapNeed::Present { maxzoom: Some(13) });

        let needs = inspect(dir.path(), DE_AT);
        assert_eq!(
            country_needs(&needs, Country::DE).basemap,
            BasemapNeed::Present { maxzoom: Some(13) }
        );
        assert_eq!(
            country_needs(&needs, Country::AT).basemap,
            BasemapNeed::Missing
        );
        assert_eq!(needs.basemap, BasemapNeed::Missing, "worst country wins");
    }

    /// Once per-country coverage rows exist, they decide (the DE
    /// fallback no longer applies).
    #[test]
    fn basemap_coverage_meta_rows_decide_per_country() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_marker_meta(
            &mut store,
            Dataset::BasemapTiles,
            Country::AT,
            "Protomaps daily build",
        );
        drop(store);
        drop(Mbtiles::open(&dir.path().join("basemap.mbtiles")).unwrap());

        let needs = inspect(dir.path(), DE_AT);
        assert_eq!(
            country_needs(&needs, Country::AT).basemap,
            BasemapNeed::Present { maxzoom: None }
        );
        assert_eq!(
            country_needs(&needs, Country::DE).basemap,
            BasemapNeed::Missing,
            "coverage rows exist, so DE needs its own"
        );
    }

    fn make_country_complete(dir: &TempDir, country: Country) {
        let mut store = open_store(dir);
        put_aero_meta(&mut store, country, &AERO_DATASETS, &AiracCycle::current());
        store.put_terrain_tile(5, 16, 10, &[1]).unwrap();
        put_marker_meta(
            &mut store,
            Dataset::TerrainTiles,
            country,
            "Copernicus GLO-30 hillshade",
        );
        put_elevation_coverage(&mut store, country, 33);
        put_elevation_meta(&mut store, country);
        put_marker_meta(
            &mut store,
            Dataset::BasemapTiles,
            country,
            "Protomaps daily build",
        );
        drop(store);
        drop(Mbtiles::open(&dir.path().join("basemap.mbtiles")).unwrap());
    }

    #[test]
    fn nothing_needed_when_everything_is_present_and_current() {
        let dir = TempDir::new().unwrap();
        make_country_complete(&dir, Country::DE);

        assert!(!inspect(dir.path(), DE).any_needed());
    }

    /// The multi-country matrix: a fully covered DE + a fresh AT trips
    /// every aggregate; completing AT clears them all.
    #[test]
    fn enabling_a_second_country_trips_needs_until_it_is_complete() {
        let dir = TempDir::new().unwrap();
        make_country_complete(&dir, Country::DE);

        let needs = inspect(dir.path(), DE_AT);
        assert!(needs.any_needed());
        let at = country_needs(&needs, Country::AT);
        assert_eq!(at.aero, AeroNeed::Missing);
        assert_eq!(at.basemap, BasemapNeed::Missing);
        assert_eq!(at.terrain, TerrainNeed::Missing);
        assert_eq!(at.elevation, ElevationNeed::Missing);
        let de = country_needs(&needs, Country::DE);
        assert!(matches!(de.aero, AeroNeed::Current(_)));
        assert!(matches!(de.basemap, BasemapNeed::Present { .. }));
        assert!(matches!(de.terrain, TerrainNeed::Present { .. }));
        assert!(matches!(de.elevation, ElevationNeed::Present { .. }));

        make_country_complete(&dir, Country::AT);
        assert!(!inspect(dir.path(), DE_AT).any_needed());
        // And DE alone is still fine.
        assert!(!inspect(dir.path(), DE).any_needed());
    }

    /// A hillshade-era install (terrain present, no elevation grid) must
    /// trip the auto-ingest predicate so the app backfills the grid.
    #[test]
    fn missing_elevation_alone_triggers_a_need() {
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS, &AiracCycle::current());
        store.put_terrain_tile(5, 16, 10, &[1]).unwrap();
        put_marker_meta(
            &mut store,
            Dataset::TerrainTiles,
            Country::DE,
            "Copernicus GLO-30 hillshade",
        );
        put_marker_meta(
            &mut store,
            Dataset::BasemapTiles,
            Country::DE,
            "Protomaps daily build",
        );
        drop(store);
        drop(Mbtiles::open(&dir.path().join("basemap.mbtiles")).unwrap());

        let needs = inspect(dir.path(), DE);
        assert_eq!(needs.elevation, ElevationNeed::Missing);
        assert!(needs.any_needed());
    }

    #[test]
    fn fixed_date_staleness_boundary() {
        // Pure check of the date logic via inspect_at: a cycle effective
        // 2026-05-14 is superseded on 2026-06-11.
        let dir = TempDir::new().unwrap();
        let mut store = open_store(&dir);
        let cycle = AiracCycle::new(
            "2506",
            NaiveDate::from_ymd_opt(2026, 5, 14).unwrap(),
        );
        put_aero_meta(&mut store, Country::DE, &AERO_DATASETS, &cycle);
        drop(store);

        let on = |y, m, d| {
            inspect_at(dir.path(), DE, NaiveDate::from_ymd_opt(y, m, d).unwrap()).aero
        };
        assert!(matches!(on(2026, 6, 10), AeroNeed::Current(_)));
        assert!(matches!(on(2026, 6, 11), AeroNeed::Stale(_)));
    }
}

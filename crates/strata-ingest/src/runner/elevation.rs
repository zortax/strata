//! `elevation` — max-pool the Copernicus GLO-30 DEM into the store's
//! 6 arc-second elevation grid (`elevation_tiles`, see
//! `strata_data::store::elevation` for the tile format), one coverage pass
//! per configured country. Tiles straddling country-box overlaps merge via
//! max (the grid is global), so multi-country runs never clobber a
//! neighbour's data.
//!
//! Runs as the second half of the terrain stage, right after hillshading:
//! the hillshade pass has already downloaded every DEM GeoTIFF into the
//! dem-cache, so this pass decodes from disk and performs **no extra
//! downloads**. It is also exposed standalone
//! ([`Ingestion::elevation`](super::Ingestion::elevation), CLI
//! `strata-ingest elevation`) so existing installs with hillshade tiles can
//! backfill the elevation grid without re-rendering.

use std::path::PathBuf;

use chrono::Utc;
use strata_data::domain::Country;
use strata_data::providers::TerrainProvider;
use strata_data::providers::copernicus::CopernicusDem;
use strata_data::store::{Dataset, DatasetMeta, ElevationPooler, Store};

use super::Ingestion;
use crate::error::{IngestError, error_chain};
use crate::events::{IngestJob, JobHandle};

const SOURCE: &str = "Copernicus GLO-30 max-pooled";

/// Outcome of one [`Ingestion::elevation`] run (also embedded in
/// [`TerrainSummary`](super::TerrainSummary)). Counts sum over all
/// coverage passes; DEM squares shared by overlapping country boxes count
/// once per pass (they decode from the dem-cache, so repeats are cheap).
#[derive(Debug, Clone)]
pub struct ElevationSummary {
    pub store_path: PathBuf,
    /// 1°×1° DEM squares pooled.
    pub dem_tiles: usize,
    /// 256×256-cell elevation tiles written to the store.
    pub tiles_written: usize,
}

pub(super) async fn run(runner: &Ingestion) -> Result<ElevationSummary, IngestError> {
    let provider = CopernicusDem::new().with_cache_dir(runner.config().dem_cache_dir());
    run_with_provider(runner, &provider).await
}

/// Provider-injected body — unit tests pool synthetic DEMs through here.
pub(super) async fn run_with_provider(
    runner: &Ingestion,
    provider: &dyn TerrainProvider,
) -> Result<ElevationSummary, IngestError> {
    let config = runner.config();
    let store_path = config.store_path();
    let mut store = Store::open(&store_path).map_err(|source| IngestError::OpenStore {
        path: store_path.clone(),
        source,
    })?;

    let mut dem_tiles = 0usize;
    let mut tiles_written = 0usize;
    for (country, bbox) in config.coverage_passes() {
        runner.check_cancelled()?;
        let ids = provider.tiles_for(bbox);
        let total = ids.len() as u64;
        tracing::info!(
            dem_tiles = ids.len(),
            ?bbox,
            ?country,
            "max-pooling the DEM into the elevation grid"
        );

        let label = match country {
            Some(c) => format!("{} {}", IngestJob::Elevation.label(), c.code()),
            None => IngestJob::Elevation.label().to_string(),
        };
        let handle = JobHandle::start_with_label(runner.events(), IngestJob::Elevation, label);
        handle.progress(0, Some(total), "");

        let pool = async {
            let mut pooler = ElevationPooler::covering(bbox);
            for (done, &id) in ids.iter().enumerate() {
                // Decodes from the dem-cache populated by the hillshade
                // pass; only a fresh standalone run would actually
                // download.
                let tile = provider
                    .fetch_tile(id)
                    .await
                    .map_err(IngestError::ElevationPool)?;
                pooler.pool_dem_tile(&tile);
                handle.progress(done as u64 + 1, Some(total), id.to_string());
                runner.check_cancelled()?;
            }

            let mut written = 0usize;
            for mut tile in pooler.into_tiles() {
                // Merge with any stored tile so partial-coverage runs
                // (bbox smokes, neighbouring countries, reruns) never
                // clobber existing data with no-data edges.
                if let Some(existing) = store
                    .elevation_tile(tile.id())
                    .map_err(IngestError::ElevationWrite)?
                {
                    tile.merge_max(&existing);
                }
                store
                    .put_elevation_tile(&tile)
                    .map_err(IngestError::ElevationWrite)?;
                written += 1;
            }
            Ok(written)
        };
        let result = match runner.cancellable(pool).await {
            // A fetch failure racing the token reports as a plain
            // cancellation.
            Err(_) if runner.is_cancelled() => Err(IngestError::Cancelled),
            other => other,
        };
        match result {
            Ok(written) => {
                handle.finish(format!("{written} elevation tiles"));
                dem_tiles += ids.len();
                tiles_written += written;
            }
            Err(err) => {
                handle.fail(error_chain(&err));
                return Err(err);
            }
        }
        // Completed full-country pass → per-country completion marker;
        // its honesty is enforced by the coverage-aware inspection (a
        // bbox-limited run writing markers for all configured countries
        // below still reports Partial until coverage really exists).
        if let Some(country) = country {
            record_meta(&mut store, country)?;
        }
    }
    if config.bbox_overridden() {
        for &country in &config.countries {
            record_meta(&mut store, country)?;
        }
    }

    Ok(ElevationSummary {
        store_path,
        dem_tiles,
        tiles_written,
    })
}

fn record_meta(store: &mut Store, country: Country) -> Result<(), IngestError> {
    store
        .put_dataset_meta(&DatasetMeta {
            dataset: Dataset::ElevationTiles,
            country,
            source: SOURCE.to_string(),
            airac: None,
            ingested_at: Utc::now(),
        })
        .map_err(|source| IngestError::RecordMeta {
            dataset: Dataset::ElevationTiles,
            source,
        })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use strata_data::Error;
    use strata_data::domain::BoundingBox;
    use strata_data::providers::copernicus::dem_tiles_covering;
    use strata_data::providers::{DemTile, DemTileId};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::config::IngestConfig;
    use crate::events::{EventSink, IngestEvent, IngestEventReceiver};

    /// Serves point-registered DEM tiles computed from an analytic
    /// elevation function (the GLO-30 grid layout, no network).
    struct SyntheticDem {
        elevation: fn(f64, f64) -> f64,
        size: u32,
    }

    #[async_trait]
    impl TerrainProvider for SyntheticDem {
        fn tiles_for(&self, bbox: BoundingBox) -> Vec<DemTileId> {
            dem_tiles_covering(bbox)
        }

        async fn fetch_tile(&self, id: DemTileId) -> Result<DemTile, Error> {
            let n = self.size as usize;
            let mut elevations_m = Vec::with_capacity(n * n);
            for r in 0..n {
                let lat = (f64::from(id.lat_sw) + 1.0) - r as f64 / n as f64;
                for c in 0..n {
                    let lon = f64::from(id.lon_sw) + c as f64 / n as f64;
                    elevations_m.push((self.elevation)(lat, lon) as f32);
                }
            }
            Ok(DemTile {
                id,
                width: self.size,
                height: self.size,
                elevations_m,
            })
        }
    }

    fn ramp(lat: f64, lon: f64) -> f64 {
        1000.0 * (lat - 50.0) + 500.0 * (lon - 10.0)
    }

    fn runner(data_dir: &std::path::Path) -> (Ingestion, IngestEventReceiver) {
        let (sink, rx) = EventSink::channel();
        let config = IngestConfig {
            bbox_override: Some(BoundingBox::new(10.2, 50.2, 10.8, 50.8).unwrap()),
            ..IngestConfig::new(data_dir, vec![Country::DE])
        };
        (Ingestion::new(config, sink, CancellationToken::new()), rx)
    }

    fn drain(rx: &mut IngestEventReceiver) -> Vec<IngestEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn pools_a_synthetic_dem_into_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, mut rx) = runner(dir.path());
        let provider = SyntheticDem {
            elevation: ramp,
            size: 60,
        };

        let summary = run_with_provider(&runner, &provider).await.expect("run");

        assert_eq!(summary.dem_tiles, 1);
        assert!(
            summary.tiles_written > 0,
            "tiles written: {}",
            summary.tiles_written
        );

        let store = Store::open(&summary.store_path).expect("open store");
        // Completion is recorded for inspect() — for the configured
        // country, despite the bbox override (the documented trap the
        // coverage check compensates for).
        let meta = store
            .dataset_meta(Dataset::ElevationTiles, Country::DE)
            .expect("meta readable")
            .expect("meta written");
        assert_eq!(meta.source, SOURCE);
        assert_eq!(meta.country, Country::DE);

        // Sample exactly at a DEM sample position inside the bbox: the
        // store's max-pooled value is never below the sample.
        let (lat, lon) = (51.0 - 30.0 / 60.0, 10.0 + 30.0 / 60.0);
        let got = store
            .max_elevation_at(lat, lon)
            .expect("query ok")
            .expect("pooled cell has data");
        assert!(got.0 >= ramp(lat, lon), "{} < {}", got.0, ramp(lat, lon));

        // Job lifecycle: started with the bbox-pass label (no country —
        // the override is not country coverage), finished cleanly.
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(
            e,
            IngestEvent::JobStarted { job: IngestJob::Elevation, label } if label == "elevation"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            IngestEvent::JobFinished {
                job: IngestJob::Elevation,
                ..
            }
        )));
    }

    /// Two runs over adjacent DEM squares share the elevation tile that
    /// straddles the 11° E seam — the second run must merge into it, not
    /// clobber the first run's half with no-data edges.
    #[tokio::test]
    async fn rerun_over_the_adjacent_dem_square_merges_at_the_seam() {
        let dir = tempfile::tempdir().unwrap();
        let provider = SyntheticDem {
            elevation: |_, _| 500.0,
            size: 60,
        };

        let run_bbox = |bbox: BoundingBox| {
            let (sink, _rx) = EventSink::channel();
            let config = IngestConfig {
                bbox_override: Some(bbox),
                ..IngestConfig::new(dir.path(), vec![Country::DE])
            };
            Ingestion::new(config, sink, CancellationToken::new())
        };

        // First run only sees DEM square E010, second only E011.
        let first = run_bbox(BoundingBox::new(10.2, 50.2, 10.9, 50.8).unwrap());
        run_with_provider(&first, &provider)
            .await
            .expect("first run");
        let second = run_bbox(BoundingBox::new(11.1, 50.2, 11.9, 50.8).unwrap());
        run_with_provider(&second, &provider)
            .await
            .expect("second run");

        let store = Store::open(&dir.path().join("store.sqlite")).expect("open store");
        // Both sides of the seam tile carry data (DEM sample positions).
        let lat = 51.0 - 30.0 / 60.0;
        let west = 10.0 + 48.0 / 60.0; // pooled by the first run only
        let east = 11.0 + 3.0 / 60.0; // pooled by the second run only
        assert!(
            store
                .max_elevation_at(lat, west)
                .expect("query ok")
                .is_some(),
            "first run's data west of the seam survived the second run"
        );
        assert!(
            store
                .max_elevation_at(lat, east)
                .expect("query ok")
                .is_some(),
            "second run's data east of the seam was written"
        );
    }

    /// A two-country run executes one labelled pass per country and
    /// records a completion marker for each.
    #[tokio::test]
    async fn multi_country_run_passes_per_country_and_records_both() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, mut rx) = EventSink::channel();
        // Tiny neighbours: Luxembourg and Malta keep the synthetic DEM
        // grid small while exercising two distinct, disjoint passes.
        let config = IngestConfig::new(dir.path(), vec![Country::LU, Country::MT]);
        let runner = Ingestion::new(config, sink, CancellationToken::new());
        let provider = SyntheticDem {
            elevation: |_, _| 321.0,
            size: 30,
        };

        let summary = run_with_provider(&runner, &provider).await.expect("run");
        assert!(summary.tiles_written > 0);

        let store = Store::open(&summary.store_path).expect("open store");
        for country in [Country::LU, Country::MT] {
            assert!(
                store
                    .dataset_meta(Dataset::ElevationTiles, country)
                    .unwrap()
                    .is_some(),
                "{country} completion recorded"
            );
        }
        // Data of both passes is present (a point in each country).
        assert!(store.max_elevation_at(49.7, 6.1).unwrap().is_some(), "LU");
        assert!(store.max_elevation_at(35.9, 14.4).unwrap().is_some(), "MT");

        // Labels carry the country.
        let events = drain(&mut rx);
        for label_want in ["elevation LU", "elevation MT"] {
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    IngestEvent::JobStarted { job: IngestJob::Elevation, label }
                        if label == label_want
                )),
                "missing job label {label_want:?}"
            );
        }
    }

    #[tokio::test]
    async fn all_sea_dem_writes_no_tiles_but_completes() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, _rx) = runner(dir.path());
        let provider = SyntheticDem {
            elevation: |_, _| f64::NAN,
            size: 30,
        };

        let summary = run_with_provider(&runner, &provider).await.expect("run");

        assert_eq!(summary.tiles_written, 0);
        let store = Store::open(&summary.store_path).expect("open store");
        assert!(
            store
                .dataset_meta(Dataset::ElevationTiles, Country::DE)
                .unwrap()
                .is_some()
        );
        assert_eq!(store.max_elevation_at(50.5, 10.5).unwrap(), None);
    }

    #[tokio::test]
    async fn precancelled_elevation_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let (sink, _rx) = EventSink::channel();
        let config = IngestConfig::new(&data_dir, vec![Country::DE]);
        let runner = Ingestion::new(config, sink, CancellationToken::new());
        runner.cancel_token().cancel();

        let result = runner.elevation().await;

        assert!(matches!(result, Err(IngestError::Cancelled)));
        assert!(!data_dir.exists(), "no store conjured into existence");
    }
}

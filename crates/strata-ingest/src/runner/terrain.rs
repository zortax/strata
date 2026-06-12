//! `terrain` — render Copernicus GLO-30 hillshade PNG tiles into the
//! store's `terrain_tiles` dataset, one coverage pass per configured
//! country (tiles are globally addressed; overlapping country boxes
//! re-render idempotently), then max-pool the same (now disk-cached) DEM
//! into the elevation grid (see [`super::elevation`]) — one stage, one set
//! of DEM downloads.

use std::path::PathBuf;

use chrono::Utc;
use strata_data::Error;
use strata_data::domain::Country;
use strata_data::providers::copernicus::{CopernicusDem, HillshadeTiler, TerrainTile};
use strata_data::store::{Dataset, DatasetMeta, Store};

use super::Ingestion;
use super::elevation::{self, ElevationSummary};
use crate::error::{IngestError, error_chain};
use crate::events::{IngestJob, JobHandle};

const SOURCE: &str = "Copernicus GLO-30 hillshade";

/// Tiles buffered before hitting SQLite — the renderer hands tiles over in
/// bursts, and writing them chunk-wise keeps the sink callback cheap.
const WRITE_BATCH: usize = 128;

/// Outcome of one [`Ingestion::terrain`] run.
#[derive(Debug, Clone)]
pub struct TerrainSummary {
    pub store_path: PathBuf,
    pub minzoom: u8,
    pub maxzoom: u8,
    /// Hillshade tiles rendered and written, across all coverage passes.
    pub rendered: usize,
    /// The elevation-grid pass that ran in the same stage.
    pub elevation: ElevationSummary,
}

pub(super) async fn run(
    runner: &Ingestion,
    minzoom: u8,
    maxzoom: u8,
) -> Result<TerrainSummary, IngestError> {
    let config = runner.config();
    let provider = CopernicusDem::new().with_cache_dir(config.dem_cache_dir());
    let tiler = HillshadeTiler::new(minzoom, maxzoom);

    let store_path = config.store_path();
    let mut store = Store::open(&store_path).map_err(|source| IngestError::OpenStore {
        path: store_path.clone(),
        source,
    })?;

    let mut rendered = 0usize;
    for (country, bbox) in config.coverage_passes() {
        runner.check_cancelled()?;
        let total = tiler.count_tiles(bbox);
        tracing::info!(
            total,
            minzoom,
            maxzoom,
            ?bbox,
            ?country,
            dem_cache = %config.dem_cache_dir().display(),
            "rendering hillshade tiles"
        );

        let label = match country {
            Some(c) => format!("{} {}", IngestJob::Terrain.label(), c.code()),
            None => IngestJob::Terrain.label().to_string(),
        };
        let handle = JobHandle::start_with_label(runner.events(), IngestJob::Terrain, label);
        handle.progress(0, Some(total as u64), "");
        let render = async {
            let mut writer = TileWriter::new(&mut store);
            let count = tiler
                .render_tiles_with_progress(
                    &provider,
                    bbox,
                    |tile| {
                        // Between-work-units cancellation: the renderer
                        // aborts on a sink error, mapped back to
                        // `Cancelled` below.
                        if runner.is_cancelled() {
                            return Err(Error::provider("strata-ingest", "cancelled"));
                        }
                        writer.push(tile)
                    },
                    |done, _total| handle.progress(done as u64, Some(total as u64), ""),
                )
                .await
                .map_err(IngestError::TerrainRender)?;
            writer.flush().map_err(IngestError::TerrainWrite)?;
            Ok(count)
        };
        let result = match runner.cancellable(render).await {
            // The sink sentinel surfaces as a render error; report it (and
            // any failure racing the token) as a plain cancellation.
            Err(_) if runner.is_cancelled() => Err(IngestError::Cancelled),
            other => other,
        };
        match result {
            Ok(count) => {
                rendered += count;
                handle.finish("done");
            }
            Err(err) => {
                handle.fail(error_chain(&err));
                return Err(err);
            }
        }
        // Completed full-country pass → per-country completion marker.
        // (A bbox-override smoke run instead marks every configured
        // country below, preserving the pre-multi-country trap that the
        // coverage-aware elevation inspection compensates for.)
        if let Some(country) = country {
            record_meta(&mut store, country)?;
        }
    }
    if config.bbox_overridden() {
        for &country in &config.countries {
            record_meta(&mut store, country)?;
        }
    }

    // Same stage, same DEM: every GeoTIFF is in the dem-cache now, so the
    // elevation pass below decodes from disk without further downloads.
    drop(store);
    let elevation = elevation::run(runner).await?;

    Ok(TerrainSummary {
        store_path,
        minzoom,
        maxzoom,
        rendered,
        elevation,
    })
}

fn record_meta(store: &mut Store, country: Country) -> Result<(), IngestError> {
    store
        .put_dataset_meta(&DatasetMeta {
            dataset: Dataset::TerrainTiles,
            country,
            source: SOURCE.to_string(),
            airac: None,
            ingested_at: Utc::now(),
        })
        .map_err(|source| IngestError::RecordMeta {
            dataset: Dataset::TerrainTiles,
            source,
        })
}

/// Buffers rendered tiles and writes them to the store in batches, so the
/// render sink returns quickly instead of paying a SQLite commit per tile.
struct TileWriter<'a> {
    store: &'a mut Store,
    buffer: Vec<TerrainTile>,
}

impl<'a> TileWriter<'a> {
    fn new(store: &'a mut Store) -> Self {
        Self {
            store,
            buffer: Vec::with_capacity(WRITE_BATCH),
        }
    }

    fn push(&mut self, tile: TerrainTile) -> Result<(), Error> {
        self.buffer.push(tile);
        if self.buffer.len() >= WRITE_BATCH {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        for tile in self.buffer.drain(..) {
            self.store.put_terrain_tile(tile.z, tile.x, tile.y, &tile.png)?;
        }
        Ok(())
    }
}

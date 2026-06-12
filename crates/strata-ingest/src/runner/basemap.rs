//! `basemap` — extract each configured country's vector tiles from the
//! latest Protomaps daily build into **one shared** local MBTiles archive
//! (`basemap.mbtiles`). Interrupted runs resume: tiles already in the
//! archive are skipped, and overlapping country boxes merge naturally
//! (tiles are globally addressed z/x/y).
//!
//! A pre-multi-country `basemap-de.mbtiles` is renamed one-shot to the
//! shared name before extraction, so existing installs keep their tiles.

use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use strata_data::domain::Country;
use strata_data::paths;
use strata_data::providers::protomaps::ProtomapsExtractor;
use strata_data::store::{Dataset, DatasetMeta, Store};

use super::Ingestion;
use crate::error::{IngestError, error_chain};
use crate::events::{IngestJob, JobHandle, human_bytes};

const SOURCE: &str = "Protomaps daily build";

/// Outcome of one [`Ingestion::basemap`] run.
#[derive(Debug, Clone)]
pub struct BasemapSummary {
    pub dest: PathBuf,
    pub maxzoom: u8,
    /// Countries whose extraction completed this run.
    pub countries: Vec<Country>,
    /// Tiles accounted for across all passes — fetched, found absent
    /// upstream, or skipped because a previous run already stored them.
    pub tiles_done: u64,
    /// Tile bytes written during this run (excludes skipped tiles).
    pub bytes_written: u64,
}

pub(super) async fn run(runner: &Ingestion, maxzoom: u8) -> Result<BasemapSummary, IngestError> {
    let config = runner.config();
    let dest = config.basemap_path();
    fs::create_dir_all(&config.data_dir).map_err(|source| IngestError::CreateDataDir {
        path: config.data_dir.clone(),
        source,
    })?;
    // One-shot legacy rename (basemap-de.mbtiles → basemap.mbtiles) so the
    // 5+ GB German extract is reused instead of re-downloaded.
    paths::migrate_legacy_basemap(&config.data_dir);
    if dest.exists() {
        tracing::info!(
            dest = %dest.display(),
            "existing basemap archive found — resuming; previously stored tiles are skipped"
        );
    }

    let extractor = ProtomapsExtractor::new();
    let build_url = runner
        .cancellable(async {
            extractor
                .latest_build_url()
                .await
                .map_err(IngestError::ResolveBuild)
        })
        .await?;

    let mut tiles_done = 0u64;
    let mut bytes_written = 0u64;
    let mut completed = Vec::new();
    for (country, bbox) in config.coverage_passes() {
        runner.check_cancelled()?;
        let label = match country {
            Some(c) => format!("{} {}", IngestJob::Basemap.label(), c.code()),
            None => IngestJob::Basemap.label().to_string(),
        };
        tracing::info!(%build_url, maxzoom, ?bbox, ?country, "extracting basemap tiles");

        let handle = JobHandle::start_with_label(runner.events(), IngestJob::Basemap, label);
        let extract = async {
            extractor
                .extract_to_mbtiles(&build_url, bbox, maxzoom, &dest, |p| {
                    handle.progress(
                        p.tiles_done,
                        p.tiles_total,
                        format!("{} written", human_bytes(p.bytes_written)),
                    );
                })
                .await
                .map_err(IngestError::BasemapExtract)
        };
        match runner.cancellable(extract).await {
            Ok(progress) => {
                handle.finish(format!("{} written", human_bytes(progress.bytes_written)));
                tiles_done += progress.tiles_done;
                bytes_written += progress.bytes_written;
            }
            Err(err) => {
                handle.fail(error_chain(&err));
                return Err(err);
            }
        }
        // Coverage bookkeeping: a completed full-country pass is recorded
        // per (basemap_tiles, country) so `inspect` knows which countries
        // the shared archive covers. Bbox-override smoke runs record
        // nothing — they are not country coverage.
        if let Some(country) = country {
            record_meta(runner, country)?;
            completed.push(country);
        }
    }

    Ok(BasemapSummary {
        dest,
        maxzoom,
        countries: completed,
        tiles_done,
        bytes_written,
    })
}

fn record_meta(runner: &Ingestion, country: Country) -> Result<(), IngestError> {
    let store_path = runner.config().store_path();
    let mut store = Store::open(&store_path).map_err(|source| IngestError::OpenStore {
        path: store_path,
        source,
    })?;
    store
        .put_dataset_meta(&DatasetMeta {
            dataset: Dataset::BasemapTiles,
            country,
            source: SOURCE.to_string(),
            airac: None,
            ingested_at: Utc::now(),
        })
        .map_err(|source| IngestError::RecordMeta {
            dataset: Dataset::BasemapTiles,
            source,
        })
}

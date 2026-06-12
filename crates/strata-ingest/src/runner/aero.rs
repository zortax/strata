//! `aero` — fetch all five openAIP static datasets for every configured
//! country and replace each (dataset, country) slice in the store, stamped
//! with the current AIRAC cycle. Countries run sequentially (the five
//! fetches of one country run concurrently); replacing one country never
//! touches another country's rows.

use std::future::Future;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use strata_data::domain::{AiracCycle, Country};
use strata_data::providers::openaip::{NormalizationReport, OpenAipClient};
use strata_data::store::{Dataset, DatasetMeta, Store};

use super::Ingestion;
use crate::error::{IngestError, error_chain};
use crate::events::{EventSink, IngestJob, JobHandle};

const SOURCE: &str = "openAIP";

/// Outcome of one [`Ingestion::aero`] run.
#[derive(Debug, Clone)]
pub struct AeroSummary {
    pub airac: AiracCycle,
    pub store_path: PathBuf,
    /// One entry per (country, dataset), in ingest order.
    pub datasets: Vec<DatasetOutcome>,
}

#[derive(Debug, Clone)]
pub struct DatasetOutcome {
    pub dataset: Dataset,
    pub country: Country,
    pub fetched: usize,
    pub ingested: usize,
    /// `(identifier, reason)` for items that failed normalization. Partial
    /// failure is not fatal: the dataset was written without them.
    pub skipped: Vec<(String, String)>,
}

pub(super) async fn run(runner: &Ingestion) -> Result<AeroSummary, IngestError> {
    let config = runner.config();
    let api_key = config
        .openaip_api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or(IngestError::MissingApiKey)?;
    let client = OpenAipClient::new(api_key);
    if config.bbox_overridden() {
        tracing::warn!(
            countries = ?config.countries,
            "--bbox is ignored by `aero`; openAIP fetches are per-country"
        );
    }

    let airac = AiracCycle::current();
    let store_path = config.store_path();
    let mut store = Store::open(&store_path).map_err(|source| IngestError::OpenStore {
        path: store_path.clone(),
        source,
    })?;

    let mut datasets = Vec::with_capacity(5 * config.countries.len());
    for &country in &config.countries {
        runner.check_cancelled()?;
        run_country(runner, &client, &mut store, country, &airac, &mut datasets).await?;
    }

    Ok(AeroSummary {
        airac,
        store_path,
        datasets,
    })
}

/// One country: fetch the five datasets concurrently, then replace each
/// (dataset, country) slice and record its meta row.
async fn run_country(
    runner: &Ingestion,
    client: &OpenAipClient,
    store: &mut Store,
    country: Country,
    airac: &AiracCycle,
    datasets: &mut Vec<DatasetOutcome>,
) -> Result<(), IngestError> {
    let sink = runner.events();
    let fetches = async {
        tokio::try_join!(
            fetch_job(
                sink,
                IngestJob::AeroAirspaces,
                country,
                client.fetch_airspaces(country)
            ),
            fetch_job(
                sink,
                IngestJob::AeroAirports,
                country,
                client.fetch_airports(country)
            ),
            fetch_job(
                sink,
                IngestJob::AeroNavaids,
                country,
                client.fetch_navaids(country)
            ),
            fetch_job(
                sink,
                IngestJob::AeroReportingPoints,
                country,
                client.fetch_reporting_points(country)
            ),
            fetch_job(
                sink,
                IngestJob::AeroObstacles,
                country,
                client.fetch_obstacles(country)
            ),
        )
    };
    let (
        (airspaces, airspaces_rep),
        (airports, airports_rep),
        (navaids, navaids_rep),
        (points, points_rep),
        (obstacles, obstacles_rep),
    ) = runner.cancellable(fetches).await?;

    let now = Utc::now();
    runner.check_cancelled()?;
    let ingested = store.insert_airspaces(country, &airspaces)?;
    datasets.push(record(store, Dataset::Airspaces, country, ingested, airspaces_rep, airac, now)?);
    runner.check_cancelled()?;
    let ingested = store.insert_airports(country, &airports)?;
    datasets.push(record(store, Dataset::Airports, country, ingested, airports_rep, airac, now)?);
    runner.check_cancelled()?;
    let ingested = store.insert_navaids(country, &navaids)?;
    datasets.push(record(store, Dataset::Navaids, country, ingested, navaids_rep, airac, now)?);
    runner.check_cancelled()?;
    let ingested = store.insert_reporting_points(country, &points)?;
    datasets.push(record(store, Dataset::ReportingPoints, country, ingested, points_rep, airac, now)?);
    runner.check_cancelled()?;
    let ingested = store.insert_obstacles(country, &obstacles)?;
    datasets.push(record(store, Dataset::Obstacles, country, ingested, obstacles_rep, airac, now)?);
    Ok(())
}

/// Runs one fetch under a [`JobHandle`] (labelled with the country) and
/// finishes it with the normalization counts (the message the CLI spinner
/// shows).
async fn fetch_job<T>(
    sink: &EventSink,
    job: IngestJob,
    country: Country,
    fetch: impl Future<Output = Result<(Vec<T>, NormalizationReport), strata_data::Error>>,
) -> Result<(Vec<T>, NormalizationReport), IngestError> {
    let handle =
        JobHandle::start_with_label(sink, job, format!("{} {}", job.label(), country.code()));
    handle.progress(0, None, "fetching…");
    match fetch.await {
        Ok((items, report)) => {
            handle.finish(format!(
                "{} fetched, {} normalized, {} skipped",
                report.total,
                report.normalized(),
                report.skipped.len()
            ));
            Ok((items, report))
        }
        Err(source) => {
            let err = IngestError::Fetch {
                label: job.label(),
                source,
            };
            handle.fail(error_chain(&err));
            Err(err)
        }
    }
}

fn record(
    store: &mut Store,
    dataset: Dataset,
    country: Country,
    ingested: usize,
    report: NormalizationReport,
    airac: &AiracCycle,
    now: DateTime<Utc>,
) -> Result<DatasetOutcome, IngestError> {
    store
        .put_dataset_meta(&DatasetMeta {
            dataset,
            country,
            source: SOURCE.to_string(),
            airac: Some(airac.clone()),
            ingested_at: now,
        })
        .map_err(|source| IngestError::RecordMeta { dataset, source })?;
    Ok(DatasetOutcome {
        dataset,
        country,
        fetched: report.total,
        ingested,
        skipped: report.skipped,
    })
}

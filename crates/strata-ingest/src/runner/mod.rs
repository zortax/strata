//! The embeddable ingestion runner: per-dataset entry points mirroring the
//! CLI commands, progress via [`IngestEvent`]s, cooperative cancellation.

mod aero;
mod basemap;
mod elevation;
mod terrain;

use std::future::Future;

use tokio_util::sync::CancellationToken;

use crate::config::IngestConfig;
use crate::error::IngestError;
use crate::events::{EventSink, IngestEvent};

pub use aero::{AeroSummary, DatasetOutcome};
pub use basemap::BasemapSummary;
pub use elevation::ElevationSummary;
pub use terrain::TerrainSummary;

/// Zoom options for [`Ingestion::all`]; `Default` matches the CLI defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllOptions {
    pub basemap_maxzoom: u8,
    pub terrain_minzoom: u8,
    pub terrain_maxzoom: u8,
}

impl Default for AllOptions {
    fn default() -> Self {
        Self {
            basemap_maxzoom: 13,
            terrain_minzoom: 5,
            terrain_maxzoom: 11,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AllSummary {
    pub aero: AeroSummary,
    pub terrain: TerrainSummary,
    pub basemap: BasemapSummary,
}

/// Runs ingestion jobs against one data dir, reporting progress through an
/// [`EventSink`] and aborting when the [`CancellationToken`] fires.
///
/// Every entry point checks the token before starting, races it while
/// running (work is dropped at the next await point) and ends its event
/// stream with [`IngestEvent::RunFinished`] whether it succeeded, failed or
/// was cancelled.
pub struct Ingestion {
    config: IngestConfig,
    events: EventSink,
    cancel: CancellationToken,
}

impl Ingestion {
    pub fn new(config: IngestConfig, events: EventSink, cancel: CancellationToken) -> Self {
        Self {
            config,
            events,
            cancel,
        }
    }

    pub fn config(&self) -> &IngestConfig {
        &self.config
    }

    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Fetch openAIP aeronautical data (airspaces, airports, navaids,
    /// reporting points, obstacles) into the store.
    pub async fn aero(&self) -> Result<AeroSummary, IngestError> {
        self.finishing(aero::run(self)).await
    }

    /// Extract the vector basemap from the latest Protomaps build into an
    /// MBTiles file (resumes if interrupted).
    pub async fn basemap(&self, maxzoom: u8) -> Result<BasemapSummary, IngestError> {
        self.finishing(basemap::run(self, maxzoom)).await
    }

    /// Render Copernicus GLO-30 hillshade tiles into the store, then
    /// max-pool the same DEM into the elevation grid (one DEM download set
    /// for both).
    pub async fn terrain(&self, minzoom: u8, maxzoom: u8) -> Result<TerrainSummary, IngestError> {
        self.finishing(terrain::run(self, minzoom, maxzoom)).await
    }

    /// Max-pool the GLO-30 DEM into the store's elevation grid only —
    /// the backfill path for installs that already have hillshade tiles
    /// (reuses the dem-cache; downloads only what is missing from it).
    pub async fn elevation(&self) -> Result<ElevationSummary, IngestError> {
        self.finishing(elevation::run(self)).await
    }

    /// Run aero, terrain and basemap in sequence (the CLI's `all` order),
    /// with a single terminal [`IngestEvent::RunFinished`].
    pub async fn all(&self, options: AllOptions) -> Result<AllSummary, IngestError> {
        self.finishing(async {
            let aero = aero::run(self).await?;
            self.check_cancelled()?;
            let terrain =
                terrain::run(self, options.terrain_minzoom, options.terrain_maxzoom).await?;
            self.check_cancelled()?;
            let basemap = basemap::run(self, options.basemap_maxzoom).await?;
            Ok(AllSummary {
                aero,
                terrain,
                basemap,
            })
        })
        .await
    }

    /// Entry-point wrapper: pre-flight cancellation check + guaranteed
    /// `RunFinished`.
    async fn finishing<T>(
        &self,
        fut: impl Future<Output = Result<T, IngestError>>,
    ) -> Result<T, IngestError> {
        let result = async {
            self.check_cancelled()?;
            fut.await
        }
        .await;
        self.events.emit(IngestEvent::RunFinished);
        result
    }

    pub(crate) fn events(&self) -> &EventSink {
        &self.events
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Between-work-units cancellation check.
    pub(crate) fn check_cancelled(&self) -> Result<(), IngestError> {
        if self.cancel.is_cancelled() {
            Err(IngestError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Races `fut` against the cancellation token: on cancel the work
    /// future is dropped at its current await point and `Cancelled` is
    /// returned. `biased` so a pre-cancelled token aborts without polling
    /// (and thus without starting) the work.
    pub(crate) async fn cancellable<T>(
        &self,
        fut: impl Future<Output = Result<T, IngestError>>,
    ) -> Result<T, IngestError> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(IngestError::Cancelled),
            result = fut => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::Country;

    use super::*;
    use crate::events::IngestEventReceiver;

    fn runner_with_events(data_dir: &std::path::Path) -> (Ingestion, IngestEventReceiver) {
        let (sink, rx) = EventSink::channel();
        let runner = Ingestion::new(
            IngestConfig::new(data_dir, vec![Country::DE]),
            sink,
            CancellationToken::new(),
        );
        (runner, rx)
    }

    fn drain(rx: &mut IngestEventReceiver) -> Vec<IngestEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn cancellable_aborts_a_pending_future() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, _rx) = runner_with_events(dir.path());
        let cancel = runner.cancel_token().clone();

        let work =
            runner.cancellable(async { std::future::pending::<Result<(), IngestError>>().await });
        // join polls `work` first (registering the cancel waker), then the
        // second future fires the token.
        let (result, ()) = tokio::join!(work, async { cancel.cancel() });

        assert!(matches!(result, Err(IngestError::Cancelled)));
    }

    #[tokio::test]
    async fn cancellable_passes_through_completed_work() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, _rx) = runner_with_events(dir.path());

        let result = runner.cancellable(async { Ok::<_, IngestError>(42) }).await;

        assert!(matches!(result, Ok(42)));
    }

    #[tokio::test]
    async fn precancelled_runner_aborts_before_any_work() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let (runner, mut rx) = runner_with_events(&data_dir);
        runner.cancel_token().cancel();

        let result = runner.basemap(13).await;

        assert!(matches!(result, Err(IngestError::Cancelled)));
        // No work happened: the data dir was never created, and the only
        // event is the stream terminator.
        assert!(!data_dir.exists());
        assert_eq!(drain(&mut rx), vec![IngestEvent::RunFinished]);
    }

    #[tokio::test]
    async fn precancelled_all_aborts_every_stage() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let (runner, mut rx) = runner_with_events(&data_dir);
        runner.cancel_token().cancel();

        let result = runner.all(AllOptions::default()).await;

        assert!(matches!(result, Err(IngestError::Cancelled)));
        assert!(!data_dir.exists());
        assert_eq!(drain(&mut rx), vec![IngestEvent::RunFinished]);
    }

    #[tokio::test]
    async fn aero_without_api_key_fails_fast_and_terminates_the_stream() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, mut rx) = runner_with_events(dir.path());

        let result = runner.aero().await;

        assert!(matches!(result, Err(IngestError::MissingApiKey)));
        assert_eq!(drain(&mut rx), vec![IngestEvent::RunFinished]);
    }

    #[tokio::test]
    async fn blank_api_key_counts_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, _rx) = EventSink::channel();
        let config = IngestConfig {
            openaip_api_key: Some("   ".to_string()),
            ..IngestConfig::new(dir.path(), vec![Country::DE])
        };
        let runner = Ingestion::new(config, sink, CancellationToken::new());

        assert!(matches!(
            runner.aero().await,
            Err(IngestError::MissingApiKey)
        ));
    }
}

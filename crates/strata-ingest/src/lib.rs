//! Embeddable Strata ingestion: download → normalize → write the local
//! data store, with GUI-consumable progress events.
//!
//! - [`Ingestion`] runs the dataset jobs ([`aero`](Ingestion::aero),
//!   [`basemap`](Ingestion::basemap), [`terrain`](Ingestion::terrain),
//!   [`all`](Ingestion::all)), emitting [`IngestEvent`]s through an
//!   [`EventSink`] and honoring a [`CancellationToken`].
//! - [`inspect()`] reports — read-only and fast — which datasets are missing
//!   or stale, so the app can auto-trigger ingestion.
//! - The `strata-ingest` binary (feature `cli`, on by default) is a thin
//!   clap + dotenvy + indicatif wrapper over this library.
//!
//! The library never reads environment variables (the openAIP key is passed
//! in via [`IngestConfig`]) and never installs a tracing subscriber.

pub mod config;
pub mod error;
pub mod events;
pub mod inspect;
pub mod runner;

pub use config::IngestConfig;
pub use error::{IngestError, error_chain};
pub use events::{EventSink, IngestEvent, IngestEventReceiver, IngestJob};
pub use inspect::{
    AeroNeed, AiracInfo, BasemapNeed, CountryNeeds, ElevationNeed, IngestNeeds, TerrainNeed,
    inspect,
};
pub use runner::{
    AeroSummary, AllOptions, AllSummary, BasemapSummary, DatasetOutcome, ElevationSummary,
    Ingestion, TerrainSummary,
};
// Re-exported so embedders don't need their own tokio-util dependency.
pub use tokio_util::sync::CancellationToken;

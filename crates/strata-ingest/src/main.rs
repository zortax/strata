//! strata-ingest — download → normalize → write the local Strata data store.
//!
//! Thin CLI over the `strata_ingest` library: clap parsing, `.env` loading,
//! and an indicatif renderer for the library's progress events.
//!
//! Subcommands: `aero` (openAIP static data), `basemap` (Protomaps vector
//! tiles), `terrain` (Copernicus hillshade + elevation grid), `elevation`
//! (elevation-grid backfill only), `all`, `status`.

mod cli;
mod console;

use anyhow::Result;
use clap::Parser as _;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // .env first — it may carry OPENAIP_API_KEY, STRATA_DATA_DIR and RUST_LOG,
    // all read below. tracing is not up yet, so remember the outcome.
    let dotenv = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    match dotenv {
        Ok(path) => tracing::debug!(path = %path.display(), "loaded .env"),
        Err(err) if err.not_found() => {}
        Err(err) => tracing::warn!(%err, "failed to load .env"),
    }

    let cli = cli::Cli::parse();
    let config = console::resolve(&cli.global)?;
    tracing::debug!(countries = ?config.countries, data_dir = %config.data_dir.display(), "resolved configuration");

    console::run(cli.command, config).await
}

//! Library error type. Variant messages mirror the `anyhow` contexts the
//! CLI used to attach, so `strata-ingest` output stays byte-identical.

use std::path::PathBuf;

use strata_data::store::{Dataset, StoreError};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IngestError {
    /// No (usable) openAIP API key in
    /// [`IngestConfig`](crate::IngestConfig). The message names the
    /// environment variable because that is how the CLI is fed; the library
    /// itself never reads the environment.
    #[error(
        "OPENAIP_API_KEY is not set. openAIP requires a (free) API key: create one at \
         https://www.openaip.net (account → API clients), then either put \
         `OPENAIP_API_KEY=<your key>` into a .env file in the directory you run \
         strata-ingest from, or export it in your shell."
    )]
    MissingApiKey,

    #[error("fetching {label} from openAIP")]
    Fetch {
        /// Dataset label, e.g. "airspaces".
        label: &'static str,
        #[source]
        source: strata_data::Error,
    },

    #[error("opening store at {}", path.display())]
    OpenStore {
        path: PathBuf,
        #[source]
        source: StoreError,
    },

    #[error("recording {dataset} dataset metadata")]
    RecordMeta {
        dataset: Dataset,
        #[source]
        source: StoreError,
    },

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("creating data dir {}", path.display())]
    CreateDataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("resolving the latest Protomaps build")]
    ResolveBuild(#[source] strata_data::Error),

    #[error("basemap extraction failed — rerun the command to resume where it stopped")]
    BasemapExtract(#[source] strata_data::Error),

    #[error("hillshade rendering failed")]
    TerrainRender(#[source] strata_data::Error),

    #[error("writing terrain tiles to the store")]
    TerrainWrite(#[source] strata_data::Error),

    #[error("max-pooling the elevation grid failed")]
    ElevationPool(#[source] strata_data::Error),

    #[error("writing elevation tiles to the store")]
    ElevationWrite(#[source] StoreError),

    /// The [`CancellationToken`](crate::CancellationToken) fired.
    #[error("ingestion cancelled")]
    Cancelled,
}

impl IngestError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Flattens an error and its source chain into one `: `-separated string —
/// what [`IngestEvent::JobFailed`](crate::IngestEvent::JobFailed) carries.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        text.push_str(": ");
        text.push_str(&err.to_string());
        source = err.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_chain_joins_sources() {
        let source = StoreError::Schema("bad header".to_string());
        let err = IngestError::OpenStore {
            path: PathBuf::from("/data/store.sqlite"),
            source,
        };
        assert_eq!(
            error_chain(&err),
            "opening store at /data/store.sqlite: store schema corrupt or incompatible: bad header"
        );
    }

    #[test]
    fn cancelled_is_recognizable() {
        assert!(IngestError::Cancelled.is_cancelled());
        assert!(!IngestError::MissingApiKey.is_cancelled());
    }
}

//! Crate-level error type. Subsystems define their own narrow errors
//! ([`crate::store::StoreError`], [`crate::decode::DecodeError`]) which
//! convert into this one at the crate boundary.

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("store: {0}")]
    Store(#[from] crate::store::StoreError),

    #[error("provider {provider}: {source}")]
    Provider {
        provider: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("decode: {0}")]
    Decode(#[from] crate::decode::DecodeError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
}

impl Error {
    /// Wraps a provider-internal failure with the provider's name.
    pub fn provider(
        provider: &'static str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        Self::Provider {
            provider,
            source: source.into(),
        }
    }
}

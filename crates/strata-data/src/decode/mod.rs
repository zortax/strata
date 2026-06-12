//! Pure METAR/TAF text decoders.
//!
//! Tokenizer + ordered-group parsers, deliberately tolerant: real-world
//! reports (CAVOK, VRB winds, `9999`, AUTO `////` groups, colour states,
//! RVR, TEMPO/BECMG, PROB30) never fail to decode — unattributable METAR
//! tokens land in [`crate::domain::MetarDecode::unparsed_tokens`], and
//! [`DecodeError`] is reserved for fundamentally unusable input.

mod elements;
mod metar;
mod taf;
mod time;

pub use metar::decode_metar;
pub use taf::decode_taf;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("empty report")]
    Empty,
    #[error("not a {expected} report: {found:?}")]
    WrongReportType {
        expected: &'static str,
        found: String,
    },
    #[error("invalid station identifier {0:?}")]
    InvalidStation(String),
    #[error("malformed {context} token {token:?}")]
    MalformedToken {
        token: String,
        context: &'static str,
    },
    #[error("missing required {0} group")]
    Missing(&'static str),
}

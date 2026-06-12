//! NOTAM domain model + ICAO transmission-format parser.
//!
//! A [`Notam`] carries the decoded header (id, new/replace/cancel), the
//! decoded Q-line ([`QLine`]: subject/condition, traffic, purpose, scope,
//! vertical limits, centre + radius), the validity window
//! ([`NotamValidity`]: EST/PERM aware), the verbatim item bodies and the
//! full raw text — the raw report always stands, decoding only adds
//! structure on top.

mod parse;
mod qcode;
mod qline;
mod validity;

pub use qcode::{QCondition, QSubject};
pub use qline::{Purpose, QCode, QLine, Scope, Traffic};
pub use validity::{NotamEnd, NotamValidity};

pub(crate) use qline::format_centre_radius;
pub(crate) use validity::format_compact_datetime;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::IcaoCode;

/// Errors decoding a NOTAM from its transmission format.
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum NotamParseError {
    #[error("empty NOTAM text")]
    Empty,
    #[error("malformed NOTAM header {0:?}")]
    MalformedHeader(String),
    #[error("malformed NOTAM id {0:?} (expected e.g. A1234/26)")]
    MalformedId(String),
    #[error("NOTAM{kind} requires the referenced NOTAM id")]
    MissingReference { kind: char },
    #[error("missing item {0})")]
    MissingItem(char),
    #[error("malformed Q-line ({field}): {value:?}")]
    MalformedQLine { field: &'static str, value: String },
    #[error("invalid ICAO location in item A: {0:?}")]
    InvalidLocation(String),
    #[error("malformed date-time {0:?} (expected YYMMDDhhmm)")]
    MalformedDateTime(String),
}

/// A NOTAM identifier: series letter, number and two-digit year,
/// e.g. `A1234/26`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NotamId {
    /// Series letter (`A`–`Z`; Germany files aerodrome NOTAMs in A–C,
    /// en-route in D/E, warnings in W, …).
    pub series: char,
    /// Sequence number within series and year.
    pub number: u16,
    /// Two-digit year of issue (2000-pivoted).
    pub year: u8,
}

impl fmt::Display for NotamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:04}/{:02}", self.series, self.number, self.year)
    }
}

impl FromStr for NotamId {
    type Err = NotamParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || NotamParseError::MalformedId(s.to_owned());
        let (head, year) = s.split_once('/').ok_or_else(err)?;
        let mut chars = head.chars();
        let series = chars
            .next()
            .filter(char::is_ascii_uppercase)
            .ok_or_else(err)?;
        let number = chars.as_str();
        if number.is_empty() || number.len() > 4 || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        if year.len() != 2 || !year.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        Ok(Self {
            series,
            number: number.parse().map_err(|_| err())?,
            year: year.parse().map_err(|_| err())?,
        })
    }
}

/// NOTAM message kind from the header suffix (`NOTAMN`/`NOTAMR`/`NOTAMC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotamKind {
    /// `NOTAMN` — new information.
    New,
    /// `NOTAMR` — replaces an earlier NOTAM.
    Replacement { replaces: NotamId },
    /// `NOTAMC` — cancels an earlier NOTAM.
    Cancellation { cancels: NotamId },
}

impl NotamKind {
    /// The header suffix letter (`N`, `R`, `C`).
    pub fn letter(&self) -> char {
        match self {
            Self::New => 'N',
            Self::Replacement { .. } => 'R',
            Self::Cancellation { .. } => 'C',
        }
    }
}

/// Verbatim bodies of the transmitted items (Q and A–G). Optional items
/// are `None` when absent (D schedule, F/G limits — and C on a NOTAMC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotamItems {
    /// Item Q — the qualifier line.
    pub q: String,
    /// Item A — affected location(s).
    pub a: String,
    /// Item B — start of validity.
    pub b: String,
    /// Item C — end of validity.
    pub c: Option<String>,
    /// Item D — activity schedule.
    pub d: Option<String>,
    /// Item E — plain-language text.
    pub e: String,
    /// Item F — lower limit.
    pub f: Option<String>,
    /// Item G — upper limit.
    pub g: Option<String>,
}

/// A decoded NOTAM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notam {
    /// Header id, e.g. `A1234/26`.
    pub id: NotamId,
    /// New / replacement / cancellation.
    pub kind: NotamKind,
    /// Decoded Q-line (FIR, subject/condition, qualifiers, limits, circle).
    pub q: QLine,
    /// Item A — the ICAO location(s) the NOTAM applies to (aerodromes, or
    /// the FIR itself for FIR-wide NOTAMs).
    pub locations: Vec<IcaoCode>,
    /// Items B/C — validity window (EST/PERM aware).
    pub validity: NotamValidity,
    /// Item D — activity schedule within the validity window, verbatim.
    pub schedule: Option<String>,
    /// Item E — the NOTAM text, verbatim.
    pub text: String,
    /// All transmitted item bodies, verbatim.
    pub items: NotamItems,
    /// The complete NOTAM as received.
    pub raw: String,
}

impl Notam {
    /// Parses a NOTAM from its ICAO transmission format (optionally
    /// wrapped in parentheses):
    ///
    /// ```text
    /// A1234/26 NOTAMN
    /// Q) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005
    /// A) EDDF B) 2606150600 C) 2606171800
    /// E) RWY 07C/25C CLSD DUE TO RWY MAINT
    /// ```
    pub fn parse(raw: &str) -> Result<Self, NotamParseError> {
        parse::parse(raw)
    }

    /// The FIR from the Q-line.
    pub fn fir(&self) -> &IcaoCode {
        &self.q.fir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notam_id_parses_and_displays() {
        let id: NotamId = "A1234/26".parse().expect("parses");
        assert_eq!(
            id,
            NotamId {
                series: 'A',
                number: 1234,
                year: 26
            }
        );
        assert_eq!(id.to_string(), "A1234/26");
    }

    #[test]
    fn notam_id_zero_pads_short_numbers() {
        let id: NotamId = "B0612/26".parse().expect("parses");
        assert_eq!(id.number, 612);
        assert_eq!(id.to_string(), "B0612/26");
        // Unpadded input still parses, display normalizes.
        let id: NotamId = "B612/26".parse().expect("parses");
        assert_eq!(id.to_string(), "B0612/26");
    }

    #[test]
    fn notam_id_rejects_malformed_input() {
        for bad in [
            "",
            "1234/26",
            "A1234",
            "A1234/2026",
            "a1234/26",
            "A12345/26",
            "A12X4/26",
        ] {
            assert!(bad.parse::<NotamId>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn kind_letters() {
        let id: NotamId = "A0001/26".parse().expect("parses");
        assert_eq!(NotamKind::New.letter(), 'N');
        assert_eq!(NotamKind::Replacement { replaces: id }.letter(), 'R');
        assert_eq!(NotamKind::Cancellation { cancels: id }.letter(), 'C');
    }
}

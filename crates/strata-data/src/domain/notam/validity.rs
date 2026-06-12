//! NOTAM validity (items B and C): compact `YYMMDDhhmm` datetimes with
//! `EST` (estimated end) and `PERM` (permanent) handling.
//!
//! Two-digit years are pivoted into 2000–2099, the NOTAM convention since
//! the year-2000 format change (a NOTAM never refers decades into the past
//! or future).

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::NotamParseError;

/// The end of a NOTAM's validity (item C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotamEnd {
    /// Definite end time.
    At(DateTime<Utc>),
    /// Estimated end time (`EST` suffix) — the originator must replace or
    /// cancel the NOTAM; treat the estimate as the working end.
    Estimated(DateTime<Utc>),
    /// `PERM` — remains valid until cancelled or incorporated into the AIP.
    Permanent,
}

/// Validity window from items B/C. The window is half-open: active from
/// `from` (inclusive) until the end (exclusive).
///
/// An item D schedule (e.g. `DLY 0700-1500`) further restricts activity
/// *within* this window; that refinement is relevance logic and lives with
/// the consumer, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotamValidity {
    /// Item B — start of validity.
    pub from: DateTime<Utc>,
    /// Item C — end of validity. A NOTAMC carries no item C; it is mapped
    /// to [`NotamEnd::Permanent`] (a cancellation never expires).
    pub until: NotamEnd,
}

impl NotamValidity {
    /// The end instant, if bounded ([`NotamEnd::Estimated`] counts as its
    /// estimate; [`NotamEnd::Permanent`] is unbounded).
    pub fn end(&self) -> Option<DateTime<Utc>> {
        match self.until {
            NotamEnd::At(t) | NotamEnd::Estimated(t) => Some(t),
            NotamEnd::Permanent => None,
        }
    }

    /// Whether the NOTAM is valid at `t` (`from <= t < end`).
    pub fn active_at(&self, t: DateTime<Utc>) -> bool {
        t >= self.from && self.end().is_none_or(|end| t < end)
    }

    /// Whether the validity window intersects the half-open window
    /// `[from, to)`.
    pub fn overlaps(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        self.from < to && self.end().is_none_or(|end| end > from)
    }
}

/// Parses a compact `YYMMDDhhmm` NOTAM datetime (UTC).
pub(crate) fn parse_compact_datetime(s: &str) -> Result<DateTime<Utc>, NotamParseError> {
    let err = || NotamParseError::MalformedDateTime(s.to_owned());
    if s.len() != 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err());
    }
    let field = |range: std::ops::Range<usize>| -> u32 {
        // All-digit input of fixed length: the parse cannot fail.
        s[range].parse().unwrap_or(0)
    };
    let year = 2000 + field(0..2) as i32;
    let date = NaiveDate::from_ymd_opt(year, field(2..4), field(4..6)).ok_or_else(err)?;
    let datetime = date
        .and_hms_opt(field(6..8), field(8..10), 0)
        .ok_or_else(err)?;
    Ok(datetime.and_utc())
}

/// Parses an item C body: `YYMMDDhhmm`, `YYMMDDhhmm EST` / `YYMMDDhhmmEST`,
/// or `PERM`.
pub(crate) fn parse_item_c(s: &str) -> Result<NotamEnd, NotamParseError> {
    let body = s.trim();
    if body.eq_ignore_ascii_case("PERM") {
        return Ok(NotamEnd::Permanent);
    }
    if let Some(stripped) = body
        .strip_suffix("EST")
        .or_else(|| body.strip_suffix("est"))
    {
        return Ok(NotamEnd::Estimated(parse_compact_datetime(
            stripped.trim_end(),
        )?));
    }
    Ok(NotamEnd::At(parse_compact_datetime(body)?))
}

/// Renders a datetime back to the compact `YYMMDDhhmm` form (UTC).
pub(crate) fn format_compact_datetime(t: DateTime<Utc>) -> String {
    t.format("%y%m%d%H%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, mo, d)
            .and_then(|date| date.and_hms_opt(h, mi, 0))
            .expect("valid test datetime")
            .and_utc()
    }

    #[test]
    fn parses_compact_datetime() {
        assert_eq!(
            parse_compact_datetime("2606150600").expect("parses"),
            utc(2026, 6, 15, 6, 0)
        );
        assert_eq!(
            parse_compact_datetime("9912312359").expect("parses"),
            utc(2099, 12, 31, 23, 59)
        );
    }

    #[test]
    fn rejects_malformed_datetimes() {
        for bad in [
            "",
            "26061506",
            "260615060000",
            "26O6150600",
            "2613150600",
            "2606321200",
        ] {
            assert!(
                parse_compact_datetime(bad).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn item_c_definite_estimated_permanent() {
        assert_eq!(
            parse_item_c("2606171800").expect("parses"),
            NotamEnd::At(utc(2026, 6, 17, 18, 0))
        );
        assert_eq!(
            parse_item_c("2609301200EST").expect("parses"),
            NotamEnd::Estimated(utc(2026, 9, 30, 12, 0))
        );
        assert_eq!(
            parse_item_c("2609301200 EST").expect("parses"),
            NotamEnd::Estimated(utc(2026, 9, 30, 12, 0))
        );
        assert_eq!(parse_item_c("PERM").expect("parses"), NotamEnd::Permanent);
        assert_eq!(parse_item_c(" perm ").expect("parses"), NotamEnd::Permanent);
    }

    #[test]
    fn active_at_is_half_open() {
        let validity = NotamValidity {
            from: utc(2026, 6, 15, 6, 0),
            until: NotamEnd::At(utc(2026, 6, 17, 18, 0)),
        };
        assert!(!validity.active_at(utc(2026, 6, 15, 5, 59)));
        assert!(validity.active_at(utc(2026, 6, 15, 6, 0)));
        assert!(validity.active_at(utc(2026, 6, 16, 12, 0)));
        assert!(!validity.active_at(utc(2026, 6, 17, 18, 0)));
    }

    #[test]
    fn permanent_validity_never_ends() {
        let validity = NotamValidity {
            from: utc(2026, 5, 15, 0, 0),
            until: NotamEnd::Permanent,
        };
        assert!(validity.active_at(utc(2099, 1, 1, 0, 0)));
        assert!(validity.overlaps(utc(2030, 1, 1, 0, 0), utc(2030, 1, 2, 0, 0)));
        assert!(!validity.overlaps(utc(2026, 5, 1, 0, 0), utc(2026, 5, 15, 0, 0)));
    }

    #[test]
    fn estimated_end_bounds_the_window() {
        let validity = NotamValidity {
            from: utc(2026, 6, 12, 0, 0),
            until: NotamEnd::Estimated(utc(2026, 9, 30, 12, 0)),
        };
        assert_eq!(validity.end(), Some(utc(2026, 9, 30, 12, 0)));
        assert!(validity.overlaps(utc(2026, 9, 1, 0, 0), utc(2026, 10, 1, 0, 0)));
        assert!(!validity.overlaps(utc(2026, 10, 1, 0, 0), utc(2026, 11, 1, 0, 0)));
    }

    #[test]
    fn overlap_edges_are_half_open() {
        let validity = NotamValidity {
            from: utc(2026, 6, 15, 6, 0),
            until: NotamEnd::At(utc(2026, 6, 17, 18, 0)),
        };
        // Window ending exactly at `from` does not overlap.
        assert!(!validity.overlaps(utc(2026, 6, 14, 0, 0), utc(2026, 6, 15, 6, 0)));
        // Window starting exactly at the end does not overlap.
        assert!(!validity.overlaps(utc(2026, 6, 17, 18, 0), utc(2026, 6, 18, 0, 0)));
        // One minute of intersection counts.
        assert!(validity.overlaps(utc(2026, 6, 17, 17, 59), utc(2026, 6, 18, 0, 0)));
    }

    #[test]
    fn compact_round_trip() {
        let t = utc(2026, 6, 15, 6, 0);
        assert_eq!(format_compact_datetime(t), "2606150600");
        assert_eq!(
            parse_compact_datetime(&format_compact_datetime(t)).expect("round-trips"),
            t
        );
    }
}

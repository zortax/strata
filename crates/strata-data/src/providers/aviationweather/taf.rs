//! `/api/data/taf?format=json` response mapping.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::{IcaoCode, Taf, TafGroup};

/// Raw record shape of the `taf` endpoint. The API's own `fcsts` decode is
/// ignored — forecast groups come from decoding the raw text in
/// [`crate::decode`]; station, issue time, and validity window stay
/// API-authoritative.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TafRecord {
    icao_id: String,
    /// RFC 3339 issue time.
    issue_time: Option<String>,
    /// RFC 3339 bulletin time, fallback for a missing issue time.
    bulletin_time: Option<String>,
    /// Validity window, Unix epoch seconds.
    valid_time_from: i64,
    valid_time_to: i64,
    #[serde(rename = "rawTAF")]
    raw_taf: String,
}

/// Maps an endpoint body to domain TAFs with empty forecast groups (see
/// [`with_decoded`]). Malformed individual records are skipped with a
/// warning; only an unparseable body errors.
pub(super) fn parse_response(body: &str) -> Result<Vec<Taf>, serde_json::Error> {
    let values: Vec<serde_json::Value> = serde_json::from_str(body)?;
    Ok(values.into_iter().filter_map(record_to_domain).collect())
}

fn record_to_domain(value: serde_json::Value) -> Option<Taf> {
    let record: TafRecord = match serde_json::from_value(value) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(%error, "skipping malformed TAF record");
            return None;
        }
    };
    let station = match IcaoCode::new(&record.icao_id) {
        Ok(station) => station,
        Err(error) => {
            tracing::warn!(%error, "skipping TAF with invalid station id");
            return None;
        }
    };
    let (Some(valid_from), Some(valid_to)) = (
        DateTime::from_timestamp(record.valid_time_from, 0),
        DateTime::from_timestamp(record.valid_time_to, 0),
    ) else {
        tracing::warn!(station = %station, "skipping TAF with invalid validity window");
        return None;
    };
    let issued_at = parse_rfc3339(record.issue_time.as_deref())
        .or_else(|| parse_rfc3339(record.bulletin_time.as_deref()))
        .unwrap_or(valid_from);
    Some(Taf {
        raw: record.raw_taf,
        station,
        issued_at,
        valid_from,
        valid_to,
        base: empty_group(),
        changes: Vec::new(),
    })
}

fn parse_rfc3339(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn empty_group() -> TafGroup {
    TafGroup {
        wind: None,
        visibility: None,
        weather: Vec::new(),
        clouds: Vec::new(),
    }
}

/// Fills the forecast groups by decoding the raw text (the issue time
/// anchors day/hour tokens). A failed decode keeps the raw-only TAF.
pub(super) fn with_decoded(mut taf: Taf) -> Taf {
    match crate::decode::decode_taf(&taf.raw, taf.issued_at) {
        Ok(decoded) => {
            // API metadata (station, issue, validity) stays authoritative;
            // the text decode supplies the forecast groups.
            taf.base = decoded.base;
            taf.changes = decoded.changes;
        }
        Err(error) => {
            tracing::warn!(
                station = %taf.station,
                %error,
                "TAF decode failed; keeping raw text only"
            );
        }
    }
    taf
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/aviationweather/tafs-de.json");

    #[test]
    fn all_fixture_records_map_to_domain() {
        let tafs = parse_response(FIXTURE).expect("fixture parses");
        assert_eq!(tafs.len(), 8);
        for taf in &tafs {
            assert!(taf.raw.starts_with("TAF "), "raw set: {:?}", taf.raw);
            assert!(
                taf.raw.contains(taf.station.as_str()),
                "station {} taken from its own report",
                taf.station
            );
            assert!(taf.valid_from < taf.valid_to, "validity window ordered");
            assert!(taf.base.wind.is_none(), "mapping alone never decodes");
            assert!(taf.changes.is_empty(), "mapping alone never decodes");
        }
    }

    #[test]
    fn first_fixture_record_fields() {
        let tafs = parse_response(FIXTURE).expect("fixture parses");
        let first = &tafs[0];
        assert_eq!(first.station.as_str(), "EDDB");
        assert_eq!(
            first.issued_at,
            DateTime::parse_from_rfc3339("2026-06-09T23:00:00Z")
                .expect("valid rfc3339")
                .with_timezone(&Utc)
        );
        assert_eq!(
            first.valid_from,
            DateTime::from_timestamp(1_781_049_600, 0).expect("valid epoch")
        );
        assert_eq!(
            first.valid_to,
            DateTime::from_timestamp(1_781_136_000, 0).expect("valid epoch")
        );
        assert_eq!(
            first.raw,
            "TAF EDDB 092300Z 1000/1024 22005KT CAVOK TEMPO 1011/1019 SHRA SCT045CB \
             PROB30 TEMPO 1012/1017 TSRA"
        );
    }

    #[test]
    fn missing_issue_time_falls_back_to_validity_start() {
        let body = r#"[{
            "icaoId": "EDDF",
            "validTimeFrom": 1781049600,
            "validTimeTo": 1781136000,
            "rawTAF": "TAF EDDF 092300Z 1000/1024 22005KT CAVOK"
        }]"#;
        let tafs = parse_response(body).expect("body parses");
        assert_eq!(tafs.len(), 1);
        assert_eq!(
            tafs[0].issued_at,
            DateTime::from_timestamp(1_781_049_600, 0).expect("valid epoch")
        );
    }

    #[test]
    fn malformed_records_are_skipped_not_fatal() {
        let body = r#"[
            {"icaoId": "X", "validTimeFrom": 1, "validTimeTo": 2, "rawTAF": "TAF X"},
            {"icaoId": "EDDF", "validTimeFrom": 1781049600, "validTimeTo": 1781136000,
             "rawTAF": "TAF EDDF 092300Z 1000/1024 22005KT CAVOK"}
        ]"#;
        let tafs = parse_response(body).expect("body parses");
        assert_eq!(tafs.len(), 1);
        assert_eq!(tafs[0].station.as_str(), "EDDF");
    }
}

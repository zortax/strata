//! `/api/data/metar?format=json` response mapping.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::{IcaoCode, Metar};

/// Raw record shape of the `metar` endpoint. Unknown fields are ignored; the
/// pre-decoded fields the API ships alongside are not trusted — decoding
/// happens from the raw text in [`crate::decode`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetarRecord {
    icao_id: String,
    /// Observation time, Unix epoch seconds.
    obs_time: Option<i64>,
    /// RFC 3339 fallback when `obsTime` is absent.
    report_time: Option<String>,
    raw_ob: String,
}

/// Maps an endpoint body to domain reports with `decoded: None` (see
/// [`with_decoded`]). Malformed individual records are skipped with a
/// warning; only an unparseable body errors.
pub(super) fn parse_response(body: &str) -> Result<Vec<Metar>, serde_json::Error> {
    let values: Vec<serde_json::Value> = serde_json::from_str(body)?;
    Ok(values.into_iter().filter_map(record_to_domain).collect())
}

fn record_to_domain(value: serde_json::Value) -> Option<Metar> {
    let record: MetarRecord = match serde_json::from_value(value) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(%error, "skipping malformed METAR record");
            return None;
        }
    };
    let station = match IcaoCode::new(&record.icao_id) {
        Ok(station) => station,
        Err(error) => {
            tracing::warn!(%error, "skipping METAR with invalid station id");
            return None;
        }
    };
    let Some(observed_at) = observation_time(&record) else {
        tracing::warn!(station = %station, "skipping METAR without a usable observation time");
        return None;
    };
    Some(Metar {
        raw: record.raw_ob,
        station,
        observed_at,
        decoded: None,
    })
}

fn observation_time(record: &MetarRecord) -> Option<DateTime<Utc>> {
    if let Some(time) = record
        .obs_time
        .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
    {
        return Some(time);
    }
    let report_time = record.report_time.as_deref()?;
    DateTime::parse_from_rfc3339(report_time)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// Attaches the decoded body; a failed decode keeps the raw-only report.
pub(super) fn with_decoded(mut metar: Metar) -> Metar {
    match crate::decode::decode_metar(&metar.raw) {
        Ok(decoded) => metar.decoded = Some(decoded),
        Err(error) => {
            tracing::warn!(
                station = %metar.station,
                %error,
                "METAR decode failed; keeping raw text only"
            );
        }
    }
    metar
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/aviationweather/metars-de.json");

    #[test]
    fn all_fixture_records_map_to_domain() {
        let metars = parse_response(FIXTURE).expect("fixture parses");
        assert_eq!(metars.len(), 63);
        for metar in &metars {
            assert!(metar.raw.starts_with("METAR "), "raw set: {:?}", metar.raw);
            assert!(
                metar.raw.contains(metar.station.as_str()),
                "station {} taken from its own report",
                metar.station
            );
            assert!(metar.observed_at.timestamp() > 0, "observation time set");
            assert!(metar.decoded.is_none(), "mapping alone never decodes");
        }
    }

    #[test]
    fn first_fixture_record_fields() {
        let metars = parse_response(FIXTURE).expect("fixture parses");
        let first = &metars[0];
        assert_eq!(first.station.as_str(), "EHGG");
        assert_eq!(
            first.observed_at,
            DateTime::from_timestamp(1_781_047_500, 0).expect("valid epoch")
        );
        assert_eq!(
            first.raw,
            "METAR EHGG 092325Z AUTO 21008KT 180V240 9999 NCD 10/09 Q1013"
        );
    }

    #[test]
    fn report_time_is_the_observation_time_fallback() {
        let body = r#"[{
            "icaoId": "EDDF",
            "reportTime": "2026-06-09T23:20:00.000Z",
            "rawOb": "METAR EDDF 092320Z 25004KT CAVOK 14/10 Q1014 NOSIG"
        }]"#;
        let metars = parse_response(body).expect("body parses");
        assert_eq!(metars.len(), 1);
        assert_eq!(
            metars[0].observed_at,
            DateTime::parse_from_rfc3339("2026-06-09T23:20:00Z")
                .expect("valid rfc3339")
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn malformed_records_are_skipped_not_fatal() {
        let body = r#"[
            {"icaoId": "EDDF", "obsTime": 1781047500,
             "rawOb": "METAR EDDF 092325Z 25004KT CAVOK 14/10 Q1014"},
            {"icaoId": "TOO_LONG_FOR_ICAO", "obsTime": 1781047500, "rawOb": "garbage"},
            {"icaoId": "EDDM"},
            {"icaoId": "EDDH", "rawOb": "METAR EDDH ..."}
        ]"#;
        let metars = parse_response(body).expect("body parses");
        assert_eq!(metars.len(), 1);
        assert_eq!(metars[0].station.as_str(), "EDDF");
    }

    #[test]
    fn unparseable_body_is_an_error() {
        assert!(parse_response("<html>503</html>").is_err());
    }
}

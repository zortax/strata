//! `GET /v1.0/notam` response rows → domain [`Notam`].
//!
//! The API returns structured fields instead of the transmitted message.
//! Rather than maintain a second decode path, each row is rendered back to
//! canonical ICAO transmission format and run through [`Notam::parse`] —
//! one decoder, and the stored `raw` is the reconstructed message. Rows
//! that fail to render or parse are skipped with a warning (same
//! record-by-record policy as the aviationweather provider).

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::domain::notam::{format_centre_radius, format_compact_datetime};
use crate::domain::{LatLon, Notam};

/// One page of the NOTAM endpoint.
#[derive(Debug, Deserialize)]
pub(super) struct NotamResponse {
    pub total: u64,
    pub rows: Vec<NotamRow>,
}

/// A NOTAM row as documented at <https://www.autorouter.aero/wiki/api/notams/>.
/// Unknown fields are ignored; everything not strictly required is
/// optional so a sparse row degrades to a skip, not a failed page.
#[derive(Debug, Deserialize)]
pub(super) struct NotamRow {
    pub series: String,
    pub number: u16,
    pub year: u8,
    /// `N` / `R` / `C`.
    #[serde(rename = "type")]
    pub kind: String,
    pub referredseries: Option<String>,
    pub referrednumber: Option<u16>,
    pub referredyear: Option<u8>,
    pub fir: String,
    /// Q-code letters 2–3.
    pub code23: Option<String>,
    /// Q-code letters 4–5.
    pub code45: Option<String>,
    pub traffic: Option<String>,
    pub purpose: Option<String>,
    pub scope: Option<String>,
    /// Lower/upper limits in flight levels (Q-line convention).
    pub lower: Option<u16>,
    pub upper: Option<u16>,
    /// Garmin 32-bit angular format: degrees = raw * 90 / 2^30
    /// (documented as "Garmin format: * 90 / (1 << 30) to convert").
    pub lat: Option<i64>,
    pub lon: Option<i64>,
    /// Radius in NM.
    pub radius: Option<u32>,
    pub itema: Vec<String>,
    /// Validity start/end in seconds since the Unix epoch.
    pub startvalidity: i64,
    pub endvalidity: Option<i64>,
    /// Whether the end of validity is estimated (`EST`). Sent as `null`
    /// when not estimated; deserialized loosely since the wiki does not
    /// document the populated type.
    #[serde(default)]
    pub estimation: Option<Value>,
    pub itemd: Option<String>,
    pub iteme: Option<String>,
    pub itemf: Option<String>,
    pub itemg: Option<String>,
}

/// `endvalidity` value the API documents as the query default upper bound
/// (`2^32 - 1`); rows carrying it have no real end — `PERM`.
const PERMANENT_SENTINEL: i64 = u32::MAX as i64;

/// Maps a page of rows, skipping malformed records with a warning.
pub(super) fn normalize(rows: Vec<NotamRow>) -> Vec<Notam> {
    rows.into_iter()
        .filter_map(|row| {
            let label = format!("{}{:04}/{:02}", row.series, row.number, row.year);
            match notam_from_row(row) {
                Ok(notam) => Some(notam),
                Err(reason) => {
                    warn!(notam = %label, %reason, "skipping malformed autorouter NOTAM row");
                    None
                }
            }
        })
        .collect()
}

fn notam_from_row(row: NotamRow) -> Result<Notam, String> {
    let text = render_canonical(&row)?;
    Notam::parse(&text).map_err(|e| e.to_string())
}

/// Renders the structured row back to ICAO transmission format.
fn render_canonical(row: &NotamRow) -> Result<String, String> {
    let mut header = format!(
        "{}{:04}/{:02} NOTAM{}",
        row.series, row.number, row.year, row.kind
    );
    if row.kind == "R" || row.kind == "C" {
        let (series, number, year) =
            match (&row.referredseries, row.referrednumber, row.referredyear) {
                (Some(series), Some(number), Some(year)) => (series, number, year),
                _ => return Err("missing referred NOTAM id".to_owned()),
            };
        header.push_str(&format!(" {series}{number:04}/{year:02}"));
    }

    let centre = match (row.lat, row.lon) {
        (Some(lat), Some(lon)) => LatLon::new(garmin_to_degrees(lat), garmin_to_degrees(lon))
            .map_err(|e| format!("centre out of range: {e}"))?,
        _ => return Err("missing centre coordinates".to_owned()),
    };
    let q_line = format!(
        "{}/Q{}{}/{}/{}/{}/{:03}/{:03}/{}",
        row.fir,
        row.code23.as_deref().unwrap_or("XX"),
        row.code45.as_deref().unwrap_or("XX"),
        row.traffic.as_deref().unwrap_or(""),
        row.purpose.as_deref().unwrap_or(""),
        row.scope.as_deref().unwrap_or(""),
        row.lower.unwrap_or(0).min(999),
        row.upper.unwrap_or(999).min(999),
        format_centre_radius(centre, row.radius.unwrap_or(0)),
    );

    let item_a = if row.itema.is_empty() {
        return Err("empty item A".to_owned());
    } else {
        row.itema.join(" ")
    };
    let item_b = format_compact_datetime(epoch(row.startvalidity)?);

    let mut text = format!("{header}\nQ) {q_line}\nA) {item_a} B) {item_b}");
    // A NOTAMC carries no item C.
    if row.kind != "C" {
        let item_c = match row.endvalidity {
            None => "PERM".to_owned(),
            Some(end) if end >= PERMANENT_SENTINEL => "PERM".to_owned(),
            Some(end) if is_estimated(&row.estimation) => {
                format!("{}EST", format_compact_datetime(epoch(end)?))
            }
            Some(end) => format_compact_datetime(epoch(end)?),
        };
        text.push_str(&format!(" C) {item_c}"));
    }
    if let Some(d) = non_empty(&row.itemd) {
        text.push_str(&format!("\nD) {d}"));
    }
    let item_e = non_empty(&row.iteme).ok_or_else(|| "missing item E".to_owned())?;
    text.push_str(&format!("\nE) {item_e}"));
    if let Some(f) = non_empty(&row.itemf) {
        text.push_str(&format!("\nF) {f}"));
    }
    if let Some(g) = non_empty(&row.itemg) {
        text.push_str(&format!("\nG) {g}"));
    }
    Ok(text)
}

/// Garmin 32-bit angular units → degrees (`raw * 90 / 2^30`).
pub(super) fn garmin_to_degrees(raw: i64) -> f64 {
    raw as f64 * 90.0 / f64::from(1u32 << 30)
}

fn epoch(seconds: i64) -> Result<DateTime<Utc>, String> {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .ok_or_else(|| format!("epoch {seconds} out of range"))
}

/// Truthy check for the loosely-typed `estimation` field: anything other
/// than `null`, `false` or `0` marks the end time as estimated.
fn is_estimated(estimation: &Option<Value>) -> bool {
    match estimation {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        Some(_) => true,
    }
}

fn non_empty(field: &Option<String>) -> Option<&str> {
    field.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::notam::{NotamEnd, NotamKind, QCondition, QSubject};

    /// The documented example row from the API wiki (EDDS, NOTAMR).
    const WIKI_EXAMPLE: &str = r#"{
        "code23": "FA",
        "code45": "LT",
        "endvalidity": 1493596740,
        "estimation": null,
        "fir": "EDGG",
        "id": 1499589,
        "itema": ["EDDS"],
        "itemd": null,
        "iteme": "MIL SIDE NO TRANSIENT ALERT AVBL DLY 2000-0400, EXCEPT SELECTED FLIGHTS.",
        "itemf": null,
        "itemg": null,
        "lat": 580814790,
        "lon": 109959116,
        "lower": 0,
        "modified": 1485763803,
        "nelat": 581807214,
        "nelon": 111462287,
        "nof": "ETCZ",
        "number": 825,
        "purpose": "NBO",
        "radius": 5,
        "referrednumber": 7633,
        "referredseries": "P",
        "referredyear": 16,
        "scope": "A",
        "series": "P",
        "startvalidity": 1485763560,
        "suppressed": false,
        "swlat": 579822366,
        "swlon": 108455945,
        "traffic": "IV",
        "type": "R",
        "upper": 999,
        "year": 17
    }"#;

    fn wiki_row() -> NotamRow {
        serde_json::from_str(WIKI_EXAMPLE).expect("documented example deserializes")
    }

    #[test]
    fn garmin_conversion_matches_documented_example() {
        // EDDS (Stuttgart) lies at ~48.69N 9.22E.
        let lat = garmin_to_degrees(580814790);
        let lon = garmin_to_degrees(109959116);
        assert!((lat - 48.68).abs() < 0.05, "lat {lat}");
        assert!((lon - 9.22).abs() < 0.05, "lon {lon}");
        assert_eq!(garmin_to_degrees(0), 0.0);
        assert!((garmin_to_degrees(-(1 << 30)) - -90.0).abs() < 1e-12);
    }

    #[test]
    fn wiki_example_row_maps_to_a_domain_notam() {
        let notams = normalize(vec![wiki_row()]);
        assert_eq!(notams.len(), 1);
        let notam = &notams[0];

        assert_eq!(notam.id.to_string(), "P0825/17");
        let NotamKind::Replacement { replaces } = notam.kind else {
            panic!("expected NOTAMR, got {:?}", notam.kind);
        };
        assert_eq!(replaces.to_string(), "P7633/16");

        assert_eq!(notam.fir().as_str(), "EDGG");
        assert_eq!(notam.q.code.subject, QSubject::Aerodrome);
        assert_eq!(notam.q.code.condition, QCondition::LimitedTo);
        assert!(notam.q.traffic.ifr && notam.q.traffic.vfr);
        assert!(notam.q.scope.aerodrome);
        assert_eq!(notam.q.radius_nm, 5);
        assert!((notam.q.centre.lat() - 48.68).abs() < 0.05);

        assert_eq!(notam.locations.len(), 1);
        assert_eq!(notam.locations[0].as_str(), "EDDS");
        assert_eq!(
            notam.validity.from,
            DateTime::<Utc>::from_timestamp(1485763560, 0).expect("valid epoch")
        );
        // Epochs are reconstructed at minute resolution (B/C format);
        // the example epoch is already a whole minute.
        let NotamEnd::At(end) = notam.validity.until else {
            panic!("expected definite end, got {:?}", notam.validity.until);
        };
        assert_eq!(
            end,
            DateTime::<Utc>::from_timestamp(1493596740, 0).expect("valid epoch")
        );
        assert!(notam.text.starts_with("MIL SIDE NO TRANSIENT ALERT"));
        assert!(notam.raw.contains("Q) EDGG/QFALT/IV/NBO/A/000/999/"));
    }

    #[test]
    fn permanent_sentinel_and_missing_end_map_to_perm() {
        let mut row = wiki_row();
        row.kind = "N".to_owned();
        row.endvalidity = Some(PERMANENT_SENTINEL);
        let notams = normalize(vec![row]);
        assert_eq!(notams[0].validity.until, NotamEnd::Permanent);
        assert!(notams[0].raw.contains("C) PERM"));

        let mut row = wiki_row();
        row.kind = "N".to_owned();
        row.endvalidity = None;
        let notams = normalize(vec![row]);
        assert_eq!(notams[0].validity.until, NotamEnd::Permanent);
    }

    #[test]
    fn estimation_flag_maps_to_estimated_end() {
        let mut row = wiki_row();
        row.kind = "N".to_owned();
        row.estimation = Some(Value::Bool(true));
        let notams = normalize(vec![row]);
        assert!(matches!(notams[0].validity.until, NotamEnd::Estimated(_)));
        assert!(notams[0].raw.contains("EST"));

        // Explicit false / 0 are not estimated.
        for value in [Value::Bool(false), Value::from(0)] {
            let mut row = wiki_row();
            row.kind = "N".to_owned();
            row.estimation = Some(value);
            let notams = normalize(vec![row]);
            assert!(matches!(notams[0].validity.until, NotamEnd::At(_)));
        }
    }

    #[test]
    fn cancellation_renders_without_item_c() {
        let mut row = wiki_row();
        row.kind = "C".to_owned();
        let notams = normalize(vec![row]);
        assert_eq!(notams.len(), 1);
        assert!(matches!(notams[0].kind, NotamKind::Cancellation { .. }));
        assert_eq!(notams[0].validity.until, NotamEnd::Permanent);
        assert!(!notams[0].raw.contains("C) "));
    }

    #[test]
    fn malformed_rows_are_skipped_not_fatal() {
        // Missing item E.
        let mut no_text = wiki_row();
        no_text.iteme = None;
        // Missing centre.
        let mut no_centre = wiki_row();
        no_centre.lat = None;
        // Replacement without a reference.
        let mut no_ref = wiki_row();
        no_ref.referredseries = None;
        // One good row keeps flowing.
        let good = wiki_row();

        let notams = normalize(vec![no_text, no_centre, no_ref, good]);
        assert_eq!(notams.len(), 1);
        assert_eq!(notams[0].id.to_string(), "P0825/17");
    }

    #[test]
    fn schedule_and_limits_pass_through() {
        let mut row = wiki_row();
        row.kind = "N".to_owned();
        row.itemd = Some("DLY 0700-1500".to_owned());
        row.itemf = Some("GND".to_owned());
        row.itemg = Some("FL100".to_owned());
        let notams = normalize(vec![row]);
        let notam = &notams[0];
        assert_eq!(notam.schedule.as_deref(), Some("DLY 0700-1500"));
        assert_eq!(notam.items.f.as_deref(), Some("GND"));
        assert_eq!(notam.items.g.as_deref(), Some("FL100"));
    }

    #[test]
    fn response_envelope_deserializes() {
        let body = format!("{{\"total\": 18, \"rows\": [{WIKI_EXAMPLE}]}}");
        let response: NotamResponse = serde_json::from_str(&body).expect("deserializes");
        assert_eq!(response.total, 18);
        assert_eq!(response.rows.len(), 1);
    }
}

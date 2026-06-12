//! openAIP `/reporting-points` payload → [`ReportingPoint`].
//!
//! Reporting points reference their airports by openAIP mongo `_id`; the
//! caller supplies an id → ICAO index (built from `/airports`, see
//! [`super::airports::icao_index`]). Unresolvable references drop with a
//! warning — the point itself is kept.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::domain::{IcaoCode, ReportingPoint};

use super::NormalizationReport;
use super::common::{item_id, point_position};

#[derive(Debug, Deserialize)]
struct RawReportingPoint {
    name: String,
    #[serde(default)]
    compulsory: bool,
    #[serde(default)]
    airports: Vec<String>,
    geometry: Value,
}

pub(crate) fn normalize(
    items: &[Value],
    airport_icao_by_id: &HashMap<String, IcaoCode>,
) -> (Vec<ReportingPoint>, NormalizationReport) {
    let mut points = Vec::with_capacity(items.len());
    let mut report = NormalizationReport::new(items.len());
    for item in items {
        match normalize_one(item, airport_icao_by_id) {
            Ok(point) => points.push(point),
            Err(reason) => report.skip(item_id(item), reason, "reporting point"),
        }
    }
    (points, report)
}

fn normalize_one(
    item: &Value,
    airport_icao_by_id: &HashMap<String, IcaoCode>,
) -> Result<ReportingPoint, String> {
    let raw: RawReportingPoint = serde_json::from_value(item.clone())
        .map_err(|e| format!("malformed reporting point: {e}"))?;
    let position = point_position(&raw.geometry)?;
    let airports = raw
        .airports
        .iter()
        .filter_map(|id| match airport_icao_by_id.get(id) {
            Some(icao) => Some(icao.clone()),
            None => {
                warn!(
                    point = %raw.name,
                    airport_id = %id,
                    "dropping unresolvable airport reference"
                );
                None
            }
        })
        .collect();
    Ok(ReportingPoint {
        name: raw.name,
        mandatory: raw.compulsory,
        position,
        airports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_items() -> Vec<Value> {
        super::super::fixture_items(include_str!(
            "../../../tests/fixtures/openaip/reporting-points.json"
        ))
    }

    #[test]
    fn every_fixture_point_normalizes() {
        let items = fixture_items();
        let (points, report) = normalize(&items, &HashMap::new());
        assert_eq!(report.total, 30);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(points.len(), 30);
        // With an empty index every airport reference is unresolvable and
        // drops, but the points themselves survive.
        assert!(points.iter().all(|p| p.airports.is_empty()));
    }

    #[test]
    fn alpha_spot_check() {
        let (points, _) = normalize(&fixture_items(), &HashMap::new());
        let alpha = &points[0];
        assert_eq!(alpha.name, "ALPHA");
        assert!(!alpha.mandatory);
        assert!((alpha.position.lat() - 49.326_566_666_666_665).abs() < 1e-12);
        assert!((alpha.position.lon() - 10.668_966_666_666_666).abs() < 1e-12);
    }

    #[test]
    fn compulsory_maps_to_mandatory() {
        let (points, _) = normalize(&fixture_items(), &HashMap::new());
        let bravo = points
            .iter()
            .find(|p| p.mandatory)
            .expect("fixture has compulsory points");
        assert_eq!(bravo.name, "BRAVO");
    }

    #[test]
    fn airport_references_resolve_through_the_index() {
        let icao = IcaoCode::new("EDQD").expect("valid code");
        // ALPHA references this airport id in the fixture.
        let index = HashMap::from([("62614a39cb27f42509443f31".to_owned(), icao.clone())]);
        let (points, _) = normalize(&fixture_items(), &index);
        assert_eq!(points[0].airports, vec![icao]);
    }
}

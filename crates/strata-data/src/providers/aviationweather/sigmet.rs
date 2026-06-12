//! `/api/data/isigmet?format=json` response mapping.
//!
//! The endpoint returns SIGMETs worldwide; only those whose geometry
//! intersects the requested region's bounding box are kept.

use chrono::DateTime;
use serde::Deserialize;

use crate::domain::{BoundingBox, LatLon, Polygon, Sigmet, SigmetHazard};

/// Raw record shape of the `isigmet` endpoint. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IsigmetRecord {
    fir_id: String,
    hazard: String,
    /// Validity window, Unix epoch seconds.
    valid_time_from: i64,
    valid_time_to: i64,
    #[serde(default)]
    coords: Option<Coords>,
    raw_sigmet: String,
}

/// `geom: "AREA"` records carry one coordinate ring, `geom: "AREAS"` a list
/// of rings (one SIGMET covering several disjoint areas).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Coords {
    Rings(Vec<Vec<Coord>>),
    Ring(Vec<Coord>),
}

#[derive(Debug, Deserialize)]
struct Coord {
    lat: f64,
    lon: f64,
}

/// Maps an endpoint body to domain SIGMETs intersecting `bbox`, one SIGMET
/// per intersecting area. Malformed individual records are skipped with a
/// warning; only an unparseable body errors.
pub(super) fn parse_response(
    body: &str,
    bbox: &BoundingBox,
) -> Result<Vec<Sigmet>, serde_json::Error> {
    let values: Vec<serde_json::Value> = serde_json::from_str(body)?;
    Ok(values
        .into_iter()
        .flat_map(|value| record_to_domain(value, bbox))
        .collect())
}

fn record_to_domain(value: serde_json::Value, bbox: &BoundingBox) -> Vec<Sigmet> {
    let record: IsigmetRecord = match serde_json::from_value(value) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(%error, "skipping malformed ISIGMET record");
            return Vec::new();
        }
    };
    let (Some(valid_from), Some(valid_to)) = (
        DateTime::from_timestamp(record.valid_time_from, 0),
        DateTime::from_timestamp(record.valid_time_to, 0),
    ) else {
        tracing::warn!(fir = %record.fir_id, "skipping SIGMET with invalid validity window");
        return Vec::new();
    };
    let hazard = hazard_from_code(&record.hazard);
    let rings = match record.coords {
        Some(Coords::Rings(rings)) => rings,
        Some(Coords::Ring(ring)) => vec![ring],
        None => Vec::new(),
    };
    rings
        .into_iter()
        .filter_map(|ring| ring_to_polygon(ring, &record.fir_id))
        .filter(|polygon| polygon.bounding_box().intersects(bbox))
        .map(|geometry| Sigmet {
            fir: record.fir_id.clone(),
            hazard: hazard.clone(),
            geometry,
            valid_from,
            valid_to,
            raw: record.raw_sigmet.clone(),
        })
        .collect()
}

fn ring_to_polygon(ring: Vec<Coord>, fir: &str) -> Option<Polygon> {
    let points: Result<Vec<LatLon>, _> = ring
        .into_iter()
        .map(|coord| LatLon::new(coord.lat, coord.lon))
        .collect();
    let points = match points {
        Ok(points) => points,
        Err(error) => {
            // SIGMETs spanning the antimeridian use longitudes beyond ±180°;
            // irrelevant for a Europe-focused view, so skip quietly.
            tracing::debug!(fir, %error, "skipping SIGMET ring with out-of-range coordinates");
            return None;
        }
    };
    match Polygon::new(points, Vec::new()) {
        Ok(polygon) => Some(polygon),
        Err(error) => {
            tracing::debug!(fir, %error, "skipping degenerate SIGMET ring");
            None
        }
    }
}

/// AWC hazard codes; anything unrecognized is preserved verbatim.
fn hazard_from_code(code: &str) -> SigmetHazard {
    match code.trim() {
        "TS" => SigmetHazard::Thunderstorm,
        "TURB" => SigmetHazard::Turbulence,
        "ICE" => SigmetHazard::Icing,
        "MTW" => SigmetHazard::MountainWave,
        "VA" => SigmetHazard::VolcanicAsh,
        "TC" => SigmetHazard::TropicalCyclone,
        "DS" => SigmetHazard::DustStorm,
        "SS" => SigmetHazard::Sandstorm,
        "RDOACT" | "RDOACT CLD" => SigmetHazard::RadioactiveCloud,
        other => SigmetHazard::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Country;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/aviationweather/isigmets.json");

    fn world() -> BoundingBox {
        BoundingBox::new(-180.0, -90.0, 180.0, 90.0).expect("valid bbox")
    }

    #[test]
    fn germany_bbox_filter_keeps_only_intersecting_sigmets() {
        let bbox = Country::DE.bounding_box();
        let sigmets = parse_response(FIXTURE, &bbox).expect("fixture parses");
        assert_eq!(sigmets.len(), 1);
        let hit = &sigmets[0];
        assert_eq!(hit.fir, "LIMM");
        assert_eq!(hit.hazard, SigmetHazard::Thunderstorm);
        assert!(hit.geometry.bounding_box().intersects(&bbox));
        assert_eq!(
            hit.valid_from,
            DateTime::from_timestamp(1_781_046_000, 0).expect("valid epoch")
        );
        assert_eq!(
            hit.valid_to,
            DateTime::from_timestamp(1_781_060_400, 0).expect("valid epoch")
        );
        assert!(hit.raw.starts_with("WSIY31 LIMM"));
    }

    #[test]
    fn world_bbox_maps_every_valid_ring() {
        // 116 fixture records: 115 single-area + one 2-area record, minus
        // one ring with longitudes beyond 180° = 116 mappable polygons.
        let sigmets = parse_response(FIXTURE, &world()).expect("fixture parses");
        assert_eq!(sigmets.len(), 116);
    }

    #[test]
    fn fixture_hazard_codes_all_map_to_dedicated_variants() {
        let sigmets = parse_response(FIXTURE, &world()).expect("fixture parses");
        for hazard in [
            SigmetHazard::Thunderstorm,
            SigmetHazard::Turbulence,
            SigmetHazard::Icing,
            SigmetHazard::MountainWave,
            SigmetHazard::VolcanicAsh,
            SigmetHazard::TropicalCyclone,
        ] {
            assert!(
                sigmets.iter().any(|s| s.hazard == hazard),
                "fixture contains {hazard:?}"
            );
        }
        assert!(
            !sigmets
                .iter()
                .any(|s| matches!(s.hazard, SigmetHazard::Other(_))),
            "every fixture hazard code has a dedicated variant"
        );
    }

    #[test]
    fn hazard_code_mapping() {
        assert_eq!(hazard_from_code("TS"), SigmetHazard::Thunderstorm);
        assert_eq!(hazard_from_code("TURB"), SigmetHazard::Turbulence);
        assert_eq!(hazard_from_code("ICE"), SigmetHazard::Icing);
        assert_eq!(hazard_from_code("MTW"), SigmetHazard::MountainWave);
        assert_eq!(hazard_from_code("VA"), SigmetHazard::VolcanicAsh);
        assert_eq!(hazard_from_code("TC"), SigmetHazard::TropicalCyclone);
        assert_eq!(hazard_from_code("DS"), SigmetHazard::DustStorm);
        assert_eq!(hazard_from_code("SS"), SigmetHazard::Sandstorm);
        assert_eq!(hazard_from_code("RDOACT"), SigmetHazard::RadioactiveCloud);
        assert_eq!(
            hazard_from_code("WIND"),
            SigmetHazard::Other("WIND".to_owned())
        );
    }

    #[test]
    fn multi_area_record_yields_one_sigmet_per_area() {
        let body = r#"[{
            "firId": "EDGG", "hazard": "TS",
            "validTimeFrom": 1781046000, "validTimeTo": 1781060400,
            "geom": "AREAS",
            "coords": [
                [{"lat": 48.0, "lon": 8.0}, {"lat": 49.0, "lon": 8.0}, {"lat": 49.0, "lon": 9.0}],
                [{"lat": 50.0, "lon": 10.0}, {"lat": 51.0, "lon": 10.0}, {"lat": 51.0, "lon": 11.0}]
            ],
            "rawSigmet": "WSDL31 EDGG ..."
        }]"#;
        let sigmets =
            parse_response(body, &Country::DE.bounding_box()).expect("body parses");
        assert_eq!(sigmets.len(), 2);
        assert!(sigmets.iter().all(|s| s.fir == "EDGG"));
    }

    #[test]
    fn degenerate_rings_are_skipped() {
        // Two distinct points (closed ring) — not a polygon.
        let body = r#"[{
            "firId": "EDGG", "hazard": "TS",
            "validTimeFrom": 1781046000, "validTimeTo": 1781060400,
            "geom": "AREA",
            "coords": [{"lat": 48.0, "lon": 8.0}, {"lat": 49.0, "lon": 9.0},
                       {"lat": 48.0, "lon": 8.0}],
            "rawSigmet": "WSDL31 EDGG ..."
        }]"#;
        let sigmets =
            parse_response(body, &Country::DE.bounding_box()).expect("body parses");
        assert!(sigmets.is_empty());
    }
}

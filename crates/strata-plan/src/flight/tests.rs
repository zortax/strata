use chrono::{TimeZone as _, Utc};
use serde_json::json;
use strata_data::domain::{LatLon, MetersAmsl};

use super::*;
use crate::units::{DegreesTrue, Kilograms, Knots, Liters, Minutes};
use crate::versioned::VersionError;

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

fn named(kind: NamedPointKind, id: &str, name: &str, lat: f64, lon: f64) -> RoutePoint {
    RoutePoint::Named(NamedPoint {
        kind,
        id: id.to_owned(),
        name: name.to_owned(),
        position: ll(lat, lon),
    })
}

/// A fully-populated document exercising every field.
fn full_doc() -> FlightDoc {
    let mut departure = RouteWaypoint::new(named(
        NamedPointKind::Airport,
        "EDFE",
        "Frankfurt-Egelsbach",
        49.96,
        8.64,
    ));
    departure.leg_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(3500.0)));
    let mut navaid = RouteWaypoint::new(named(
        NamedPointKind::Navaid,
        "FFM",
        "Frankfurt VOR",
        50.05,
        8.63,
    ));
    navaid.leg_altitude = Some(PlannedAltitude::FlightLevel(65));
    navaid.leg_wind = Some(ManualWind {
        direction: DegreesTrue::new(250.0),
        speed: Knots(15.0),
    });
    let free = RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: Some("Ridge crossing".to_owned()),
        position: ll(49.5, 9.8),
    }));
    let destination = RouteWaypoint::new(named(
        NamedPointKind::ReportingPoint,
        "ECHO 1",
        "ECHO 1",
        49.0,
        11.7,
    ));

    FlightDoc {
        name: "EDFE → EDQN".to_owned(),
        aircraft_id: Some(crate::aircraft::AircraftId::new("d-eabc").unwrap()),
        power_setting: Some("65%".to_owned()),
        departure_time: Some(Utc.with_ymd_and_hms(2026, 6, 14, 9, 30, 0).unwrap()),
        cruise_altitude: Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0))),
        route: vec![departure, navaid, free, destination],
        alternates: vec![named(
            NamedPointKind::Airport,
            "EDDN",
            "Nürnberg",
            49.5,
            11.08,
        )],
        loading: LoadingScenario {
            name: "Two on board".to_owned(),
            station_loads: vec![StationLoad {
                station: "Front seats".to_owned(),
                mass: Kilograms(155.0),
            }],
            fuel: Liters(120.0),
        },
        fuel_policy: FuelPolicy {
            taxi: Minutes(15.0),
            contingency: Contingency::Fixed(Liters(8.0)),
            final_reserve: Minutes(45.0),
            extra: Liters(10.0),
        },
        weather_snapshot: Some(WeatherSnapshot::new(
            Utc.with_ymd_and_hms(2026, 6, 14, 7, 0, 0).unwrap(),
            json!({"metars": [{"station": "EDDF", "raw": "EDDF 140650Z ..."}]}),
        )),
        notam_snapshot: Some(NotamSnapshot::new(
            Utc.with_ymd_and_hms(2026, 6, 14, 7, 1, 0).unwrap(),
            json!({"notams": ["A1234/26"]}),
        )),
        ..FlightDoc::default()
    }
}

#[test]
fn round_trip_preserves_document() {
    let doc = full_doc();
    let saved = doc.to_json_string().unwrap();
    let loaded = FlightDoc::from_json_str(&saved).unwrap();
    assert_eq!(doc, loaded);
}

#[test]
fn saved_json_carries_format_version() {
    let saved = full_doc().to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(value["format_version"], json!(FLIGHT_FORMAT_VERSION));
}

#[test]
fn snapshot_payload_round_trips_verbatim() {
    let doc = full_doc();
    let loaded = FlightDoc::from_json_str(&doc.to_json_string().unwrap()).unwrap();
    let payload = &loaded.weather_snapshot.unwrap().payload;
    assert_eq!(payload["metars"][0]["station"], json!("EDDF"));
}

#[test]
fn unknown_fields_are_tolerated() {
    let json = r#"{
        "format_version": 1,
        "name": "tolerance",
        "some_future_field": {"nested": [1, 2, 3]},
        "route": [
            {
                "point": {"free": {"position": {"lat": 50.0, "lon": 8.0}, "color": "red"}},
                "leg_altitude": {"flight_level": 75},
                "another_future_field": true
            }
        ]
    }"#;
    let doc = FlightDoc::from_json_str(json).unwrap();
    assert_eq!(doc.name, "tolerance");
    assert_eq!(doc.route.len(), 1);
    assert_eq!(
        doc.route[0].leg_altitude,
        Some(PlannedAltitude::FlightLevel(75))
    );
}

#[test]
fn missing_fields_default() {
    let doc = FlightDoc::from_json_str(r#"{"name": "bare"}"#).unwrap();
    assert_eq!(doc.format_version, FLIGHT_FORMAT_VERSION);
    assert_eq!(doc.rules, FlightRules::Vfr);
    assert!(doc.route.is_empty());
    assert_eq!(doc.loading.name, "Standard");
    // NCO template defaults.
    assert_eq!(doc.fuel_policy.taxi, Minutes(10.0));
    assert_eq!(doc.fuel_policy.contingency, Contingency::PercentOfTrip(5.0));
    assert_eq!(doc.fuel_policy.final_reserve, Minutes(30.0));
    assert_eq!(doc.fuel_policy.extra, Liters(0.0));
}

#[test]
fn empty_object_is_a_default_document() {
    let doc = FlightDoc::from_json_str("{}").unwrap();
    assert_eq!(doc, FlightDoc::default());
}

#[test]
fn newer_format_version_is_refused() {
    let result = FlightDoc::from_json_str(r#"{"format_version": 99}"#);
    assert!(matches!(
        result,
        Err(FlightError::Version(VersionError::TooNew {
            found: 99,
            supported: 1
        }))
    ));
}

#[test]
fn unknown_old_version_is_refused() {
    // Version 0 was never written; the scaffold has no migration for it.
    let result = FlightDoc::from_json_str(r#"{"format_version": 0}"#);
    assert!(matches!(
        result,
        Err(FlightError::Version(VersionError::NoMigration { from: 0 }))
    ));
}

#[test]
fn non_object_root_is_refused() {
    assert!(matches!(
        FlightDoc::from_json_str("[]"),
        Err(FlightError::Version(VersionError::NotAnObject))
    ));
}

#[test]
fn route_point_accessors() {
    let airport = named(
        NamedPointKind::Airport,
        "EDDF",
        "Frankfurt/Main",
        50.0379,
        8.5622,
    );
    assert_eq!(airport.ident(), Some("EDDF"));
    assert_eq!(airport.label(), "EDDF");
    assert_eq!(airport.position(), ll(50.0379, 8.5622));

    let free_named = RoutePoint::Free(FreePoint {
        name: Some("Lake bend".to_owned()),
        position: ll(48.0, 11.0),
    });
    assert_eq!(free_named.ident(), None);
    assert_eq!(free_named.label(), "Lake bend");

    let free_anon = RoutePoint::Free(FreePoint {
        name: None,
        position: ll(48.0, 11.0),
    });
    assert_eq!(free_anon.label(), "48.00000°N 11.00000°E");
}

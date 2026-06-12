//! FPL tests: golden messages for a reference flight (assert the exact
//! generated text) plus per-item validator positive/negative cases
//! (plan §7 "FPL format").

use chrono::{TimeZone as _, Utc};
use strata_data::domain::LatLon;

use crate::aircraft::{AircraftId, AircraftProfile, PowerSetting};
use crate::compute::ComputedFlight;
use crate::corridor::{Corridor, CorridorParams};
use crate::flight::{
    FlightDoc, FreePoint, NamedPoint, NamedPointKind, PlannedAltitude, RoutePoint, RouteWaypoint,
};
use crate::fuel::FuelLadder;
use crate::navlog::{NavLog, NavLogTotals};
use crate::perf::PhasePlan;
use crate::units::{Kilograms, Knots, Liters, LitersPerHour, Minutes, NauticalMiles};
use crate::wb::WbReport;

use super::*;

fn named(kind: NamedPointKind, id: &str, lat: f64, lon: f64) -> RoutePoint {
    RoutePoint::Named(NamedPoint {
        kind,
        id: id.to_owned(),
        name: id.to_owned(),
        position: LatLon::new(lat, lon).expect("valid"),
    })
}

fn free(lat: f64, lon: f64) -> RoutePoint {
    RoutePoint::Free(FreePoint {
        name: None,
        position: LatLon::new(lat, lon).expect("valid"),
    })
}

fn aircraft() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("d-eabc").expect("valid"));
    profile.registration = "D-EABC".to_owned();
    profile.type_designator = "C172".to_owned();
    profile.weight_balance.max_takeoff = Kilograms(1157.0);
    profile.performance.cruise_settings = vec![PowerSetting {
        name: "65%".to_owned(),
        tas: Knots(107.0),
        fuel_flow: LitersPerHour(26.0),
    }];
    profile.performance.taxi_fuel_flow = LitersPerHour(6.0);
    profile // equipment stays at the V/S template defaults
}

/// Reference flight EDFE → 4945N00930E → EDQN, alternate EDQC,
/// off-blocks 14 Jun 2026 09:30Z, 4500 ft, 105 L loaded.
fn doc() -> FlightDoc {
    let mut doc = FlightDoc::new("EDFE → EDQN");
    doc.route = vec![
        RouteWaypoint::new(named(NamedPointKind::Airport, "EDFE", 49.96, 8.64)),
        RouteWaypoint::new(free(49.75, 9.5)),
        RouteWaypoint::new(named(NamedPointKind::Airport, "EDQN", 49.99, 10.58)),
    ];
    doc.alternates = vec![named(NamedPointKind::Airport, "EDQC", 50.26, 10.99)];
    doc.departure_time = Some(Utc.with_ymd_and_hms(2026, 6, 14, 9, 30, 0).unwrap());
    doc.cruise_altitude = Some(PlannedAltitude::Amsl(
        strata_data::domain::MetersAmsl::from_feet(4500.0),
    ));
    doc.loading.fuel = Liters(105.0);
    doc
}

/// `generate` reads only the nav-log totals from the computed flight.
fn computed(ete_minutes: f64) -> ComputedFlight {
    ComputedFlight {
        legs: Vec::new(),
        corridor: Corridor {
            params: CorridorParams::default(),
            samples: Vec::new(),
            crossings: Vec::new(),
        },
        winds: Vec::new(),
        phases: PhasePlan {
            segments: Vec::new(),
            toc: None,
            tod: None,
            total_duration: Minutes(0.0),
            total_fuel: Liters(0.0),
        },
        weight_balance: WbReport {
            states: Vec::new(),
            burn_track: Vec::new(),
        },
        fuel: FuelLadder {
            taxi: Liters(1.0),
            trip: Liters(14.5),
            contingency: Liters(0.7),
            alternate: Liters(0.0),
            final_reserve: Liters(13.0),
            extra: Liters(0.0),
            minimum_required: Liters(29.2),
            loaded: Liters(105.0),
            margin: Liters(75.8),
        },
        conflicts: Vec::new(),
        navlog: NavLog {
            rows: Vec::new(),
            totals: NavLogTotals {
                distance: NauticalMiles(82.0),
                ete: Minutes(ete_minutes),
                fuel: Liters(14.5),
            },
        },
    }
}

fn pilot() -> PilotInfo {
    PilotInfo {
        pilot_in_command: "Peter Schmidt".to_owned(),
        persons_on_board: Some(2),
        aircraft_color: Some("white blue".to_owned()),
    }
}

#[test]
fn golden_reference_message() {
    // Worked values: registration D-EABC ⇒ DEABC; MTOW 1157 kg ⇒ wake L;
    // TAS 107 ⇒ N0107; 4500 ft ⇒ A045; free point 49.75°/9.5° ⇒
    // 49°45'N 009°30'E; EET 79 min ⇒ 0119; endurance
    // (105 L − 10 min × 6 L/h) / 26 L/h = 104/26 = 4.0 h ⇒ E/0400.
    let message = generate(&doc(), &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert_eq!(
        message,
        "(FPL-DEABC-VG\n\
         -C172/L-V/S\n\
         -EDFE0930\n\
         -N0107A045 DCT 4945N00930E DCT\n\
         -EDQN0119 EDQC\n\
         -DOF/260614\n\
         -E/0400 P/2 A/WHITE BLUE C/PETER SCHMIDT)"
    );
}

#[test]
fn golden_zzzz_destination_goes_to_item18() {
    // A free-point destination files as ZZZZ with DEST/coordinates.
    let mut doc = doc();
    doc.route[2] = RouteWaypoint::new(free(50.1, 9.9));
    doc.alternates.clear();
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert_eq!(
        message,
        "(FPL-DEABC-VG\n\
         -C172/L-V/S\n\
         -EDFE0930\n\
         -N0107A045 DCT 4945N00930E DCT\n\
         -ZZZZ0119\n\
         -DOF/260614 DEST/5006N00954E\n\
         -E/0400 P/2 A/WHITE BLUE C/PETER SCHMIDT)"
    );
}

#[test]
fn route_idents_used_where_they_fit_else_coordinates() {
    // A navaid ident joins as-is; a reporting point with a space falls
    // back to coordinates; a two-point route is plain DCT.
    let mut doc = doc();
    doc.route[1] = RouteWaypoint::new(named(NamedPointKind::Navaid, "WUR", 49.8, 9.6));
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert!(
        message.contains("-N0107A045 DCT WUR DCT\n"),
        "got: {message}"
    );

    doc.route[1] = RouteWaypoint::new(named(NamedPointKind::ReportingPoint, "ECHO 1", 49.8, 9.6));
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert!(
        message.contains("-N0107A045 DCT 4948N00936E DCT\n"),
        "got: {message}"
    );

    doc.route.remove(1);
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert!(message.contains("-N0107A045 DCT\n"), "got: {message}");
}

#[test]
fn level_falls_back_to_vfr_and_fl_formats() {
    let mut doc = doc();
    doc.cruise_altitude = None;
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert!(message.contains("-N0107VFR DCT"), "got: {message}");

    doc.cruise_altitude = Some(PlannedAltitude::FlightLevel(85));
    let message = generate(&doc, &aircraft(), &computed(79.0), &pilot()).expect("generates");
    assert!(message.contains("-N0107F085 DCT"), "got: {message}");
}

#[test]
fn persons_unknown_files_as_tbn() {
    let mut pilot = pilot();
    pilot.persons_on_board = None;
    pilot.aircraft_color = None;
    let message = generate(&doc(), &aircraft(), &computed(79.0), &pilot).expect("generates");
    assert!(
        message.ends_with("-E/0400 P/TBN C/PETER SCHMIDT)"),
        "got: {message}"
    );
}

#[test]
fn callsign_default_overrides_the_registration_in_item7() {
    let mut with_callsign = aircraft();
    with_callsign.callsign = "fly 23".to_owned();
    let message = generate(&doc(), &with_callsign, &computed(79.0), &pilot()).expect("generates");
    assert!(message.starts_with("(FPL-FLY23-VG"), "got: {message}");

    // Whitespace-only callsigns fall back to the registration.
    let mut blank_callsign = aircraft();
    blank_callsign.callsign = "  ".to_owned();
    let message = generate(&doc(), &blank_callsign, &computed(79.0), &pilot()).expect("generates");
    assert!(message.starts_with("(FPL-DEABC-VG"), "got: {message}");
}

#[test]
fn missing_data_is_reported_per_item() {
    let item_of = |result: Result<String, FplError>| match result.unwrap_err() {
        FplError::MissingData { item, .. } => item,
        other => panic!("expected MissingData, got {other:?}"),
    };

    let mut no_registration = aircraft();
    no_registration.registration.clear();
    assert_eq!(
        item_of(generate(
            &doc(),
            &no_registration,
            &computed(79.0),
            &pilot()
        )),
        7
    );

    let mut no_type = aircraft();
    no_type.type_designator.clear();
    assert_eq!(
        item_of(generate(&doc(), &no_type, &computed(79.0), &pilot())),
        9
    );

    let mut no_time = doc();
    no_time.departure_time = None;
    assert_eq!(
        item_of(generate(&no_time, &aircraft(), &computed(79.0), &pilot())),
        13
    );

    let mut no_cruise = aircraft();
    no_cruise.performance.cruise_settings.clear();
    assert_eq!(
        item_of(generate(&doc(), &no_cruise, &computed(79.0), &pilot())),
        15
    );

    // EET of zero means the nav log never computed.
    assert_eq!(
        item_of(generate(&doc(), &aircraft(), &computed(0.0), &pilot())),
        16
    );

    let mut no_fuel = doc();
    no_fuel.loading.fuel = Liters(0.0);
    assert_eq!(
        item_of(generate(&no_fuel, &aircraft(), &computed(79.0), &pilot())),
        19
    );

    let mut no_pic = pilot();
    no_pic.pilot_in_command = "  ".to_owned();
    assert_eq!(
        item_of(generate(&doc(), &aircraft(), &computed(79.0), &no_pic)),
        19
    );
}

// ── per-item validators ────────────────────────────────────────────────

#[test]
fn item7_aircraft_identification() {
    assert!(validate_item(7, "DEABC").is_ok());
    assert!(validate_item(7, "N123AB").is_ok());
    assert!(validate_item(7, "D-EABC").is_err(), "hyphens never file");
    assert!(validate_item(7, "").is_err());
    assert!(validate_item(7, "TOOLONGID").is_err());
    assert!(validate_item(7, "1234").is_err(), "needs a letter");
    assert!(validate_item(7, "deabc").is_err(), "uppercase only");
}

#[test]
fn item8_flight_rules() {
    assert!(validate_item(8, "V").is_ok());
    assert!(validate_item(8, "VG").is_ok());
    assert!(validate_item(8, "IS").is_ok());
    assert!(validate_item(8, "XG").is_err());
    assert!(validate_item(8, "VQ").is_err());
    assert!(validate_item(8, "VGX").is_err());
    assert!(validate_item(8, "").is_err());
}

#[test]
fn item9_type_and_wake() {
    assert!(validate_item(9, "C172/L").is_ok());
    assert!(validate_item(9, "BE36/L").is_ok());
    assert!(validate_item(9, "A388/J").is_ok());
    assert!(validate_item(9, "C172/X").is_err(), "wake must be LMHJ");
    assert!(validate_item(9, "C/L").is_err(), "type too short");
    assert!(validate_item(9, "C1722/L").is_err(), "type too long");
    assert!(validate_item(9, "C172L").is_err(), "missing separator");
    assert!(validate_item(9, "1234/L").is_err(), "type needs a letter");
}

#[test]
fn item10_equipment() {
    assert!(validate_item(10, "V/S").is_ok());
    assert!(validate_item(10, "SDFGLO/S").is_ok());
    assert!(validate_item(10, "N/N").is_ok());
    assert!(validate_item(10, "VS").is_err());
    assert!(validate_item(10, "/S").is_err());
    assert!(validate_item(10, "V/").is_err());
    assert!(validate_item(10, "v/s").is_err());
}

#[test]
fn item13_departure() {
    assert!(validate_item(13, "EDFE0930").is_ok());
    assert!(validate_item(13, "ZZZZ2359").is_ok());
    assert!(validate_item(13, "EDFE2400").is_err(), "no hour 24");
    assert!(validate_item(13, "EDFE0960").is_err(), "no minute 60");
    assert!(validate_item(13, "EDF0930").is_err(), "indicator too short");
    assert!(validate_item(13, "edfe0930").is_err());
}

#[test]
fn item15_speed_level_route() {
    assert!(validate_item(15, "N0107A045 DCT WUR DCT").is_ok());
    assert!(validate_item(15, "N0107VFR DCT").is_ok());
    assert!(validate_item(15, "N0107F085 DCT 4945N00930E DCT").is_ok());
    assert!(validate_item(15, "K0200A045 DCT").is_ok());
    assert!(validate_item(15, "M082F350 DCT").is_ok());
    assert!(
        validate_item(15, "0107A045 DCT").is_err(),
        "speed prefix missing"
    );
    assert!(
        validate_item(15, "N107A045 DCT").is_err(),
        "speed needs 4 digits"
    );
    assert!(
        validate_item(15, "N0107A45 DCT").is_err(),
        "level needs 3 digits"
    );
    assert!(
        validate_item(15, "N0107A045 W%R").is_err(),
        "bad route element"
    );
    assert!(
        validate_item(15, "N0107A045 9945N00930E").is_err(),
        "latitude over 90"
    );
    assert!(
        validate_item(15, "N0107A045 4965N00930E").is_err(),
        "minutes over 59"
    );
    assert!(validate_item(15, "").is_err());
}

#[test]
fn item16_destination_and_alternates() {
    assert!(validate_item(16, "EDQN0119").is_ok());
    assert!(validate_item(16, "EDQN0119 EDQC").is_ok());
    assert!(validate_item(16, "EDQN0119 EDQC EDQD").is_ok());
    assert!(
        validate_item(16, "EDQN9959").is_ok(),
        "EET is a duration, not a clock"
    );
    assert!(validate_item(16, "EDQN0170").is_err(), "minutes over 59");
    assert!(
        validate_item(16, "EDQN0119 EDQC EDQD EDQE").is_err(),
        "max two alternates"
    );
    assert!(validate_item(16, "EDQN0119 EDQC5").is_err());
    assert!(validate_item(16, "EDQ0119").is_err());
}

#[test]
fn item18_other_information() {
    assert!(validate_item(18, "0").is_ok());
    assert!(validate_item(18, "DOF/260614").is_ok());
    assert!(validate_item(18, "DOF/260614 DEST/5006N00954E").is_ok());
    assert!(validate_item(18, "garbage").is_err());
    assert!(validate_item(18, "DOF/").is_err(), "empty group value");
    assert!(validate_item(18, "").is_err());
}

#[test]
fn item19_supplementary_groups() {
    assert!(validate_item(19, "E/0400 P/2 C/LEO SEIBOLD").is_ok());
    assert!(validate_item(19, "E/0400 P/TBN A/WHITE BLUE C/NAME").is_ok());
    assert!(validate_item(19, "E/0400 R/E S/P J/L C/NAME").is_ok());
    assert!(
        validate_item(19, "E/0470 C/NAME").is_err(),
        "minutes over 59"
    );
    assert!(
        validate_item(19, "E/0400 P/ABC C/NAME").is_err(),
        "persons must be digits/TBN"
    );
    assert!(
        validate_item(19, "E/0400 R/X C/NAME").is_err(),
        "radio letters UVE only"
    );
    assert!(
        validate_item(19, "NAME E/0400").is_err(),
        "value before any group"
    );
    assert!(validate_item(19, "").is_err());
}

#[test]
fn unsupported_items_are_rejected() {
    for item in [0u8, 1, 6, 11, 12, 14, 17, 20, 255] {
        assert!(
            matches!(validate_item(item, "X"), Err(FplError::InvalidItem { .. })),
            "item {item} must be unsupported"
        );
    }
}

#[test]
fn multibyte_input_is_rejected_without_panicking() {
    // Regression: validators used to slice by byte index and panicked when a
    // multibyte char straddled the boundary (e.g. 'Ö' at byte 4 of "EDDÖ123").
    let cases: &[(u8, &str)] = &[
        (7, "DÖABC"),
        (8, "VÖ"),
        (9, "CÖ72/L"),
        (10, "SÖ/C"),
        (13, "EDDÖ123"),
        (15, "N0107A045 DCT 5030N0094Ö DCT"),
        (15, "N123Ö4VFR DCT"),
        (16, "EDDÖ123"),
        (18, "DÖF/260614"),
    ];
    for (item, value) in cases {
        assert!(
            validate_item(*item, value).is_err(),
            "item {item} {value:?} must be rejected, not panic"
        );
    }
    // Item 19 free-text groups (pilot name, colour, …) accept non-ASCII by
    // design — they must simply not panic.
    assert!(validate_item(19, "E/0400 P/2 C/SÖREN MÜLLER").is_ok());
}

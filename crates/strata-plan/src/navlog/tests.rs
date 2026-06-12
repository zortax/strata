//! Nav-log tests around one fully worked example (plan §7): a 38.6 NM
//! two-leg route along the 50°N parallel with climb/cruise/descent
//! phases, the classic E6B wind case on leg 0 and a pure tailwind on
//! leg 1. Every asserted number is derived by hand in the comments.

use chrono::{NaiveDate, TimeZone as _, Utc};
use strata_data::domain::{
    Airport, AirportKind, Frequency, FrequencyKind, IcaoCode, LatLon, Meters, MetersAmsl,
    RadioFrequency,
};

use crate::aircraft::{AircraftId, AircraftProfile, PowerSetting};
use crate::flight::{
    FlightDoc, FreePoint, NamedPoint, NamedPointKind, PlannedAltitude, RoutePoint, RouteWaypoint,
};
use crate::perf::{PhaseKind, PhasePlan, PhaseSegment, ProfileMarker};
use crate::route::intermediate_point;
use crate::sources::{MagvarSource, Provenance, SourceError, WindsAloft};
use crate::units::{
    Celsius, DegreesTrue, Knots, Liters, LitersPerHour, MagneticVariation, Minutes,
};
use crate::wind::{LegWind, LegWindOrigin, WindTriangle};

use super::*;

/// Germany-style constant variation, 2° east: MT = TT − 2.
struct FixedVariation;

impl MagvarSource for FixedVariation {
    fn magvar(&self, _: LatLon, _: NaiveDate) -> Result<MagneticVariation, SourceError> {
        Ok(MagneticVariation(2.0))
    }
}

fn airport_point(id: &str, lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Named(NamedPoint {
        kind: NamedPointKind::Airport,
        id: id.to_owned(),
        name: id.to_owned(),
        position: LatLon::new(lat, lon).expect("valid"),
    }))
}

fn free_point(name: &str, lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: Some(name.to_owned()),
        position: LatLon::new(lat, lon).expect("valid"),
    }))
}

fn aircraft() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("d-test").expect("valid id"));
    profile.performance.cruise_settings = vec![PowerSetting {
        name: "65%".to_owned(),
        tas: Knots(110.0),
        fuel_flow: LitersPerHour(26.0),
    }];
    profile.performance.taxi_fuel_flow = LitersPerHour(6.0);
    profile
}

fn frequency(name: &str, mhz: f64, kind: FrequencyKind) -> Frequency {
    Frequency {
        frequency: RadioFrequency::from_mhz(mhz),
        name: name.to_owned(),
        kind,
        primary: true,
    }
}

fn airport(ident: &str, lat: f64, lon: f64, frequencies: Vec<Frequency>) -> Airport {
    Airport {
        ident: Some(IcaoCode::new(ident).expect("valid icao")),
        name: ident.to_owned(),
        kind: AirportKind::Airfield,
        position: LatLon::new(lat, lon).expect("valid"),
        elevation: MetersAmsl(0.0),
        runways: Vec::new(),
        frequencies,
    }
}

/// The worked-example fixture. Route EDXA (50°N 8°E) → MITTE (50°N 8.5°E)
/// → EDXC (50°N 9°E):
///
/// - Leg length (haversine, R = 6 371 008.8 m):
///   2·asin(cos 50°·sin 0.25°)·R = 2·asin(0.6427876·0.00436331)·R
///   = 0.00560934 rad·R = **35 737.1 m = 19.2965 NM** per leg,
///   total 71 474.3 m = 38.593 NM.
/// - Phases: climb 0→18 520 m (10 NM) to 4500 ft (1371.6 m), 10 min,
///   5.0 L; cruise 18 520→62 214.3 m, 20 min, 8.0 L; descent the last
///   9260 m (5 NM), 6 min, 1.5 L. TOC/TOD markers at the seams.
/// - Winds: leg 0 the classic E6B case (track ≈090°, TAS 110 kt, wind
///   040°/20 kt ⇒ WCA −8.0°, GS 96.1 kt); leg 1 direct tailwind
///   270°/15 kt ⇒ WCA 0°, GS 125 kt.
/// - Taxi fuel: 10 min × 6 L/h = 1.0 L; loaded 100 L.
struct Fixture {
    doc: FlightDoc,
    aircraft: AircraftProfile,
    winds: Vec<LegWind>,
    phases: PhasePlan,
    airports: Vec<Airport>,
    total: f64,
}

fn fixture() -> Fixture {
    let a = LatLon::new(50.0, 8.0).expect("valid");
    let c = LatLon::new(50.0, 9.0).expect("valid");

    let mut doc = FlightDoc::new("worked example");
    doc.route = vec![
        airport_point("EDXA", 50.0, 8.0),
        free_point("MITTE", 50.0, 8.5),
        airport_point("EDXC", 50.0, 9.0),
    ];
    doc.departure_time = Some(Utc.with_ymd_and_hms(2026, 6, 14, 9, 30, 0).unwrap());
    doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(1371.6)));
    doc.loading.fuel = Liters(100.0);

    let total = crate::route::total_distance(&doc.route).0; // 71 474.3 m
    let cruise_alt = MetersAmsl(1371.6);
    let toc_along = 18_520.0;
    let tod_along = total - 9260.0;
    let segment = |kind, start: f64, end: f64, alt0: f64, alt1: f64, tas, min, fuel| PhaseSegment {
        kind,
        start_along_track: Meters(start),
        end_along_track: Meters(end),
        start_altitude: MetersAmsl(alt0),
        end_altitude: MetersAmsl(alt1),
        tas: Knots(tas),
        duration: Minutes(min),
        fuel: Liters(fuel),
    };
    let phases = PhasePlan {
        segments: vec![
            segment(
                PhaseKind::Climb,
                0.0,
                toc_along,
                0.0,
                1371.6,
                75.0,
                10.0,
                5.0,
            ),
            segment(
                PhaseKind::Cruise,
                toc_along,
                tod_along,
                1371.6,
                1371.6,
                110.0,
                20.0,
                8.0,
            ),
            segment(
                PhaseKind::Descent,
                tod_along,
                total,
                1371.6,
                0.0,
                100.0,
                6.0,
                1.5,
            ),
        ],
        toc: Some(ProfileMarker {
            along_track: Meters(toc_along),
            position: intermediate_point(a, c, toc_along / total),
            altitude: cruise_alt,
        }),
        tod: Some(ProfileMarker {
            along_track: Meters(tod_along),
            position: intermediate_point(a, c, tod_along / total),
            altitude: cruise_alt,
        }),
        total_duration: Minutes(36.0),
        total_fuel: Liters(14.5),
    };

    let leg0_track = crate::route::initial_true_track(a, LatLon::new(50.0, 8.5).expect("valid"));
    let leg1_track = crate::route::initial_true_track(LatLon::new(50.0, 8.5).expect("valid"), c);
    let winds = vec![
        LegWind {
            leg_index: 0,
            wind: WindsAloft {
                direction: DegreesTrue::new(40.0),
                speed: Knots(20.0),
                temperature: Celsius(5.0),
                temperature_provenance: Provenance::Real,
            },
            origin: LegWindOrigin::Sampled,
            triangle: WindTriangle {
                wind_correction_angle_deg: -8.0,
                true_heading: DegreesTrue::new(leg0_track.0 - 8.0),
                ground_speed: Knots(96.1),
            },
        },
        LegWind {
            leg_index: 1,
            wind: WindsAloft {
                direction: DegreesTrue::new(270.0),
                speed: Knots(15.0),
                temperature: Celsius(5.0),
                temperature_provenance: Provenance::Isa,
            },
            origin: LegWindOrigin::Manual,
            triangle: WindTriangle {
                wind_correction_angle_deg: 0.0,
                true_heading: leg1_track,
                ground_speed: Knots(125.0),
            },
        },
    ];

    let airports = vec![
        airport(
            "EDXA",
            50.0,
            8.0,
            vec![
                frequency("EDXA TURM", 119.9, FrequencyKind::Tower),
                frequency("LANGEN INFORMATION", 128.95, FrequencyKind::Fis),
            ],
        ),
        airport(
            "EDXC",
            50.0,
            9.0,
            vec![frequency("EDXC TURM", 120.4, FrequencyKind::Tower)],
        ),
    ];

    Fixture {
        doc,
        aircraft: aircraft(),
        winds,
        phases,
        airports,
        total,
    }
}

fn build(fixture: &Fixture) -> NavLog {
    build_navlog(
        &fixture.doc,
        &fixture.aircraft,
        &fixture.winds,
        &fixture.phases,
        &FixedVariation,
        &fixture.airports,
    )
    .expect("consistent fixture")
}

#[test]
fn worked_example_row_structure() {
    let f = fixture();
    let log = build(&f);
    // Departure + TOC + MITTE + TOD + EDXC.
    let labels: Vec<&str> = log.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["EDXA", "TOC", "MITTE", "TOD", "EDXC"]);
    assert_eq!(log.rows[1].kind, NavLogRowKind::TopOfClimb);
    assert_eq!(log.rows[3].kind, NavLogRowKind::TopOfDescent);

    // Leg length 19.2965 NM each (worked in the fixture docs); row
    // distances split by TOC/TOD and sum back to the total.
    assert!((f.total - 71_474.3).abs() < 1.0, "got {}", f.total);
    let dist = |i: usize| log.rows[i].distance.unwrap().0;
    assert!((dist(1) - 10.0).abs() < 1e-9); // TOC at exactly 10 NM
    assert!((dist(2) - 9.2965).abs() < 0.002);
    assert!((dist(3) - 14.2966).abs() < 0.002);
    assert!((dist(4) - 5.0).abs() < 1e-6);
    let sum: f64 = (1..=4).map(dist).sum();
    assert!((sum - log.totals.distance.0).abs() < 1e-9);
    assert!((log.totals.distance.0 - 38.593).abs() < 0.002);
}

#[test]
fn departure_row_is_all_none() {
    let f = fixture();
    let row = &build(&f).rows[0];
    assert_eq!(row.kind, NavLogRowKind::Waypoint);
    assert!(row.altitude.is_none());
    assert!(row.true_track.is_none());
    assert!(row.magnetic_track.is_none());
    assert!(row.wind.is_none());
    assert!(row.wind_correction_angle_deg.is_none());
    assert!(row.magnetic_heading.is_none());
    assert!(row.tas.is_none());
    assert!(row.ground_speed.is_none());
    assert!(row.distance.is_none());
    assert!(row.ete.is_none());
    assert!(row.eta.is_none());
    assert!(row.leg_fuel.is_none());
    assert!(row.cumulative_fuel.is_none());
    assert!(row.remaining_fuel.is_none());
    assert!(row.frequency.is_none());
    assert!(row.notes.is_empty());
}

#[test]
fn worked_example_headings_and_speeds() {
    let f = fixture();
    let log = build(&f);
    // Leg 0 initial true track along the 50°N parallel:
    // atan2(sin Δλ·cos φ, cos φ·sin φ·(1−cos Δλ)) ≈ 89.81°.
    let tt0 = log.rows[1].true_track.unwrap().0;
    assert!((tt0 - 89.81).abs() < 0.02, "got {tt0}");
    // MT = TT − 2°E variation ("east is least").
    let mt0 = log.rows[1].magnetic_track.unwrap().0;
    assert!((mt0 - (tt0 - 2.0)).abs() < 1e-9);
    // E6B case: WCA −8° ⇒ TH = TT − 8 ⇒ MH = TT − 10.
    assert_eq!(log.rows[1].wind_correction_angle_deg, Some(-8.0));
    let mh0 = log.rows[1].magnetic_heading.unwrap().0;
    assert!((mh0 - (tt0 - 10.0)).abs() < 1e-9, "got {mh0}");
    assert_eq!(log.rows[1].tas, Some(Knots(110.0)));
    assert_eq!(log.rows[1].ground_speed, Some(Knots(96.1)));
    // Leg 1: zero WCA ⇒ MH = MT.
    let row3 = &log.rows[3];
    assert_eq!(row3.wind_correction_angle_deg, Some(0.0));
    assert!((row3.magnetic_heading.unwrap().0 - row3.magnetic_track.unwrap().0).abs() < 1e-9);
    assert_eq!(row3.ground_speed, Some(Knots(125.0)));
    // The wind itself is carried for the PLOG wind column.
    assert_eq!(log.rows[2].wind.unwrap().speed, Knots(20.0));
}

#[test]
fn worked_example_times() {
    let f = fixture();
    let log = build(&f);
    let ete = |i: usize| log.rows[i].ete.unwrap().0;
    // TOC: 10.0 NM at GS 96.1 ⇒ 10/96.1×60 = 6.2435 min.
    assert!((ete(1) - 6.2435).abs() < 0.002, "got {}", ete(1));
    // MITTE: 9.2965 NM at 96.1 ⇒ 5.8043 min.
    assert!((ete(2) - 5.8043).abs() < 0.003);
    // TOD: 14.2966 NM at 125 ⇒ 6.8624 min.
    assert!((ete(3) - 6.8624).abs() < 0.003);
    // EDXC: 5.0 NM at 125 ⇒ 2.4 min exactly.
    assert!((ete(4) - 2.4).abs() < 1e-9);
    // Totals: 21.3102 min.
    assert!((log.totals.ete.0 - 21.3102).abs() < 0.005);
    // Cumulative ETA: 09:30 + 21.31 min = 09:51:18.6.
    let eta = log.rows[4].eta.unwrap();
    let expected = Utc.with_ymd_and_hms(2026, 6, 14, 9, 51, 19).unwrap();
    assert!((eta - expected).num_seconds().abs() <= 1, "got {eta}");
    // ETAs increase monotonically.
    assert!(log.rows[1].eta.unwrap() < log.rows[2].eta.unwrap());
    assert!(log.rows[2].eta.unwrap() < log.rows[3].eta.unwrap());
}

#[test]
fn worked_example_fuel_ladder_down_the_rows() {
    let f = fixture();
    let log = build(&f);
    let leg_fuel = |i: usize| log.rows[i].leg_fuel.unwrap().0;
    // TOC: the whole climb segment ⇒ 5.0 L.
    assert!((leg_fuel(1) - 5.0).abs() < 1e-9);
    // MITTE: cruise share (35737.1−18520)/(62214.3−18520) × 8.0
    //      = 17217.1/43694.3 × 8.0 = 3.1522 L.
    assert!((leg_fuel(2) - 3.1522).abs() < 0.002);
    // TOD: remaining cruise ⇒ 8.0 − 3.1522 = 4.8478 L.
    assert!((leg_fuel(3) - 4.8478).abs() < 0.002);
    // EDXC: the descent ⇒ 1.5 L.
    assert!((leg_fuel(4) - 1.5).abs() < 1e-9);
    // Cumulative reaches the trip fuel.
    assert!((log.rows[4].cumulative_fuel.unwrap().0 - 14.5).abs() < 1e-9);
    assert!((log.totals.fuel.0 - 14.5).abs() < 1e-12);
    // Remaining: 100 L loaded − 1.0 L taxi (10 min × 6 L/h) − burn.
    assert!((log.rows[1].remaining_fuel.unwrap().0 - 94.0).abs() < 1e-9);
    assert!((log.rows[4].remaining_fuel.unwrap().0 - 84.5).abs() < 1e-9);
}

#[test]
fn worked_example_altitudes() {
    let f = fixture();
    let log = build(&f);
    // TOC/TOD rows carry the marker altitude.
    assert_eq!(
        log.rows[1].altitude,
        Some(PlannedAltitude::Amsl(MetersAmsl(1371.6)))
    );
    assert_eq!(
        log.rows[3].altitude,
        Some(PlannedAltitude::Amsl(MetersAmsl(1371.6)))
    );
    // MITTE: the arriving leg's altitude (cruise default here).
    assert_eq!(
        log.rows[2].altitude,
        Some(PlannedAltitude::Amsl(MetersAmsl(1371.6)))
    );
    // Destination: the profile's end altitude (field level).
    assert_eq!(
        log.rows[4].altitude,
        Some(PlannedAltitude::Amsl(MetersAmsl(0.0)))
    );
}

#[test]
fn frequency_suggestions_follow_the_documented_heuristic() {
    let f = fixture();
    let log = build(&f);
    // En-route row near EDXA: FIS beats the tower (en-route priority).
    assert_eq!(
        log.rows[1].frequency.as_ref().map(|f| f.name.as_str()),
        Some("LANGEN INFORMATION")
    );
    // TOD sits 5 NM from EDXC: nearest airport, no FIS there ⇒ tower.
    assert_eq!(
        log.rows[3].frequency.as_ref().map(|f| f.name.as_str()),
        Some("EDXC TURM")
    );
    // Destination row is the airport itself ⇒ its tower (airport priority).
    assert_eq!(
        log.rows[4].frequency.as_ref().map(|f| f.name.as_str()),
        Some("EDXC TURM")
    );
}

#[test]
fn missing_leg_wind_falls_back_to_phase_time() {
    let mut f = fixture();
    f.winds.remove(1); // leg 1 loses its wind
    let log = build(&f);
    let row3 = &log.rows[3]; // TOD, on leg 1
    assert!(row3.wind.is_none());
    assert!(row3.magnetic_heading.is_none());
    assert!(row3.ground_speed.is_none());
    // Phase time share over (35737.1, 62214.3]: cruise minutes
    // 20 × 26477.2/43694.3 = 12.119 min.
    assert!(
        (row3.ete.unwrap().0 - 12.119).abs() < 0.005,
        "got {}",
        row3.ete.unwrap().0
    );
    // Track/variation are pure geometry and survive without wind.
    assert!(row3.magnetic_track.is_some());
}

#[test]
fn no_fuel_load_means_no_remaining_column() {
    let mut f = fixture();
    f.doc.loading.fuel = Liters(0.0);
    let log = build(&f);
    assert!(log.rows[4].leg_fuel.is_some());
    assert!(log.rows[4].remaining_fuel.is_none());
}

#[test]
fn unknown_power_setting_is_inconsistent() {
    let mut f = fixture();
    f.doc.power_setting = Some("max cruise".to_owned());
    let err = build_navlog(
        &f.doc,
        &f.aircraft,
        &f.winds,
        &f.phases,
        &FixedVariation,
        &f.airports,
    )
    .unwrap_err();
    assert!(matches!(err, NavLogError::InconsistentInput(_)));
}

#[test]
fn short_route_is_inconsistent() {
    let mut f = fixture();
    f.doc.route.truncate(1);
    let err = build_navlog(
        &f.doc,
        &f.aircraft,
        &f.winds,
        &f.phases,
        &FixedVariation,
        &f.airports,
    )
    .unwrap_err();
    assert!(matches!(err, NavLogError::InconsistentInput(_)));
}

#[test]
fn phase_plan_not_spanning_the_route_is_inconsistent() {
    let mut f = fixture();
    for segment in &mut f.phases.segments {
        segment.start_along_track.0 *= 0.5;
        segment.end_along_track.0 *= 0.5;
    }
    let err = build_navlog(
        &f.doc,
        &f.aircraft,
        &f.winds,
        &f.phases,
        &FixedVariation,
        &f.airports,
    )
    .unwrap_err();
    assert!(matches!(err, NavLogError::InconsistentInput(_)));
}

/// Waypoint notes stored on the document surface on the matching rows —
/// the departure row included — while synthetic TOC/TOD rows stay blank
/// (they have no document slot; their position moves with every replan).
#[test]
fn rows_carry_the_waypoints_stored_notes() {
    let mut f = fixture();
    f.doc.route[0].notes = "request taxi".to_owned();
    f.doc.route[1].notes = "report MITTE".to_owned();
    let log = build(&f);
    let notes: Vec<&str> = log.rows.iter().map(|r| r.notes.as_str()).collect();
    // EDXA, TOC, MITTE, TOD, EDXC.
    assert_eq!(notes, ["request taxi", "", "report MITTE", "", ""]);
}

//! Relevance tests: the embedded fixture corpus over a realistic
//! EDDF → EDDM flight, plus synthetic edge cases (corridor margins,
//! expired NOTAMs, altitude bands, EST/PERM ends, supersedure, ordering).

use chrono::{DateTime, NaiveDate, Utc};
use strata_data::domain::{LatLon, Meters, MetersAmsl, Notam};
use strata_data::providers::autorouter::FixtureNotamProvider;

use crate::corridor::{Corridor, CorridorParams, CorridorSample, Station};
use crate::flight::{FreePoint, NamedPoint, NamedPointKind, RoutePoint, RouteWaypoint};
use crate::perf::{PhaseKind, PhasePlan, PhaseSegment};
use crate::route::{great_circle_distance, intermediate_point};
use crate::units::{Knots, Liters, METERS_PER_NAUTICAL_MILE, Minutes};

use super::*;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(y, mo, d)
        .and_then(|date| date.and_hms_opt(h, mi, 0))
        .expect("valid test datetime")
        .and_utc()
}

fn airport(id: &str, lat: f64, lon: f64) -> RoutePoint {
    RoutePoint::Named(NamedPoint {
        kind: NamedPointKind::Airport,
        id: id.to_owned(),
        name: id.to_owned(),
        position: LatLon::new(lat, lon).expect("valid test coordinates"),
    })
}

fn free(lat: f64, lon: f64) -> RoutePoint {
    RoutePoint::Free(FreePoint {
        name: None,
        position: LatLon::new(lat, lon).expect("valid test coordinates"),
    })
}

fn route(points: &[RoutePoint]) -> Vec<RouteWaypoint> {
    points
        .iter()
        .map(|point| RouteWaypoint::new(point.clone()))
        .collect()
}

/// A centerline-only corridor along `points` with ~`spacing_m` stations
/// (terrain/obstacles are irrelevant to NOTAM geometry — only the station
/// positions and the half-width matter).
fn corridor(points: &[RoutePoint], spacing_m: f64, half_width_nm: f64) -> Corridor {
    let mut samples = Vec::new();
    let mut along = 0.0;
    let mut index = 0;
    for (leg_index, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0].position(), pair[1].position());
        let length = great_circle_distance(a, b).0;
        let n = (length / spacing_m).ceil().max(1.0) as usize;
        for i in 0..n {
            let fraction = i as f64 / n as f64;
            samples.push(CorridorSample {
                station: Station {
                    index,
                    leg_index,
                    along_track: Meters(along + length * fraction),
                    position: intermediate_point(a, b, fraction),
                },
                max_terrain: None,
                min_terrain: None,
                tallest_obstacle: None,
            });
            index += 1;
        }
        along += length;
    }
    // Final station exactly on the destination.
    if let Some(last) = points.last() {
        samples.push(CorridorSample {
            station: Station {
                index,
                leg_index: points.len().saturating_sub(2),
                along_track: Meters(along),
                position: last.position(),
            },
            max_terrain: None,
            min_terrain: None,
            tallest_obstacle: None,
        });
    }
    Corridor {
        params: CorridorParams {
            half_width: Meters(half_width_nm * METERS_PER_NAUTICAL_MILE),
            station_spacing: Meters(spacing_m),
            lateral_samples_per_side: 4,
        },
        samples,
        crossings: Vec::new(),
    }
}

/// One synthetic NOTAM in transmission format. `c` may be `"PERM"`, a
/// compact datetime, or carry an `EST` suffix.
fn notam(id: &str, q: &str, a: &str, b: &str, c: &str, e: &str) -> Notam {
    Notam::parse(&format!(
        "{id} NOTAMN\nQ) {q}\nA) {a} B) {b} C) {c}\nE) {e}"
    ))
    .expect("synthetic test NOTAM parses")
}

fn band(floor_ft: f64, ceiling_ft: f64) -> AltitudeBand {
    AltitudeBand {
        floor: MetersAmsl::from_feet(floor_ft),
        ceiling: MetersAmsl::from_feet(ceiling_ft),
    }
}

fn ids(relevant: &[RelevantNotam]) -> Vec<String> {
    relevant
        .iter()
        .map(|entry| entry.notam.id.to_string())
        .collect()
}

// --- the fixture corpus over EDDF → EDDM --------------------------------------

/// EDDF → EDDM on 2026-06-16, off-blocks 09:00Z, ~90 min, 5 NM corridor,
/// GND–5000 ft. Briefing window: one hour before departure to 24 h after.
fn corpus_input<'a>(
    notams: &'a [Notam],
    route: &'a [RouteWaypoint],
    corridor: &'a Corridor,
) -> RelevanceInput<'a> {
    RelevanceInput {
        notams,
        route,
        alternates: &[],
        corridor,
        briefing_window: TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0)),
        flight_window: TimeWindow::new(utc(2026, 6, 16, 9, 0), utc(2026, 6, 16, 10, 30)),
        altitude_band: Some(band(0.0, 5000.0)),
    }
}

fn eddf_eddm() -> Vec<RoutePoint> {
    vec![
        airport("EDDF", 50.0333, 8.5706),
        airport("EDDM", 48.3538, 11.7861),
    ]
}

#[test]
fn fixture_corpus_briefs_an_eddf_eddm_flight() {
    let provider = FixtureNotamProvider::builtin();
    let points = eddf_eddm();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let relevant = relevant_notams(&corpus_input(provider.notams(), &route, &corridor));

    assert_eq!(
        ids(&relevant),
        vec![
            // Departure EDDF (validity-start order): apron NOTAMR, runway
            // closure. The taxiway work is cancelled by its NOTAMC (both
            // dropped), the ILS outage ended 14.06, the TWR frequency
            // change starts 20.06 — all outside the briefing.
            "A1300/26", "A1234/26",
            // Destination EDDM: permanent mast, crane (EST end in
            // September), PAPI, and tomorrow's VOR/DME outage.
            "B0820/26", "B0815/26", "B0612/26", "B0788/26",
            // Corridor (entry order, ties on validity start): GPS jamming
            // (150 NM circle over the departure), glider activity 15 NM
            // around 4959N00827E — both reach the corridor at EDDF.
            "E0231/26", "W0903/26",
        ],
        "off-track NOTAMs never appear: the EDDS aerodrome NOTAMs (~90 km \
         abeam), the ED-R 136A activation (~120 km north) and the expired \
         Oberschleissheim parachute exercise are all filtered"
    );

    // Relevance classes.
    let by_id = |id: &str| {
        relevant
            .iter()
            .find(|entry| entry.notam.id.to_string() == id)
            .unwrap_or_else(|| panic!("{id} is relevant"))
    };
    assert_eq!(
        by_id("A1234/26").relevance,
        NotamRelevance::Aerodrome(IcaoCode::new("EDDF").expect("valid"))
    );
    assert_eq!(
        by_id("B0612/26").relevance,
        NotamRelevance::Aerodrome(IcaoCode::new("EDDM").expect("valid"))
    );
    let NotamRelevance::RouteCorridor { distance_nm } = by_id("W0903/26").relevance else {
        panic!("glider activity classifies by corridor");
    };
    assert_eq!(
        distance_nm.0, 0.0,
        "the 15 NM glider circle contains the departure — centerline inside"
    );

    // Activity: everything overlaps the flight window except tomorrow's
    // VOR/DME outage (listed for context, never raising the badge).
    for entry in &relevant {
        let expected_active = entry.notam.id.to_string() != "B0788/26";
        assert_eq!(
            entry.active_during_flight, expected_active,
            "{} active flag",
            entry.notam.id
        );
    }
}

#[test]
fn corpus_restriction_activation_is_exactly_the_edr() {
    let provider = FixtureNotamProvider::builtin();
    let activations: Vec<String> = provider
        .notams()
        .iter()
        .filter(|notam| is_restriction_activation(notam))
        .map(|notam| notam.id.to_string())
        .collect();
    assert_eq!(activations, vec!["D0452/26"]);
}

/// A detour leg through the Grafenwoehr area pulls the ED-R 136A
/// activation onto the briefing — as a corridor hit, the red badge class.
#[test]
fn fixture_corpus_route_through_the_edr_lists_the_activation() {
    let provider = FixtureNotamProvider::builtin();
    let points = vec![
        airport("EDDF", 50.0333, 8.5706),
        free(49.70, 11.90), // inside the ED-R 136A circle (4942N01156E r10)
        airport("EDDM", 48.3538, 11.7861),
    ];
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let relevant = relevant_notams(&corpus_input(provider.notams(), &route, &corridor));

    let edr = relevant
        .iter()
        .find(|entry| entry.notam.id.to_string() == "D0452/26")
        .expect("the ED-R activation is on the briefing");
    let NotamRelevance::RouteCorridor { distance_nm } = edr.relevance else {
        panic!("the detour route crosses the circle: {:?}", edr.relevance);
    };
    assert_eq!(distance_nm.0, 0.0, "the centerline passes through the area");
    assert!(edr.active_during_flight);
    assert!(is_restriction_activation(&edr.notam));
}

// --- synthetic geometry -------------------------------------------------------

/// Straight test track at 50°N from 8°E to 8.5°E (~35.7 km).
fn straight_track() -> Vec<RoutePoint> {
    vec![airport("AAAA", 50.0, 8.0), airport("BBBB", 50.0, 8.5)]
}

fn synthetic_input<'a>(
    notams: &'a [Notam],
    route: &'a [RouteWaypoint],
    corridor: &'a Corridor,
) -> RelevanceInput<'a> {
    RelevanceInput {
        notams,
        route,
        alternates: &[],
        corridor,
        briefing_window: TimeWindow::new(utc(2026, 6, 15, 6, 0), utc(2026, 6, 16, 6, 0)),
        flight_window: TimeWindow::new(utc(2026, 6, 15, 9, 0), utc(2026, 6, 15, 10, 0)),
        altitude_band: Some(band(0.0, 5000.0)),
    }
}

/// Two warning circles north of the track: 5007N r3 NM reaches into the
/// 5 NM corridor (~7.4 km edge distance), 5008N r2 NM stays ~1.8 km
/// outside it.
#[test]
fn corridor_reach_is_radius_plus_half_width() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let notams = vec![
        notam(
            "W0001/26",
            "EDGG/QWPLW/IV/M/W/000/050/5007N00815E003",
            "EDGG",
            "2606150600",
            "2606152000",
            "PJE INSIDE REACH",
        ),
        notam(
            "W0002/26",
            "EDGG/QWPLW/IV/M/W/000/050/5008N00815E002",
            "EDGG",
            "2606150600",
            "2606152000",
            "PJE JUST OUTSIDE REACH",
        ),
    ];
    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(ids(&relevant), vec!["W0001/26"]);
    let NotamRelevance::RouteCorridor { distance_nm } = &relevant[0].relevance else {
        panic!("classified by corridor");
    };
    // Centre 50°07'N is ~12.9 km north of the track; minus the 3 NM
    // radius the circle edge sits ~4 NM off the centerline.
    assert!(
        (distance_nm.0 - 4.0).abs() < 0.5,
        "edge distance {} NM",
        distance_nm.0
    );
}

#[test]
fn expired_and_not_yet_valid_notams_are_dropped() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    // Directly on the track — only time excludes them.
    let q = "EDGG/QWPLW/IV/M/W/000/050/5000N00815E003";
    let notams = vec![
        notam("W0010/26", q, "EDGG", "2606010600", "2606022000", "EXPIRED"),
        notam("W0011/26", q, "EDGG", "2607010600", "2607022000", "NOT YET"),
        notam("W0012/26", q, "EDGG", "2606150600", "2606152000", "CURRENT"),
    ];
    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(ids(&relevant), vec!["W0012/26"]);
}

#[test]
fn estimated_ends_count_as_the_working_end_and_perm_never_expires() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let q = "EDGG/QWPLW/IV/M/W/000/050/5000N00815E003";
    let notams = vec![
        // Estimate passed before the briefing window: dropped.
        notam(
            "W0020/26",
            q,
            "EDGG",
            "2606010600",
            "2606140000EST",
            "EST PASSED",
        ),
        // Estimate inside the window: kept.
        notam(
            "W0021/26",
            q,
            "EDGG",
            "2606140600",
            "2606151200EST",
            "EST CURRENT",
        ),
        // Permanent: kept forever.
        notam("W0022/26", q, "EDGG", "2606010600", "PERM", "PERMANENT"),
    ];
    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(ids(&relevant), vec!["W0022/26", "W0021/26"]);
}

#[test]
fn bands_above_the_flight_are_dropped_and_edges_are_inclusive() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let notams = vec![
        // FL200–FL300, flight band GND–5000 ft: dropped.
        notam(
            "W0030/26",
            "EDGG/QRRCA/IV/BO/W/200/300/5000N00815E005",
            "EDGG",
            "2606150600",
            "2606152000",
            "HIGH ED-R",
        ),
        // FL050 floor == 5000 ft ceiling: inclusive, kept.
        notam(
            "W0031/26",
            "EDGG/QRRCA/IV/BO/W/050/100/5000N00815E005",
            "EDGG",
            "2606150600",
            "2606152000",
            "EDGE ED-R",
        ),
        // GND floor always overlaps.
        notam(
            "W0032/26",
            "EDGG/QRRCA/IV/BO/W/000/100/5000N00815E005",
            "EDGG",
            "2606150600",
            "2606152000",
            "LOW ED-R",
        ),
    ];
    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(ids(&relevant), vec!["W0031/26", "W0032/26"]);

    // Without a band (uncomputed flight) nothing is filtered vertically.
    let mut input = synthetic_input(&notams, &route, &corridor);
    input.altitude_band = None;
    assert_eq!(relevant_notams(&input).len(), 3);
}

#[test]
fn inactive_during_flight_is_listed_but_flagged() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    // Valid tonight 18:00–22:00; the flight lands 10:00.
    let notams = vec![notam(
        "W0040/26",
        "EDGG/QWPLW/IV/M/W/000/050/5000N00815E003",
        "EDGG",
        "2606151800",
        "2606152200",
        "EVENING PJE",
    )];
    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(relevant.len(), 1);
    assert!(!relevant[0].active_during_flight);
}

// --- supersedure + administrative NOTAMs ---------------------------------------

#[test]
fn cancelled_and_replaced_notams_collapse() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let q = "EDGG/QMXLC/IV/M/A/000/999/5000N00800E005";
    let cancelled = notam(
        "A0100/26",
        q,
        "AAAA",
        "2606100600",
        "2606302000",
        "OLD WORK",
    );
    let cancellation = Notam::parse(&format!(
        "A0101/26 NOTAMC A0100/26\nQ) {q}\nA) AAAA B) 2606140600\nE) WORK COMPLETED"
    ))
    .expect("cancellation parses");
    let replaced = notam(
        "A0102/26",
        q,
        "AAAA",
        "2606100600",
        "2606302000",
        "OLD TEXT",
    );
    let replacement = Notam::parse(&format!(
        "A0103/26 NOTAMR A0102/26\nQ) {q}\nA) AAAA B) 2606140600 C) 2606302000\nE) NEW TEXT"
    ))
    .expect("replacement parses");
    let notams = vec![cancelled, cancellation, replaced, replacement];

    let relevant = relevant_notams(&synthetic_input(&notams, &route, &corridor));
    assert_eq!(
        ids(&relevant),
        vec!["A0103/26"],
        "only the replacement survives: target + NOTAMC + replaced all drop"
    );
}

#[test]
fn checklist_notams_never_brief() {
    let points = straight_track();
    let route = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let notams = vec![notam(
        "A0110/26",
        "EDGG/QKKKK/K/K/K/000/999/5000N00815E999",
        "AAAA",
        "2606010000",
        "2607010000",
        "CHECKLIST YEAR 2026",
    )];
    assert!(relevant_notams(&synthetic_input(&notams, &route, &corridor)).is_empty());
}

// --- ordering -------------------------------------------------------------------

#[test]
fn briefing_order_is_aerodromes_then_corridor_then_fir() {
    let points = vec![
        airport("AAAA", 50.0, 8.0),
        free(50.0, 8.25),
        airport("BBBB", 50.0, 8.5),
    ];
    let route_wps = route(&points);
    let corridor = corridor(&points, 1_000.0, 5.0);
    let alternates = vec![airport("CCCC", 50.2, 8.25)];
    let b = "2606150600";
    let c = "2606152000";
    let notams = vec![
        // Input deliberately shuffled.
        notam(
            "E0001/26",
            "EDGG/QGWAU/IV/NBO/E/000/999/5000N00815E999",
            "EDGG",
            b,
            c,
            "FIR WIDE",
        ),
        // Corridor at ~28.6 km along (8°24'E), on the centerline.
        notam(
            "W0051/26",
            "EDGG/QWPLW/IV/M/W/000/050/5000N00824E002",
            "EDGG",
            b,
            c,
            "LATE PJE",
        ),
        // Alternate.
        notam(
            "C0001/26",
            "EDGG/QFAXX/IV/BO/A/000/999/5012N00815E005",
            "CCCC",
            b,
            c,
            "ALTN BIRDS",
        ),
        // Destination — its circle covers the track, but the aerodrome
        // class wins over corridor geometry.
        notam(
            "B0001/26",
            "EDGG/QMRLC/IV/NBO/A/000/999/5000N00830E005",
            "BBBB",
            b,
            c,
            "DEST RWY",
        ),
        // Corridor at ~7.1 km along (8°06'E).
        notam(
            "W0050/26",
            "EDGG/QWPLW/IV/M/W/000/050/5000N00806E002",
            "EDGG",
            b,
            c,
            "EARLY PJE",
        ),
        // Departure.
        notam(
            "A0001/26",
            "EDGG/QMXLC/IV/M/A/000/999/5000N00800E005",
            "AAAA",
            b,
            c,
            "DEP TWY",
        ),
    ];
    let mut input = synthetic_input(&notams, &route_wps, &corridor);
    input.alternates = &alternates;

    let relevant = relevant_notams(&input);
    assert_eq!(
        ids(&relevant),
        vec![
            "A0001/26", // departure aerodrome
            "B0001/26", // destination aerodrome
            "C0001/26", // alternate aerodrome
            "W0050/26", // corridor, entering first
            "W0051/26", // corridor, entering later
            "E0001/26", // FIR
        ]
    );
    assert_eq!(
        relevant[2].relevance,
        NotamRelevance::Aerodrome(IcaoCode::new("CCCC").expect("valid"))
    );
    assert!(matches!(relevant[5].relevance, NotamRelevance::Fir));
}

// --- helpers --------------------------------------------------------------------

#[test]
fn altitude_band_spans_the_phase_plan() {
    let segment = |start_ft: f64, end_ft: f64, kind: PhaseKind| PhaseSegment {
        kind,
        start_along_track: Meters(0.0),
        end_along_track: Meters(10_000.0),
        start_altitude: MetersAmsl::from_feet(start_ft),
        end_altitude: MetersAmsl::from_feet(end_ft),
        tas: Knots(100.0),
        duration: Minutes(5.0),
        fuel: Liters(2.0),
    };
    let phases = PhasePlan {
        segments: vec![
            segment(350.0, 4500.0, PhaseKind::Climb),
            segment(4500.0, 4500.0, PhaseKind::Cruise),
            segment(4500.0, 600.0, PhaseKind::Descent),
        ],
        toc: None,
        tod: None,
        total_duration: Minutes(15.0),
        total_fuel: Liters(6.0),
    };
    let band = AltitudeBand::from_phases(&phases).expect("non-empty plan");
    assert!((band.floor.as_feet() - 350.0).abs() < 1e-9);
    assert!((band.ceiling.as_feet() - 4500.0).abs() < 1e-9);

    let empty = PhasePlan {
        segments: Vec::new(),
        toc: None,
        tod: None,
        total_duration: Minutes(0.0),
        total_fuel: Liters(0.0),
    };
    assert_eq!(AltitudeBand::from_phases(&empty), None);
}

#[test]
fn restriction_activation_requires_group_r_and_an_activation_condition() {
    let b = "2606150600";
    let c = "2606152000";
    let case = |q: &str| is_restriction_activation(&notam("D0001/26", q, "EDGG", b, c, "X"));
    // ED-R activated / danger area will take place: yes.
    assert!(case("EDMM/QRRCA/IV/BO/W/000/100/4942N01156E010"));
    assert!(case("EDMM/QRDLW/IV/BO/W/000/100/4942N01156E010"));
    // Restricted area *deactivated*: no.
    assert!(!case("EDMM/QRRCD/IV/BO/W/000/100/4942N01156E010"));
    // Activation outside group R (a CTR): no.
    assert!(!case("EDMM/QACCA/IV/BO/AE/000/100/4942N01156E010"));
    // Navigation warning: no (amber material, not the red class).
    assert!(!case("EDMM/QWPLW/IV/M/W/000/130/4815N01133E003"));
}

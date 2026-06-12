//! Conflict-engine tests: synthetic corridors and profiles with worked
//! values in comments (plan §7 — datum traps, threshold boundaries).

use chrono::{TimeZone as _, Utc};
use strata_data::domain::{
    Airspace, AirspaceClass, AirspaceKind, LatLon, Meters, MetersAgl, MetersAmsl, Notam, NotamKind,
    Obstacle, ObstacleKind, Polygon, VerticalLimit,
};

use crate::corridor::{AirspaceCrossing, Corridor, CorridorParams, CorridorSample, Station};
use crate::fuel::FuelLadder;
use crate::perf::{PhaseKind, PhasePlan, PhaseSegment};
use crate::route::great_circle_distance;
use crate::units::{Kilograms, Knots, Liters, Minutes};
use crate::wb::{WbReport, WbState, WbStateKind};

use super::*;

// ── shared builders ────────────────────────────────────────────────────

fn segment(kind: PhaseKind, start: f64, end: f64, alt0: f64, alt1: f64) -> PhaseSegment {
    PhaseSegment {
        kind,
        start_along_track: Meters(start),
        end_along_track: Meters(end),
        start_altitude: MetersAmsl(alt0),
        end_altitude: MetersAmsl(alt1),
        tas: Knots(100.0),
        duration: Minutes(10.0),
        fuel: Liters(4.0),
    }
}

/// Climb 0→`alt` over `[0, climb]`, cruise, descent back to 0 over the
/// last `descent` meters.
pub(crate) fn climb_cruise_descent_plan(
    total: f64,
    climb: f64,
    descent: f64,
    alt: f64,
) -> PhasePlan {
    PhasePlan {
        segments: vec![
            segment(PhaseKind::Climb, 0.0, climb, 0.0, alt),
            segment(PhaseKind::Cruise, climb, total - descent, alt, alt),
            segment(PhaseKind::Descent, total - descent, total, alt, 0.0),
        ],
        toc: None,
        tod: None,
        total_duration: Minutes(30.0),
        total_fuel: Liters(12.0),
    }
}

/// A single cruise segment at `alt` — no clearance ramp anywhere.
pub(crate) fn cruise_only_plan(total: f64, alt: f64) -> PhasePlan {
    PhasePlan {
        segments: vec![segment(PhaseKind::Cruise, 0.0, total, alt, alt)],
        toc: None,
        tod: None,
        total_duration: Minutes(20.0),
        total_fuel: Liters(8.0),
    }
}

fn empty_plan() -> PhasePlan {
    PhasePlan {
        segments: Vec::new(),
        toc: None,
        tod: None,
        total_duration: Minutes(0.0),
        total_fuel: Liters(0.0),
    }
}

fn sample(index: usize, along: f64, terrain: Option<f64>) -> CorridorSample {
    CorridorSample {
        station: Station {
            index,
            leg_index: 0,
            along_track: Meters(along),
            position: LatLon::new(50.0, 8.0 + index as f64 * 0.001).expect("valid coords"),
        },
        max_terrain: terrain.map(MetersAmsl),
        min_terrain: terrain.map(MetersAmsl),
        tallest_obstacle: None,
    }
}

/// Stations every `spacing` m with per-station terrain from `terrain(i)`.
fn corridor_with_terrain(
    n: usize,
    spacing: f64,
    terrain: impl Fn(usize) -> Option<f64>,
) -> Corridor {
    Corridor {
        params: CorridorParams::default(),
        samples: (0..n)
            .map(|i| sample(i, i as f64 * spacing, terrain(i)))
            .collect(),
        crossings: Vec::new(),
    }
}

fn triangle() -> Polygon {
    Polygon::new(
        vec![
            LatLon::new(49.9, 7.9).expect("valid"),
            LatLon::new(50.1, 8.1).expect("valid"),
            LatLon::new(49.9, 8.3).expect("valid"),
        ],
        vec![],
    )
    .expect("valid polygon")
}

fn airspace(
    name: &str,
    class: AirspaceClass,
    kind: AirspaceKind,
    lower: VerticalLimit,
    upper: VerticalLimit,
) -> Airspace {
    Airspace {
        name: name.to_owned(),
        class,
        kind,
        lower,
        upper,
        geometry: triangle(),
        airac: None,
    }
}

fn crossing(airspace: Airspace, entry: f64, exit: f64) -> AirspaceCrossing {
    AirspaceCrossing {
        airspace,
        entry_along_track: Meters(entry),
        exit_along_track: Meters(exit),
    }
}

fn ok_wb() -> WbReport {
    WbReport {
        states: vec![
            wb_state(WbStateKind::Ramp, true),
            wb_state(WbStateKind::Takeoff, true),
            wb_state(WbStateKind::ZeroFuel, true),
            wb_state(WbStateKind::Landing, true),
        ],
        burn_track: Vec::new(),
    }
}

fn wb_state(kind: WbStateKind, within: bool) -> WbState {
    WbState {
        kind,
        mass: Kilograms(1043.0),
        arm: Meters(1.1),
        within_envelope: within,
    }
}

fn fuel_with_margin(margin: f64) -> FuelLadder {
    FuelLadder {
        taxi: Liters(1.0),
        trip: Liters(20.0),
        contingency: Liters(1.0),
        alternate: Liters(0.0),
        final_reserve: Liters(13.0),
        extra: Liters(0.0),
        minimum_required: Liters(35.0),
        loaded: Liters(35.0 + margin),
        margin: Liters(margin),
    }
}

fn detect(
    corridor: &Corridor,
    phases: &PhasePlan,
    thresholds: &ConflictThresholds,
) -> Vec<Conflict> {
    detect_conflicts(
        corridor,
        phases,
        &ok_wb(),
        &fuel_with_margin(5.0),
        thresholds,
    )
    .expect("consistent inputs")
}

fn thresholds_m(terrain: f64, obstacle: f64) -> ConflictThresholds {
    ConflictThresholds {
        terrain_clearance: MetersAgl(terrain),
        obstacle_clearance: MetersAgl(obstacle),
        ..ConflictThresholds::default()
    }
}

// ── frozen-type sanity (kept from the skeleton) ────────────────────────

#[test]
fn severity_orders_for_badge_aggregation() {
    assert!(ConflictSeverity::Info < ConflictSeverity::Caution);
    assert!(ConflictSeverity::Caution < ConflictSeverity::Warning);
}

#[test]
fn default_thresholds_are_the_documented_buffers() {
    let t = ConflictThresholds::default();
    assert!((t.terrain_clearance.as_feet() - 1000.0).abs() < 1e-9);
    assert!((t.obstacle_clearance.as_feet() - 1000.0).abs() < 1e-9);
    assert_eq!(t.min_runway_margin_ratio, 1.0);
}

#[test]
fn conflict_serializes() {
    let conflict = Conflict {
        kind: ConflictKind::Notam,
        severity: ConflictSeverity::Warning,
        location: ConflictLocation::Leg { index: 2 },
        message: "test".to_owned(),
    };
    let json = serde_json::to_string(&conflict).unwrap();
    assert!(json.contains("\"notam\""));
    let back: Conflict = serde_json::from_str(&json).unwrap();
    assert_eq!(conflict, back);
}

// ── terrain / obstacle clearance ───────────────────────────────────────

#[test]
fn terrain_clearance_boundary_is_strict() {
    // Cruise at 700 m, buffer 300 m. Terrain 400 m ⇒ clearance exactly
    // 300 m ⇒ NOT a conflict; terrain 400.5 m ⇒ 299.5 m ⇒ conflict.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = cruise_only_plan(10_000.0, 700.0);

    let at_buffer = corridor_with_terrain(11, 1000.0, |_| Some(400.0));
    assert!(detect(&at_buffer, &phases, &thresholds).is_empty());

    let under_buffer = corridor_with_terrain(11, 1000.0, |_| Some(400.5));
    let conflicts = detect(&under_buffer, &phases, &thresholds);
    assert_eq!(conflicts.len(), 1, "one merged conflict for the whole run");
    assert_eq!(conflicts[0].kind, ConflictKind::Terrain);
    assert_eq!(conflicts[0].severity, ConflictSeverity::Warning);
}

#[test]
fn contiguous_violations_merge_and_anchor_at_the_worst_station() {
    // Buffer 300 m, cruise 700 m. Terrain: 600 m at stations 3–5 (worst
    // 650 at 4), 500 m at station 8 — two separate runs ⇒ two conflicts.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = cruise_only_plan(10_000.0, 700.0);
    let corridor = corridor_with_terrain(11, 1000.0, |i| match i {
        3 | 5 => Some(600.0),
        4 => Some(650.0),
        8 => Some(500.0),
        _ => Some(100.0),
    });
    let conflicts = detect(&corridor, &phases, &thresholds);
    assert_eq!(conflicts.len(), 2);
    let ConflictLocation::Station { along_track, .. } = conflicts[0].location else {
        panic!("terrain conflicts anchor at stations");
    };
    // Worst of the first run is station 4 (clearance 50 m).
    assert_eq!(along_track, Meters(4000.0));
    let ConflictLocation::Station { along_track, .. } = conflicts[1].location else {
        panic!("terrain conflicts anchor at stations");
    };
    assert_eq!(along_track, Meters(8000.0));
}

#[test]
fn below_terrain_reads_differently_from_low_clearance() {
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = cruise_only_plan(2000.0, 700.0);
    let corridor = corridor_with_terrain(3, 1000.0, |_| Some(800.0)); // 100 m above us
    let conflicts = detect(&corridor, &phases, &thresholds);
    assert_eq!(conflicts.len(), 1);
    assert!(
        conflicts[0].message.contains("above the planned altitude"),
        "got: {}",
        conflicts[0].message
    );
}

#[test]
fn clearance_buffer_ramps_through_the_initial_climb() {
    // Climb 0→300 m over 10 km then cruise to 20 km; buffer 300 m. Over
    // flat 0 m terrain the planned line climbs at exactly the ramp rate:
    // planned(x) = 300·x/10000 = required(x) ⇒ clean, no conflicts.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = PhasePlan {
        segments: vec![
            segment(PhaseKind::Climb, 0.0, 10_000.0, 0.0, 300.0),
            segment(PhaseKind::Cruise, 10_000.0, 20_000.0, 300.0, 300.0),
        ],
        toc: None,
        tod: None,
        total_duration: Minutes(20.0),
        total_fuel: Liters(8.0),
    };
    let flat = corridor_with_terrain(21, 1000.0, |_| Some(0.0));
    assert!(detect(&flat, &phases, &thresholds).is_empty());

    // A 200 m ridge at 5 km: planned(5 km) = 150 m is *below* the ridge
    // — flagged even at the halved buffer.
    let ridge = corridor_with_terrain(21, 1000.0, |i| Some(if i == 5 { 200.0 } else { 0.0 }));
    let conflicts = detect(&ridge, &phases, &thresholds);
    assert_eq!(conflicts.len(), 1);
    let ConflictLocation::Station { along_track, .. } = conflicts[0].location else {
        panic!("anchored at the ridge");
    };
    assert_eq!(along_track, Meters(5000.0));
}

#[test]
fn obstacle_clearance_boundary() {
    // Cruise 700 m, obstacle buffer 300 m: top at 400 m ⇒ exactly at the
    // buffer ⇒ no conflict; top at 450 m ⇒ 250 m clearance ⇒ conflict.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = cruise_only_plan(4000.0, 700.0);
    let obstacle = |top: f64| Obstacle {
        name: Some("Mast Mitte".to_owned()),
        kind: ObstacleKind::Mast,
        position: LatLon::new(50.0, 8.002).expect("valid"),
        height: MetersAgl(top - 100.0),
        elevation_top: MetersAmsl(top),
        lighted: true,
    };
    let mut corridor = corridor_with_terrain(5, 1000.0, |_| Some(100.0));
    corridor.samples[2].tallest_obstacle = Some(obstacle(400.0));
    assert!(detect(&corridor, &phases, &thresholds).is_empty());

    corridor.samples[2].tallest_obstacle = Some(obstacle(450.0));
    let conflicts = detect(&corridor, &phases, &thresholds);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, ConflictKind::Obstacle);
    assert!(
        conflicts[0].message.contains("mast"),
        "got: {}",
        conflicts[0].message
    );
}

#[test]
fn endpoint_grace_exempts_the_climb_out_and_letdown_mile() {
    // Climb 0→600 m over 10 km, cruise to 20 km, descent over the last
    // 10 km; buffer 300 m, default grace 1 NM (1852 m). A 500 m knoll at
    // 1 km sits inside the grace *and* the climb ⇒ exempt; the same knoll
    // at 5 km (planned 300 m — below it) still conflicts.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = climb_cruise_descent_plan(30_000.0, 10_000.0, 10_000.0, 600.0);
    let knoll = |at: usize| {
        corridor_with_terrain(31, 1000.0, move |i| Some(if i == at { 500.0 } else { 0.0 }))
    };

    assert!(
        detect(&knoll(1), &phases, &thresholds).is_empty(),
        "knoll inside the departure grace is exempt"
    );
    let conflicts = detect(&knoll(5), &phases, &thresholds);
    assert_eq!(
        conflicts.len(),
        1,
        "the same knoll beyond the grace conflicts"
    );
    assert_eq!(conflicts[0].kind, ConflictKind::Terrain);

    // Mirror at the destination: 1 km before the route end, inside the
    // final descent.
    assert!(
        detect(&knoll(29), &phases, &thresholds).is_empty(),
        "knoll inside the arrival grace is exempt"
    );

    // Grace 0 disables the exemption entirely.
    let mut no_grace = thresholds;
    no_grace.endpoint_grace_distance = Meters(0.0);
    assert_eq!(detect(&knoll(1), &phases, &no_grace).len(), 1);
    assert_eq!(detect(&knoll(29), &phases, &no_grace).len(), 1);
}

#[test]
fn endpoint_grace_applies_only_while_climbing_or_descending() {
    // Cruise-only profile at 300 m: a 500 m knoll 1 km in is *not* a
    // climb-out — the grace never engages and the conflict stands.
    let thresholds = thresholds_m(300.0, 300.0);
    let phases = cruise_only_plan(30_000.0, 300.0);
    let corridor = corridor_with_terrain(31, 1000.0, |i| Some(if i == 1 { 500.0 } else { 0.0 }));
    let conflicts = detect(&corridor, &phases, &thresholds);
    assert_eq!(conflicts.len(), 1);
    assert!(
        conflicts[0].message.contains("above the planned altitude"),
        "got: {}",
        conflicts[0].message
    );
}

// ── airspace penetration ───────────────────────────────────────────────

#[test]
fn agl_floor_follows_sloping_terrain() {
    // Terrain rises 100 + 25·i m at stations i = 0..=20 (spacing 1 km).
    // Volume over [5 km, 15 km] with floor 300 m AGL, ceiling 5000 m MSL.
    // Cruise at 700 m MSL ⇒ inside where terrain + 300 ≤ 700, i.e.
    // terrain ≤ 400 m ⇒ i ≤ 12. Within the crossing: stations 5..=12.
    let corridor = Corridor {
        crossings: vec![crossing(
            airspace(
                "ED-R SLOPE",
                AirspaceClass::Unclassified,
                AirspaceKind::Restricted,
                VerticalLimit::agl(MetersAgl(300.0)),
                VerticalLimit::amsl(MetersAmsl(5000.0)),
            ),
            5000.0,
            15_000.0,
        )],
        ..corridor_with_terrain(21, 1000.0, |i| Some(100.0 + 25.0 * i as f64))
    };
    let phases = cruise_only_plan(20_000.0, 700.0);

    let stations = airspace::penetrating_stations(&corridor.crossings[0], &corridor, &phases);
    assert_eq!(stations, (5..=12).collect::<Vec<_>>());

    // Keep the terrain checker quiet (max terrain 600 m, clearance 100 m).
    let conflicts = detect(&corridor, &phases, &thresholds_m(50.0, 50.0));
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, ConflictKind::Airspace);
    assert_eq!(
        conflicts[0].severity,
        ConflictSeverity::Warning,
        "ED-R is always red"
    );
    let ConflictLocation::Station { along_track, .. } = conflicts[0].location else {
        panic!("airspace conflicts anchor at the first penetrating station");
    };
    assert_eq!(along_track, Meters(5000.0));
    assert!(conflicts[0].message.contains("ED-R SLOPE"));
    // 300 m AGL renders chart-style as feet (984 ft AGL).
    assert!(
        conflicts[0].message.contains("984 ft AGL"),
        "got: {}",
        conflicts[0].message
    );
}

#[test]
fn agl_floor_rides_the_lowest_corridor_terrain() {
    // A 600 m ridge abeam must not lift the floor of a 1000 ft AGL volume
    // above the 300 m valley under the track: floor = min terrain +
    // 304.8 m = 604.8 m AMSL. Cruise 800 m is genuinely inside; the old
    // max-terrain floor (904.8 m) reported no penetration — a missed
    // Warning in the dangerous direction.
    let mut corridor = Corridor {
        crossings: vec![crossing(
            airspace(
                "ED-R VALLEY",
                AirspaceClass::Unclassified,
                AirspaceKind::Restricted,
                VerticalLimit::agl(MetersAgl::from_feet(1000.0)),
                VerticalLimit::amsl(MetersAmsl(5000.0)),
            ),
            0.0,
            10_000.0,
        )],
        ..corridor_with_terrain(11, 1000.0, |_| Some(600.0))
    };
    for sample in &mut corridor.samples {
        sample.min_terrain = Some(MetersAmsl(300.0));
    }

    let phases = cruise_only_plan(10_000.0, 800.0);
    let stations = airspace::penetrating_stations(&corridor.crossings[0], &corridor, &phases);
    assert_eq!(
        stations,
        (0..=10).collect::<Vec<_>>(),
        "floor 604.8 m ≤ cruise 800 m at every station"
    );

    // Guard: below the lowest plausible floor is still clean.
    let phases = cruise_only_plan(10_000.0, 600.0);
    let stations = airspace::penetrating_stations(&corridor.crossings[0], &corridor, &phases);
    assert!(stations.is_empty(), "cruise 600 m sits below floor 604.8 m");
}

#[test]
fn penetration_altitude_overlap_matrix() {
    // Band 1000–2000 m MSL; limits are inclusive on both sides.
    let cases: &[(f64, bool)] = &[
        (999.0, false),  // below floor
        (1000.0, true),  // exactly at floor
        (1500.0, true),  // inside
        (2000.0, true),  // exactly at ceiling
        (2001.0, false), // above ceiling
    ];
    for &(alt, expected) in cases {
        let corridor = Corridor {
            crossings: vec![crossing(
                airspace(
                    "CTR MATRIX",
                    AirspaceClass::D,
                    AirspaceKind::Ctr,
                    VerticalLimit::amsl(MetersAmsl(1000.0)),
                    VerticalLimit::amsl(MetersAmsl(2000.0)),
                ),
                0.0,
                4000.0,
            )],
            ..corridor_with_terrain(5, 1000.0, |_| Some(0.0))
        };
        let phases = cruise_only_plan(4000.0, alt);
        let conflicts = detect(&corridor, &phases, &thresholds_m(1.0, 1.0));
        assert_eq!(
            conflicts.len(),
            usize::from(expected),
            "altitude {alt} m vs band 1000–2000 m"
        );
    }
}

#[test]
fn fl_and_gnd_and_unl_limits_normalize() {
    // FL 50 floor = 5000 ft = 1524.0 m exactly. GND floor / UNL ceiling
    // are unbounded.
    let fl_floor = |alt: f64| {
        let corridor = Corridor {
            crossings: vec![crossing(
                airspace(
                    "TMA FL",
                    AirspaceClass::C,
                    AirspaceKind::Tma,
                    VerticalLimit::fl(50),
                    VerticalLimit::unl(),
                ),
                0.0,
                2000.0,
            )],
            ..corridor_with_terrain(3, 1000.0, |_| Some(0.0))
        };
        detect(
            &corridor,
            &cruise_only_plan(2000.0, alt),
            &thresholds_m(1.0, 1.0),
        )
        .len()
    };
    assert_eq!(fl_floor(1523.9), 0, "just below FL 50");
    // Exactly at the floor: derive the altitude through the same ft→m
    // conversion the engine uses (1524 m up to the last ULP).
    assert_eq!(
        fl_floor(MetersAmsl::from_feet(5000.0).0),
        1,
        "at FL 50 (inclusive)"
    );
    assert_eq!(fl_floor(9000.0), 1, "UNL ceiling is unbounded");

    let gnd = Corridor {
        crossings: vec![crossing(
            airspace(
                "EDDF CTR",
                AirspaceClass::D,
                AirspaceKind::Ctr,
                VerticalLimit::gnd(),
                VerticalLimit::amsl(MetersAmsl(1500.0)),
            ),
            0.0,
            2000.0,
        )],
        ..corridor_with_terrain(3, 1000.0, |_| Some(0.0))
    };
    let conflicts = detect(
        &gnd,
        &cruise_only_plan(2000.0, 300.0),
        &thresholds_m(1.0, 1.0),
    );
    assert_eq!(
        conflicts.len(),
        1,
        "GND floor catches any altitude below the ceiling"
    );
    assert!(
        conflicts[0].message.contains("floor GND"),
        "got: {}",
        conflicts[0].message
    );
}

#[test]
fn severity_table_matches_the_design() {
    use AirspaceClass as C;
    use AirspaceKind as K;
    let sev = |class, kind| {
        airspace::airspace_severity(&airspace(
            "X",
            class,
            kind,
            VerticalLimit::gnd(),
            VerticalLimit::unl(),
        ))
    };
    // ED-R/D/P always red.
    assert_eq!(
        sev(C::Unclassified, K::Restricted),
        Some(ConflictSeverity::Warning)
    );
    assert_eq!(
        sev(C::Unclassified, K::Danger),
        Some(ConflictSeverity::Warning)
    );
    assert_eq!(
        sev(C::Unclassified, K::Prohibited),
        Some(ConflictSeverity::Warning)
    );
    // TMZ/RMZ amber-informational.
    assert_eq!(
        sev(C::Unclassified, K::Tmz),
        Some(ConflictSeverity::Caution)
    );
    assert_eq!(
        sev(C::Unclassified, K::Rmz),
        Some(ConflictSeverity::Caution)
    );
    // Clearance-bound airspace.
    assert_eq!(sev(C::D, K::Ctr), Some(ConflictSeverity::Caution));
    assert_eq!(sev(C::C, K::Tma), Some(ConflictSeverity::Caution));
    // Class A: VFR prohibited.
    assert_eq!(sev(C::A, K::Cta), Some(ConflictSeverity::Warning));
    // Pattern/activity areas inform.
    assert_eq!(sev(C::Unclassified, K::Atz), Some(ConflictSeverity::Info));
    assert_eq!(
        sev(C::Unclassified, K::ParachuteJumpArea),
        Some(ConflictSeverity::Info)
    );
    // Legal-to-enter VFR airspace stays quiet.
    assert_eq!(sev(C::E, K::Tma), None);
    assert_eq!(sev(C::G, K::Area), None);
    assert_eq!(sev(C::Unclassified, K::FisSector), None);
}

#[test]
fn crossing_without_vertical_overlap_is_no_conflict() {
    // ED-R from 2000 m up; we cruise at 700 m — crossing laterally but
    // safely below ⇒ no conflict.
    let corridor = Corridor {
        crossings: vec![crossing(
            airspace(
                "ED-R HIGH",
                AirspaceClass::Unclassified,
                AirspaceKind::Restricted,
                VerticalLimit::amsl(MetersAmsl(2000.0)),
                VerticalLimit::unl(),
            ),
            0.0,
            4000.0,
        )],
        ..corridor_with_terrain(5, 1000.0, |_| Some(0.0))
    };
    let conflicts = detect(
        &corridor,
        &cruise_only_plan(4000.0, 700.0),
        &thresholds_m(1.0, 1.0),
    );
    assert!(conflicts.is_empty());
}

// ── folded-in states ───────────────────────────────────────────────────

#[test]
fn out_of_envelope_states_fold_into_warnings() {
    let mut wb = ok_wb();
    wb.states[1].within_envelope = false; // takeoff
    let conflicts = detect_conflicts(
        &corridor_with_terrain(0, 1000.0, |_| None),
        &empty_plan(),
        &wb,
        &fuel_with_margin(5.0),
        &ConflictThresholds::default(),
    )
    .expect("consistent");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, ConflictKind::WeightBalance);
    assert_eq!(conflicts[0].severity, ConflictSeverity::Warning);
    assert_eq!(conflicts[0].location, ConflictLocation::Flight);
    assert!(conflicts[0].message.contains("takeoff"));
}

#[test]
fn fuel_margin_boundary() {
    let detect_fuel = |margin: f64| {
        detect_conflicts(
            &corridor_with_terrain(0, 1000.0, |_| None),
            &empty_plan(),
            &ok_wb(),
            &fuel_with_margin(margin),
            &ConflictThresholds::default(),
        )
        .expect("consistent")
    };
    assert!(
        detect_fuel(0.0).is_empty(),
        "exactly at minimum is not a conflict"
    );
    let conflicts = detect_fuel(-2.5);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, ConflictKind::Fuel);
    assert!(
        conflicts[0].message.contains("2.5 L under"),
        "got: {}",
        conflicts[0].message
    );
}

#[test]
fn runway_margin_threshold() {
    let thresholds = ConflictThresholds::default(); // ratio 1.0
    // 600 m required, 600 m available ⇒ ratio 1.0 ⇒ fine.
    assert!(runway_margin_conflict("EDFE 08", Meters(600.0), Meters(600.0), &thresholds).is_none());
    // 550 m available ⇒ ratio 0.92 ⇒ Warning.
    let conflict = runway_margin_conflict("EDFE 08", Meters(600.0), Meters(550.0), &thresholds)
        .expect("under margin");
    assert_eq!(conflict.kind, ConflictKind::RunwayDistance);
    assert_eq!(conflict.severity, ConflictSeverity::Warning);
    assert!(conflict.message.contains("EDFE 08"));
    // No data ⇒ no conflict.
    assert!(runway_margin_conflict("EDFE 08", Meters(0.0), Meters(600.0), &thresholds).is_none());
}

#[test]
fn sampled_corridor_without_phases_is_inconsistent() {
    let result = detect_conflicts(
        &corridor_with_terrain(3, 1000.0, |_| Some(0.0)),
        &empty_plan(),
        &ok_wb(),
        &fuel_with_margin(5.0),
        &ConflictThresholds::default(),
    );
    assert!(matches!(result, Err(ConflictError::IncompleteInput(_))));
}

// ── NOTAM areas ────────────────────────────────────────────────────────

/// Corridor along the 50°N parallel from 8.0°E to 8.3°E (stations every
/// 0.02°, ~1.43 km), with along-track from real great-circle distances.
fn parallel_corridor() -> (Corridor, PhasePlan) {
    let positions: Vec<LatLon> = (0..=15)
        .map(|i| LatLon::new(50.0, 8.0 + 0.02 * i as f64).expect("valid"))
        .collect();
    let mut along = 0.0;
    let mut samples = Vec::new();
    for (i, &p) in positions.iter().enumerate() {
        if i > 0 {
            along += great_circle_distance(positions[i - 1], p).0;
        }
        samples.push(CorridorSample {
            station: Station {
                index: i,
                leg_index: 0,
                along_track: Meters(along),
                position: p,
            },
            max_terrain: Some(MetersAmsl(100.0)),
            min_terrain: Some(MetersAmsl(100.0)),
            tallest_obstacle: None,
        });
    }
    let phases = cruise_only_plan(along, 700.0); // ≈ 2300 ft
    (
        Corridor {
            params: CorridorParams::default(),
            samples,
            crossings: Vec::new(),
        },
        phases,
    )
}

/// ED-R activation 3 NM around 50°00'N 008°10'E (on the corridor),
/// SFC–FL100, active 14 Jun 2026 10:00–16:00 UTC.
const EDR_ACTIVATION: &str = "D0123/26 NOTAMN\n\
    Q) EDGG/QRRCA/IV/BO/W/000/100/5000N00810E003\n\
    A) EDGG B) 2606141000 C) 2606141600\n\
    E) ED-R 123 WERTHEIM ACTIVE";

fn window(h_from: u32, h_to: u32) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    (
        Utc.with_ymd_and_hms(2026, 6, 14, h_from, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 6, 14, h_to, 0, 0).unwrap(),
    )
}

#[test]
fn active_edr_notam_in_corridor_is_red() {
    let (corridor, phases) = parallel_corridor();
    let notam = Notam::parse(EDR_ACTIVATION).expect("fixture parses");
    let (from, to) = window(11, 13);
    let conflicts = detect_notam_conflicts(&corridor, &phases, &[notam], from, to);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, ConflictKind::Notam);
    assert_eq!(
        conflicts[0].severity,
        ConflictSeverity::Warning,
        "R-group is red"
    );
    assert!(conflicts[0].message.contains("D0123/26"));
    assert!(
        conflicts[0].message.contains("ED-R 123 WERTHEIM"),
        "got: {}",
        conflicts[0].message
    );
    assert!(matches!(
        conflicts[0].location,
        ConflictLocation::Station { .. }
    ));
}

#[test]
fn notam_outside_the_flight_window_is_quiet() {
    let (corridor, phases) = parallel_corridor();
    let notam = Notam::parse(EDR_ACTIVATION).expect("fixture parses");
    let (from, to) = window(17, 19); // NOTAM ended 16:00
    assert!(detect_notam_conflicts(&corridor, &phases, &[notam], from, to).is_empty());
}

#[test]
fn notam_far_from_the_corridor_is_quiet() {
    let (corridor, phases) = parallel_corridor();
    // Same NOTAM, centre moved ~190 NM north-east.
    let far = EDR_ACTIVATION.replace("5000N00810E003", "5300N01200E003");
    let notam = Notam::parse(&far).expect("fixture parses");
    let (from, to) = window(11, 13);
    assert!(detect_notam_conflicts(&corridor, &phases, &[notam], from, to).is_empty());
}

#[test]
fn notam_vertically_above_the_flight_is_quiet() {
    let (corridor, phases) = parallel_corridor();
    // FL200–FL300 band: we cruise at 700 m ≈ 2300 ft, far below.
    let high = EDR_ACTIVATION.replace("/000/100/", "/200/300/");
    let notam = Notam::parse(&high).expect("fixture parses");
    let (from, to) = window(11, 13);
    assert!(detect_notam_conflicts(&corridor, &phases, &[notam], from, to).is_empty());
}

#[test]
fn fir_wide_radius_is_not_a_corridor_conflict() {
    let (corridor, phases) = parallel_corridor();
    let fir_wide = EDR_ACTIVATION.replace("5000N00810E003", "5000N00810E999");
    let notam = Notam::parse(&fir_wide).expect("fixture parses");
    let (from, to) = window(11, 13);
    assert!(detect_notam_conflicts(&corridor, &phases, &[notam], from, to).is_empty());
}

#[test]
fn aerodrome_notams_and_cancellations_are_not_area_conflicts() {
    let (corridor, phases) = parallel_corridor();
    let (from, to) = window(11, 13);

    // Runway closure (group M) on the corridor — briefing material, not a
    // corridor conflict.
    let rwy = "A1234/26 NOTAMN\n\
        Q) EDGG/QMRLC/IV/NBO/A/000/999/5000N00810E005\n\
        A) EDFE B) 2606140600 C) 2606171800\n\
        E) RWY 08/26 CLSD";
    let notam = Notam::parse(rwy).expect("fixture parses");
    assert!(detect_notam_conflicts(&corridor, &phases, &[notam], from, to).is_empty());

    // A cancellation never conflicts, whatever its Q-line says.
    let mut cancelled = Notam::parse(EDR_ACTIVATION).expect("fixture parses");
    cancelled.kind = NotamKind::Cancellation {
        cancels: cancelled.id,
    };
    assert!(detect_notam_conflicts(&corridor, &phases, &[cancelled], from, to).is_empty());
}

#[test]
fn navigation_warning_notams_are_amber() {
    let (corridor, phases) = parallel_corridor();
    // Parachute jumping (group W) over the corridor.
    let jumping = "B0456/26 NOTAMN\n\
        Q) EDGG/QWPLW/IV/M/W/000/100/5000N00814E002\n\
        A) EDGG B) 2606140800 C) 2606141800\n\
        E) PJE WILL TAKE PLACE WI 2NM RADIUS OF 5000N00814E";
    let notam = Notam::parse(jumping).expect("fixture parses");
    let (from, to) = window(11, 13);
    let conflicts = detect_notam_conflicts(&corridor, &phases, &[notam], from, to);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].severity, ConflictSeverity::Caution);
}

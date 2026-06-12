//! Profile-series tests: worked values for the band resolution (the datum
//! traps again, drawing flavor), the MSA aggregation and the
//! freezing-level estimates.

use strata_data::domain::{
    Airspace, AirspaceClass, AirspaceKind, LatLon, MetersAgl, Obstacle, ObstacleKind, Polygon,
    VerticalLimit,
};

use crate::corridor::{CorridorParams, Station};
use crate::flight::{FreePoint, RoutePoint};
use crate::sources::{Provenance, WindsAloft};
use crate::units::{DegreesTrue, Knots, Liters, Minutes};
use crate::wind::{LegWindOrigin, WindTriangle};

use super::*;

// ── builders ───────────────────────────────────────────────────────────

fn sample(index: usize, leg: usize, along: f64, terrain: Option<f64>) -> CorridorSample {
    CorridorSample {
        station: Station {
            index,
            leg_index: leg,
            along_track: Meters(along),
            position: LatLon::new(50.0, 8.0 + index as f64 * 0.001).expect("valid coords"),
        },
        max_terrain: terrain.map(MetersAmsl),
        min_terrain: terrain.map(MetersAmsl),
        tallest_obstacle: None,
    }
}

fn corridor(samples: Vec<CorridorSample>) -> Corridor {
    Corridor {
        params: CorridorParams::default(),
        samples,
        crossings: Vec::new(),
    }
}

fn airspace(lower: VerticalLimit, upper: VerticalLimit) -> Airspace {
    Airspace {
        name: "BAND".to_owned(),
        class: AirspaceClass::D,
        kind: AirspaceKind::Ctr,
        lower,
        upper,
        geometry: Polygon::new(
            vec![
                LatLon::new(49.9, 7.9).expect("valid"),
                LatLon::new(50.1, 8.1).expect("valid"),
                LatLon::new(49.9, 8.3).expect("valid"),
            ],
            vec![],
        )
        .expect("valid polygon"),
        airac: None,
    }
}

fn crossing(lower: VerticalLimit, upper: VerticalLimit, entry: f64, exit: f64) -> AirspaceCrossing {
    AirspaceCrossing {
        airspace: airspace(lower, upper),
        entry_along_track: Meters(entry),
        exit_along_track: Meters(exit),
    }
}

fn obstacle(top: f64) -> Obstacle {
    Obstacle {
        name: Some("Mast".to_owned()),
        kind: ObstacleKind::Mast,
        position: LatLon::new(50.0, 8.0).expect("valid"),
        height: MetersAgl(top - 100.0),
        elevation_top: MetersAmsl(top),
        lighted: true,
    }
}

fn leg_wind(leg_index: usize, temperature: f64) -> LegWind {
    LegWind {
        leg_index,
        wind: WindsAloft {
            direction: DegreesTrue::new(270.0),
            speed: Knots(10.0),
            temperature: Celsius(temperature),
            temperature_provenance: Provenance::Real,
        },
        origin: LegWindOrigin::Sampled,
        triangle: WindTriangle {
            wind_correction_angle_deg: 0.0,
            true_heading: DegreesTrue::new(0.0),
            ground_speed: Knots(100.0),
        },
    }
}

fn waypoint(lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: None,
        position: LatLon::new(lat, lon).expect("valid coords"),
    }))
}

// ── airspace bands ─────────────────────────────────────────────────────

#[test]
fn agl_floor_follows_the_terrain_slope() {
    // Terrain 100 + 25·i m at stations every 1 km; crossing 5–8 km with a
    // 300 m AGL floor and a 2000 m AMSL ceiling: floors 525/550/575/600 m
    // follow the slope, the ceiling stays flat.
    let corridor = corridor(
        (0..=10)
            .map(|i| sample(i, 0, i as f64 * 1000.0, Some(100.0 + 25.0 * i as f64)))
            .collect(),
    );
    let crossing = crossing(
        VerticalLimit::agl(MetersAgl(300.0)),
        VerticalLimit::amsl(MetersAmsl(2000.0)),
        5000.0,
        8000.0,
    );

    let bands = crossing_bands(&corridor, &crossing);
    assert_eq!(bands.len(), 4, "stations 5..=8, inclusive at both ends");
    for (offset, band) in bands.iter().enumerate() {
        let i = 5 + offset;
        assert_eq!(band.along_track, Meters(i as f64 * 1000.0));
        let expected_floor = 100.0 + 25.0 * i as f64 + 300.0;
        assert_eq!(band.floor, MetersAmsl(expected_floor));
        assert_eq!(band.ceiling, Some(MetersAmsl(2000.0)));
    }
}

#[test]
fn gnd_floor_sits_on_terrain_and_unl_ceiling_stays_open() {
    let corridor = corridor(vec![
        sample(0, 0, 0.0, Some(412.0)),
        sample(1, 0, 1000.0, None), // outside elevation coverage
    ]);
    let crossing = crossing(VerticalLimit::gnd(), VerticalLimit::unl(), 0.0, 1000.0);

    let bands = crossing_bands(&corridor, &crossing);
    assert_eq!(bands.len(), 2);
    assert_eq!(
        bands[0].floor,
        MetersAmsl(412.0),
        "GND sits on the silhouette"
    );
    assert_eq!(
        bands[1].floor,
        MetersAmsl(0.0),
        "unknown terrain draws from sea level"
    );
    assert!(
        bands.iter().all(|b| b.ceiling.is_none()),
        "UNL is the view's chart top"
    );
}

#[test]
fn fl_limits_resolve_on_the_standard_atmosphere() {
    // FL 50 floor = 5000 ft = 1524 m exactly, terrain-independent; the
    // AGL ceiling over unknown terrain rides a sea-level base.
    let corridor = corridor(vec![sample(0, 0, 0.0, None)]);
    let crossing = crossing(
        VerticalLimit::fl(50),
        VerticalLimit::agl(MetersAgl(700.0)),
        0.0,
        500.0,
    );
    let bands = crossing_bands(&corridor, &crossing);
    assert_eq!(bands.len(), 1);
    assert_eq!(bands[0].floor, MetersAmsl::from_feet(5000.0));
    assert_eq!(bands[0].ceiling, Some(MetersAmsl(700.0)));
}

#[test]
fn stations_outside_the_crossing_interval_are_excluded() {
    let corridor = corridor(
        (0..=10)
            .map(|i| sample(i, 0, i as f64 * 1000.0, Some(100.0)))
            .collect(),
    );
    let crossing = crossing(
        VerticalLimit::gnd(),
        VerticalLimit::amsl(MetersAmsl(1500.0)),
        2500.0,
        4500.0,
    );
    let bands = crossing_bands(&corridor, &crossing);
    let along: Vec<f64> = bands.iter().map(|b| b.along_track.0).collect();
    assert_eq!(along, [3000.0, 4000.0]);
}

// ── minimum safe altitude ──────────────────────────────────────────────

#[test]
fn msa_per_leg_takes_the_worst_of_terrain_and_obstacles() {
    // Leg 0: terrain up to 500 m. Leg 1: terrain 300 m but an 800 m mast
    // top — the obstacle wins. Buffer 304.8 m (1000 ft).
    let mut samples = vec![
        sample(0, 0, 0.0, Some(420.0)),
        sample(1, 0, 1000.0, Some(500.0)),
        sample(2, 1, 2000.0, Some(300.0)),
        sample(3, 1, 3000.0, Some(280.0)),
    ];
    samples[2].tallest_obstacle = Some(obstacle(800.0));
    let msa = msa_per_leg(&corridor(samples), MetersAgl::from_feet(1000.0));

    assert_eq!(msa.len(), 2);
    assert_eq!(msa[0], Some(MetersAmsl(500.0 + 304.8)));
    assert_eq!(msa[1], Some(MetersAmsl(800.0 + 304.8)));
}

#[test]
fn msa_handles_missing_data_and_station_less_legs() {
    // Stations only on legs 0 and 2 (leg 1 shorter than the spacing);
    // leg 2's stations are outside elevation coverage with no obstacles.
    let samples = vec![sample(0, 0, 0.0, Some(150.0)), sample(1, 2, 1200.0, None)];
    let msa = msa_per_leg(&corridor(samples), MetersAgl(100.0));
    assert_eq!(msa, [Some(MetersAmsl(250.0)), None, None]);

    assert!(msa_per_leg(&corridor(Vec::new()), MetersAgl(100.0)).is_empty());
}

// ── planned altitude sampling ──────────────────────────────────────────

#[test]
fn planned_altitude_samples_the_phase_plan() {
    // Climb 0→1000 m over the first 10 km, cruise to 30 km, descent over
    // the last 10 km (the conflict tests' shared builder).
    let plan =
        crate::conflict::tests::climb_cruise_descent_plan(40_000.0, 10_000.0, 10_000.0, 1000.0);
    assert_eq!(
        planned_altitude_at(&plan, Meters(5_000.0)),
        Some(MetersAmsl(500.0))
    );
    assert_eq!(
        planned_altitude_at(&plan, Meters(20_000.0)),
        Some(MetersAmsl(1000.0))
    );

    let empty = PhasePlan {
        segments: Vec::new(),
        toc: None,
        tod: None,
        total_duration: Minutes(0.0),
        total_fuel: Liters(0.0),
    };
    assert_eq!(planned_altitude_at(&empty, Meters(0.0)), None);
}

// ── freezing level ─────────────────────────────────────────────────────

#[test]
fn freezing_level_extrapolates_the_isa_lapse() {
    // +6.5 °C at 2000 m → 0 °C exactly 1000 m higher (6.5 °C/km).
    assert_eq!(
        freezing_level_estimate(Celsius(6.5), MetersAmsl(2000.0)),
        MetersAmsl(3000.0)
    );
    // −6.5 °C at 1000 m → the level is honestly *below* the sample.
    assert_eq!(
        freezing_level_estimate(Celsius(-6.5), MetersAmsl(1000.0)),
        MetersAmsl(0.0)
    );
}

#[test]
fn freezing_levels_resolve_per_leg_altitudes() {
    // Two legs: leg 0 overrides to 2000 m, leg 1 flies the 1000 m cruise.
    let mut route = vec![
        waypoint(50.0, 8.0),
        waypoint(50.0, 8.4),
        waypoint(50.0, 8.8),
    ];
    route[0].leg_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(2000.0)));
    let cruise = Some(PlannedAltitude::Amsl(MetersAmsl(1000.0)));
    let winds = [leg_wind(0, 6.5), leg_wind(1, -6.5)];

    let levels = freezing_levels(&route, cruise, &winds);
    assert_eq!(levels, [Some(MetersAmsl(3000.0)), Some(MetersAmsl(0.0))]);

    // A leg without a wind entry, or without a resolvable altitude,
    // yields None instead of a made-up number.
    let levels = freezing_levels(&route, cruise, &winds[..1]);
    assert_eq!(levels[1], None);
    let levels = freezing_levels(&route, None, &winds);
    assert_eq!(levels, [Some(MetersAmsl(3000.0)), None]);
}

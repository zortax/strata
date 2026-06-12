//! Compute-façade tests: synthetic sources, real math end to end
//! (plan §3 `compute/`). Worked values are quoted in comments; the
//! per-module depth lives in each module's own tests — here the assertions
//! target the *wiring*: consistent inputs, frozen output shape, error
//! mapping, determinism and the <10 ms latency budget.

use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use strata_data::domain::{
    Airspace, AirspaceClass, AirspaceKind, BoundingBox, LatLon, Meters, MetersAgl, MetersAmsl,
    Obstacle, ObstacleKind, Polygon, VerticalLimit,
};

use super::*;
use crate::aircraft::{
    AircraftId, ClimbPerformance, DescentPerformance, EnvelopePoint, PowerSetting, StationKind,
    WbStation,
};
use crate::conflict::{ConflictKind, ConflictLocation, ConflictSeverity};
use crate::flight::{FreePoint, PlannedAltitude, RoutePoint, StationLoad};
use crate::navlog::NavLogRowKind;
use crate::perf::PhaseKind;
use crate::sources::{
    AirspaceSource, MagvarSource, ObstacleSource, Provenance, WindsAloft, WindsAloftSampler,
};
use crate::units::{
    Celsius, FeetPerMinute, Kilograms, Knots, Liters, LitersPerHour, METERS_PER_NAUTICAL_MILE,
    MagneticVariation,
};
use crate::wb::WbStateKind;
use crate::wind::LegWindOrigin;

// ── synthetic sources ──────────────────────────────────────────────────

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

struct FnTerrain<F: Fn(LatLon) -> Option<f64>>(F);

impl<F: Fn(LatLon) -> Option<f64>> ElevationSource for FnTerrain<F> {
    fn max_elevation_at(&self, p: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        Ok((self.0)(p).map(MetersAmsl))
    }
}

struct FailingTerrain;

impl ElevationSource for FailingTerrain {
    fn max_elevation_at(&self, _: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        Err(SourceError::new("elevation store on fire"))
    }
}

struct VecObstacles(Vec<Obstacle>);

impl ObstacleSource for VecObstacles {
    fn obstacles_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Obstacle>, SourceError> {
        Ok(self
            .0
            .iter()
            .filter(|o| bbox.contains(o.position))
            .cloned()
            .collect())
    }
}

struct VecAirspaces(Vec<Airspace>);

impl AirspaceSource for VecAirspaces {
    fn airspaces_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airspace>, SourceError> {
        Ok(self
            .0
            .iter()
            .filter(|a| bbox.intersects(&a.geometry.bounding_box()))
            .cloned()
            .collect())
    }
}

struct ConstWind(WindsAloft);

impl WindsAloftSampler for ConstWind {
    fn sample(
        &self,
        _: LatLon,
        _: MetersAmsl,
        _: DateTime<Utc>,
    ) -> Result<Option<WindsAloft>, SourceError> {
        Ok(Some(self.0))
    }
}

fn westerly() -> ConstWind {
    ConstWind(WindsAloft {
        direction: DegreesTrue::new(270.0),
        speed: Knots(20.0),
        temperature: Celsius(5.0),
        temperature_provenance: Provenance::Real,
    })
}

fn calm() -> ConstWind {
    ConstWind(WindsAloft {
        direction: DegreesTrue::new(0.0),
        speed: Knots(0.0),
        temperature: Celsius(15.0),
        temperature_provenance: Provenance::Real,
    })
}

struct ConstMagvar(f64);

impl MagvarSource for ConstMagvar {
    fn magvar(&self, _: LatLon, _: NaiveDate) -> Result<MagneticVariation, SourceError> {
        Ok(MagneticVariation(self.0))
    }
}

// ── fixtures ───────────────────────────────────────────────────────────

/// Test aircraft (C172-ish), the same numbers as the perf/wb worked
/// examples: cruise "65 %" = 110 kt @ 35 L/h (first) and "75 %" = 120 kt @
/// 40 L/h; climb 70 kt / 500 ft/min / 40 L/h; descent 90 kt / 500 ft/min /
/// 20 L/h; taxi flow 12 L/h. Empty 743 kg @ 1.04 m, MTOW 1111 kg, fuel
/// station at 1.17 m, density 0.72 kg/L.
fn aircraft() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("e2e").unwrap());
    profile.performance.cruise_settings = vec![
        PowerSetting {
            name: "65 %".into(),
            tas: Knots(110.0),
            fuel_flow: LitersPerHour(35.0),
        },
        PowerSetting {
            name: "75 %".into(),
            tas: Knots(120.0),
            fuel_flow: LitersPerHour(40.0),
        },
    ];
    profile.performance.climb = ClimbPerformance {
        ias: Knots(70.0),
        rate: FeetPerMinute(500.0),
        fuel_flow: LitersPerHour(40.0),
    };
    profile.performance.descent = DescentPerformance {
        ias: Knots(90.0),
        rate: FeetPerMinute(500.0),
        fuel_flow: LitersPerHour(20.0),
    };
    profile.performance.taxi_fuel_flow = LitersPerHour(12.0);
    profile.fuel.usable = Liters(201.0);
    profile.weight_balance.empty_mass = Kilograms(743.0);
    profile.weight_balance.empty_arm = Meters(1.04);
    profile.weight_balance.max_takeoff = Kilograms(1111.0);
    profile.weight_balance.stations = vec![
        WbStation {
            name: "front seats".into(),
            arm: Meters(0.94),
            kind: StationKind::Seat,
            max_load: None,
        },
        WbStation {
            name: "fuel".into(),
            arm: Meters(1.17),
            kind: StationKind::Fuel,
            max_load: None,
        },
    ];
    profile.weight_balance.envelope = vec![
        EnvelopePoint {
            arm: Meters(0.89),
            mass: Kilograms(600.0),
        },
        EnvelopePoint {
            arm: Meters(0.89),
            mass: Kilograms(885.0),
        },
        EnvelopePoint {
            arm: Meters(1.00),
            mass: Kilograms(1111.0),
        },
        EnvelopePoint {
            arm: Meters(1.20),
            mass: Kilograms(1111.0),
        },
        EnvelopePoint {
            arm: Meters(1.20),
            mass: Kilograms(600.0),
        },
    ];
    profile
}

fn wp(name: &str, lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: Some(name.into()),
        position: ll(lat, lon),
    }))
}

/// A flight on `route` at 3500 ft (1066.8 m) AMSL, departing 2026-06-15
/// 10:00 UTC, two pilots up front (154 kg) and 100 L of fuel, default NCO
/// fuel policy (10 min taxi / 5 % / 30 min reserve).
fn flight(route: Vec<RouteWaypoint>) -> FlightDoc {
    let mut doc = FlightDoc::new("E2E");
    doc.route = route;
    doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(1066.8)));
    doc.departure_time = Some(Utc.with_ymd_and_hms(2026, 6, 15, 10, 0, 0).unwrap());
    doc.loading.station_loads = vec![StationLoad {
        station: "front seats".into(),
        mass: Kilograms(154.0),
    }];
    doc.loading.fuel = Liters(100.0);
    doc
}

/// The end-to-end route: A (50.0, 8.0) → B (50.0, 8.4) → C (50.3, 8.4) →
/// D (50.3, 8.8). Leg lengths (haversine, R = 6 371 008.8 m): ~28.6 km,
/// ~33.4 km, ~28.4 km — total ~90.4 km ≈ 48.8 NM.
fn e2e_route() -> Vec<RouteWaypoint> {
    vec![
        wp("A", 50.0, 8.0),
        wp("B", 50.0, 8.4),
        wp("C", 50.3, 8.4),
        wp("D", 50.3, 8.8),
    ]
}

/// Terrain for the end-to-end flight: 200 m everywhere except a 1000 m
/// ridge across lon 8.55–8.65 north of lat 50.2 — squarely under the last
/// leg (C→D at lat 50.3), and out of lateral reach (>10.7 km) of the B→C
/// corridor at lon 8.4 with the default 5 NM half-width (9.26 km).
fn ridge_terrain() -> FnTerrain<impl Fn(LatLon) -> Option<f64>> {
    FnTerrain(|p: LatLon| {
        Some(if (8.55..=8.65).contains(&p.lon()) && p.lat() > 50.2 {
            1000.0
        } else {
            200.0
        })
    })
}

fn rect(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> Polygon {
    Polygon::new(
        vec![
            ll(lat0, lon0),
            ll(lat0, lon1),
            ll(lat1, lon1),
            ll(lat1, lon0),
        ],
        vec![],
    )
    .unwrap()
}

/// ED-R 137 across leg A→B (lon 8.15–8.25 at lat 49.85–50.15),
/// GND–5000 ft AMSL — the climb passes through at 200–1066.8 m, well
/// inside the band, so the conflict list must carry a Warning.
fn restricted_area() -> Airspace {
    Airspace {
        name: "ED-R 137".into(),
        class: AirspaceClass::Unclassified,
        kind: AirspaceKind::Restricted,
        lower: VerticalLimit::gnd(),
        upper: VerticalLimit::amsl(MetersAmsl::from_feet(5000.0)),
        geometry: rect(49.85, 8.15, 50.15, 8.25),
        airac: None,
    }
}

/// A 900 m AMSL mast on the A→B centerline at lon 8.3 (~21.5 km along,
/// past the ~12.3 km TOC): cruise 1066.8 m < 900 + 304.8 m buffer →
/// deliberate obstacle conflict.
fn mast() -> Obstacle {
    Obstacle {
        name: Some("Sender Mainflingen".into()),
        kind: ObstacleKind::Mast,
        position: ll(50.0, 8.3),
        height: MetersAgl(700.0),
        elevation_top: MetersAmsl(900.0),
        lighted: true,
    }
}

fn nm(meters: f64) -> f64 {
    meters / METERS_PER_NAUTICAL_MILE
}

// ── frozen-type sanity (kept from the skeleton) ────────────────────────

#[test]
fn default_params_compose_module_defaults() {
    let params = ComputeParams::default();
    assert_eq!(params.corridor, CorridorParams::default());
    assert_eq!(params.thresholds, ConflictThresholds::default());
}

// ── not-computable classification ──────────────────────────────────────

/// The synthetic-source bundle every classification test shares.
fn benign_sources_compute(doc: &FlightDoc, aircraft: &AircraftProfile) -> ComputeOutcome {
    let terrain = ridge_terrain();
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    compute(doc, aircraft, &sources, &ComputeParams::default()).expect("no real failure")
}

#[test]
fn short_or_degenerate_routes_are_not_computable() {
    let aircraft = aircraft();
    for (route, expected) in [
        (Vec::new(), NotComputable::NoRoute),
        (vec![wp("A", 50.0, 8.0)], NotComputable::RouteTooShort),
        // Two coincident waypoints: zero total length, no corridor/profile.
        (
            vec![wp("A", 50.0, 8.0), wp("A2", 50.0, 8.0)],
            NotComputable::RouteTooShort,
        ),
    ] {
        let doc = flight(route);
        assert_eq!(
            benign_sources_compute(&doc, &aircraft),
            ComputeOutcome::NotComputable(expected)
        );
    }
}

#[test]
fn missing_planned_altitude_is_not_computable_with_the_leg_index() {
    let aircraft = aircraft();

    // No cruise altitude, no leg altitudes: the first leg is the gap.
    let mut doc = flight(e2e_route());
    doc.cruise_altitude = None;
    assert_eq!(
        benign_sources_compute(&doc, &aircraft),
        ComputeOutcome::NotComputable(NotComputable::MissingAltitude { leg: 0 })
    );

    // The first gap wins: legs 0 and 1 covered, leg 2 missing.
    let altitude = Some(PlannedAltitude::Amsl(MetersAmsl(1066.8)));
    doc.route[0].leg_altitude = altitude;
    doc.route[1].leg_altitude = altitude;
    assert_eq!(
        benign_sources_compute(&doc, &aircraft),
        ComputeOutcome::NotComputable(NotComputable::MissingAltitude { leg: 2 })
    );

    // Every leg covered: no cruise altitude needed — it computes.
    doc.route[2].leg_altitude = altitude;
    assert!(matches!(
        benign_sources_compute(&doc, &aircraft),
        ComputeOutcome::Computed(_)
    ));
}

// ── error mapping ──────────────────────────────────────────────────────

#[test]
fn unknown_power_setting_fails_fast_as_perf_error() {
    let aircraft = aircraft();
    let terrain = ridge_terrain();
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    let mut doc = flight(e2e_route());
    doc.power_setting = Some("max chat".into());

    let err = compute(&doc, &aircraft, &sources, &ComputeParams::default()).unwrap_err();
    assert!(matches!(
        err,
        ComputeError::Perf(PerfError::UnknownPowerSetting(name)) if name == "max chat"
    ));
}

#[test]
fn empty_cruise_table_is_no_cruise_setting() {
    let mut aircraft = aircraft();
    aircraft.performance.cruise_settings.clear();
    let terrain = ridge_terrain();
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    let doc = flight(e2e_route());

    let err = compute(&doc, &aircraft, &sources, &ComputeParams::default()).unwrap_err();
    assert!(matches!(
        err,
        ComputeError::Perf(PerfError::NoCruiseSetting)
    ));
}

#[test]
fn elevation_source_failure_propagates_from_the_corridor() {
    let aircraft = aircraft();
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: &FailingTerrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    let doc = flight(e2e_route());

    let err = compute(&doc, &aircraft, &sources, &ComputeParams::default()).unwrap_err();
    assert!(matches!(
        err,
        ComputeError::Corridor(CorridorError::Source(_))
    ));
}

// ── the end-to-end synthetic flight ────────────────────────────────────

/// Builds the full e2e scenario and computes it: ridge terrain, the ED-R,
/// the mast, westerly 270°/20 kt, 3°E variation, one alternate 14.2 km
/// east of the destination.
fn computed_e2e() -> (FlightDoc, AircraftProfile, ComputedFlight) {
    let aircraft = aircraft();
    let terrain = ridge_terrain();
    let obstacles = VecObstacles(vec![mast()]);
    let airspaces = VecAirspaces(vec![restricted_area()]);
    let wind = westerly();
    let magvar = ConstMagvar(3.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    let mut doc = flight(e2e_route());
    doc.alternates = vec![RoutePoint::Free(FreePoint {
        name: Some("ALT".into()),
        position: ll(50.3, 9.0),
    })];

    let computed = compute(&doc, &aircraft, &sources, &ComputeParams::default())
        .expect("e2e flight computes")
        .computed()
        .expect("e2e flight is computable");
    (doc, aircraft, computed)
}

#[test]
fn end_to_end_legs_and_magnetic_tracks() {
    let (_, _, computed) = computed_e2e();

    assert_eq!(computed.legs.len(), 3);
    assert_eq!(computed.legs[0].from, "A");
    assert_eq!(computed.legs[0].to, "B");
    assert_eq!(computed.legs[2].to, "D");
    for (index, leg) in computed.legs.iter().enumerate() {
        assert_eq!(leg.index, index);
        assert!(leg.distance.0 > 20_000.0);
        // Constant 3°E variation: magnetic = true − 3 ("east is least").
        let expected = (leg.true_track.0 - 3.0).rem_euclid(360.0);
        assert!(
            (leg.magnetic_track.0 - expected).abs() < 1e-9,
            "leg {index}: MT {} vs TT {}",
            leg.magnetic_track.0,
            leg.true_track.0
        );
    }
    // Leg B→C runs due north along the 8.4° meridian: TT 0°, MT 357°.
    assert!(computed.legs[1].true_track.0.abs() < 1e-9);
    assert!((computed.legs[1].magnetic_track.0 - 357.0).abs() < 1e-9);
}

#[test]
fn end_to_end_winds_solve_against_the_westerly() {
    let (_, _, computed) = computed_e2e();

    assert_eq!(computed.winds.len(), 3);
    for (index, wind) in computed.winds.iter().enumerate() {
        assert_eq!(wind.leg_index, index);
        assert_eq!(wind.origin, LegWindOrigin::Sampled);
        assert_eq!(wind.wind.speed, Knots(20.0));
    }
    // Northbound leg (TT 0°), wind from 270° at 20 kt, TAS 110 kt:
    // WCA = asin(20·sin(270°−0°)/110) = asin(−0.18182) = −10.4757° (left),
    // GS  = 110·cos(−10.4757°) − 20·cos(270°) = 108.166 − 0 = 108.166 kt.
    let north = &computed.winds[1].triangle;
    assert!((north.wind_correction_angle_deg - (-10.4757)).abs() < 1e-3);
    assert!((north.ground_speed.0 - 108.166).abs() < 1e-2);
    // Eastbound legs ride a near-pure 20 kt tailwind: GS ≈ 130 kt.
    assert!((computed.winds[0].triangle.ground_speed.0 - 130.0).abs() < 0.1);
}

#[test]
fn end_to_end_corridor_and_phases_shape() {
    let (doc, _, computed) = computed_e2e();
    let total = route::total_distance(&doc.route).0;

    // Stations from departure to destination in strict along-track order.
    assert!(computed.corridor.samples.len() > 150);
    for pair in computed.corridor.samples.windows(2) {
        assert!(pair[0].station.along_track.0 < pair[1].station.along_track.0);
    }
    let last = computed.corridor.samples.last().unwrap();
    assert!((last.station.along_track.0 - total).abs() < 1.0);
    // The ED-R is seen as a crossing.
    assert!(
        computed
            .corridor
            .crossings
            .iter()
            .any(|c| c.airspace.name == "ED-R 137")
    );

    // Phases: climb 200 → 1066.8 m at 500 ft/min / 70 kt (gradient
    // 2.54/36.011 = 0.070534) tops out at 866.8/0.070534 = 12 289 m;
    // descent at 90 kt (gradient 0.054860) starts 866.8/0.054860 =
    // 15 802 m before the end (TOD ≈ 74 560 m of ~90 360 m).
    let phases = &computed.phases;
    let toc = phases.toc.expect("reaches cruise");
    let tod = phases.tod.expect("leaves cruise");
    assert!((toc.along_track.0 - 12_289.0).abs() < 10.0, "{toc:?}");
    assert!(
        (tod.along_track.0 - (total - 15_802.0)).abs() < 10.0,
        "{tod:?}"
    );
    assert_eq!(toc.altitude, MetersAmsl(1066.8));
    // Segments span 0..total gap-free.
    assert!(phases.segments[0].start_along_track.0.abs() < 1e-6);
    for pair in phases.segments.windows(2) {
        assert!((pair[1].start_along_track.0 - pair[0].end_along_track.0).abs() < 1e-6);
    }
    assert!((phases.segments.last().unwrap().end_along_track.0 - total).abs() < 1e-6);
    assert!(phases.total_duration.0 > 0.0);
    assert!(phases.total_fuel.0 > 0.0);
}

#[test]
fn end_to_end_conflicts_contain_the_planted_hazards() {
    let (_, _, computed) = computed_e2e();
    let conflicts = &computed.conflicts;

    // The 1000 m ridge under the C→D leg (lon 8.55–8.65 → ~72.6–79.7 km
    // along track): cruise 1066.8 m < 1000 + 304.8 m buffer, and the
    // descent even dips below the ridge top.
    let terrain = conflicts
        .iter()
        .find(|c| c.kind == ConflictKind::Terrain)
        .expect("ridge produces a terrain conflict");
    assert_eq!(terrain.severity, ConflictSeverity::Warning);
    let ConflictLocation::Station { along_track, .. } = terrain.location else {
        panic!("terrain conflicts anchor at a station: {terrain:?}");
    };
    assert!(
        (70_000.0..82_000.0).contains(&along_track.0),
        "terrain conflict at {} m, expected on the ridge",
        along_track.0
    );

    // The 900 m mast under cruise: 1066.8 < 900 + 304.8.
    let obstacle = conflicts
        .iter()
        .find(|c| c.kind == ConflictKind::Obstacle)
        .expect("mast produces an obstacle conflict");
    assert!(obstacle.message.contains("Sender Mainflingen"));

    // The ED-R penetration during the climb is a Warning.
    let airspace = conflicts
        .iter()
        .find(|c| c.kind == ConflictKind::Airspace)
        .expect("ED-R produces an airspace conflict");
    assert_eq!(airspace.severity, ConflictSeverity::Warning);
    assert!(
        airspace.message.contains("ED-R 137"),
        "{}",
        airspace.message
    );

    // Nothing else: W&B is inside the envelope and fuel margin positive.
    assert!(conflicts.iter().all(|c| matches!(
        c.kind,
        ConflictKind::Terrain | ConflictKind::Obstacle | ConflictKind::Airspace
    )));
}

#[test]
fn end_to_end_fuel_ladder_and_wb_are_consistent() {
    let (_, _, computed) = computed_e2e();
    let fuel = &computed.fuel;

    // Taxi: 10 min × 12 L/h = 2 L.
    assert!((fuel.taxi.0 - 2.0).abs() < 1e-9);
    // Trip is exactly the phase plan's fuel.
    assert_eq!(fuel.trip, computed.phases.total_fuel);
    // Contingency: 5 % of trip.
    assert!((fuel.contingency.0 - 0.05 * fuel.trip.0).abs() < 1e-9);
    // Alternate: 14 206 m diversion D→ALT, apex-capped at the gradients
    // above (apex 6 215 m / +438.4 m): climb 438.4/2.54/60 = 2.876 min →
    // 40 L/h × 0.04794 h = 1.918 L, descent 20 L/h × 0.04794 h = 0.959 L,
    // total ≈ 2.876 L.
    assert!(
        (fuel.alternate.0 - 2.876).abs() < 0.01,
        "{}",
        fuel.alternate.0
    );
    // Final reserve: 30 min at the planned 35 L/h cruise flow = 17.5 L.
    assert!((fuel.final_reserve.0 - 17.5).abs() < 1e-9);
    assert_eq!(fuel.extra, Liters(0.0));
    let rungs = fuel.taxi.0
        + fuel.trip.0
        + fuel.contingency.0
        + fuel.alternate.0
        + fuel.final_reserve.0
        + fuel.extra.0;
    assert!((fuel.minimum_required.0 - rungs).abs() < 1e-12);
    assert_eq!(fuel.loaded, Liters(100.0));
    assert!((fuel.margin.0 - (100.0 - fuel.minimum_required.0)).abs() < 1e-12);

    // W&B states in order, all inside the envelope. Worked masses:
    // zero-fuel 743 + 154 = 897 kg; ramp + 100 L × 0.72 = 969 kg;
    // takeoff = ramp − taxi 2 L × 0.72 = 967.56 kg;
    // landing = takeoff − trip × 0.72 kg.
    let states = &computed.weight_balance.states;
    let kinds: Vec<_> = states.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            WbStateKind::Ramp,
            WbStateKind::Takeoff,
            WbStateKind::ZeroFuel,
            WbStateKind::Landing
        ]
    );
    assert!(states.iter().all(|s| s.within_envelope), "{states:?}");
    assert!((states[0].mass.0 - 969.0).abs() < 1e-9);
    assert!((states[1].mass.0 - 967.56).abs() < 1e-9);
    assert!((states[2].mass.0 - 897.0).abs() < 1e-9);
    let landing_expected = 897.0 + (98.0 - fuel.trip.0) * 0.72;
    assert!((states[3].mass.0 - landing_expected).abs() < 1e-9);
}

#[test]
fn end_to_end_navlog_rows_and_totals() {
    let (doc, _, computed) = computed_e2e();
    let navlog = &computed.navlog;
    let total = route::total_distance(&doc.route).0;

    // Departure row + A→TOC→B→C→TOD→D checkpoints in along-track order.
    let kinds: Vec<_> = navlog.rows.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            NavLogRowKind::Waypoint,
            NavLogRowKind::TopOfClimb,
            NavLogRowKind::Waypoint,
            NavLogRowKind::Waypoint,
            NavLogRowKind::TopOfDescent,
            NavLogRowKind::Waypoint,
        ]
    );
    let departure = &navlog.rows[0];
    assert_eq!(departure.label, "A");
    assert!(departure.distance.is_none());
    assert!(departure.ete.is_none());
    assert!(departure.true_track.is_none());

    // The arriving rows agree with the leg summaries on magnetic track
    // (same midpoint variation, same date convention).
    let row_b = &navlog.rows[2];
    assert_eq!(row_b.label, "B");
    let mt = row_b.magnetic_track.expect("arriving row has MT");
    assert!((mt.0 - computed.legs[0].magnetic_track.0).abs() < 1e-9);

    // ETAs strictly increase, distances sum to the route total.
    let etas: Vec<_> = navlog.rows.iter().filter_map(|r| r.eta).collect();
    assert_eq!(etas.len(), navlog.rows.len() - 1);
    assert!(etas.windows(2).all(|p| p[0] < p[1]));
    let distance_sum: f64 = navlog
        .rows
        .iter()
        .filter_map(|r| r.distance)
        .map(|d| d.0)
        .sum();
    assert!((distance_sum - nm(total)).abs() < 1e-6);
    assert!((navlog.totals.distance.0 - nm(total)).abs() < 1e-9);
    assert_eq!(navlog.totals.fuel, computed.phases.total_fuel);
    // Frequencies stay None: compute's Sources carry no airport data.
    assert!(navlog.rows.iter().all(|r| r.frequency.is_none()));
}

#[test]
fn end_to_end_is_deterministic_and_serializable() {
    let (_, _, first) = computed_e2e();
    let (_, _, second) = computed_e2e();
    assert_eq!(first, second);

    // ComputedFlight is the strata-brief PDF context: full serde round-trip.
    let json = serde_json::to_string(&first).expect("serializes");
    let back: ComputedFlight = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, first);
}

// ── the no-wind sanity flight ──────────────────────────────────────────

/// Calm wind, flat 1000 ft terrain, cruise exactly at field elevation:
/// the profile is one cruise segment and every time is the textbook
/// `ETE = distance / TAS` — asserted exactly, not approximately.
#[test]
fn no_wind_ete_is_distance_over_tas_exactly() {
    let aircraft = aircraft();
    // 304.8 m = 1000 ft everywhere; cruise at 1000 ft → no climb/descent.
    let terrain = FnTerrain(|_| Some(304.8));
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    let mut doc = flight(vec![wp("A", 50.0, 8.0), wp("B", 50.0, 8.5)]);
    doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(304.8)));
    // Zero clearance buffers: flying *at* the terrain elevation is not
    // strictly below terrain + 0, so the flight is conflict-free.
    let params = ComputeParams {
        corridor: CorridorParams::default(),
        thresholds: ConflictThresholds {
            terrain_clearance: MetersAgl(0.0),
            obstacle_clearance: MetersAgl(0.0),
            ..ConflictThresholds::default()
        },
    };

    let computed = compute(&doc, &aircraft, &sources, &params)
        .expect("computes")
        .computed()
        .expect("computable");

    // Calm wind solves trivially: WCA 0, GS == TAS, exactly.
    let triangle = &computed.winds[0].triangle;
    assert_eq!(triangle.wind_correction_angle_deg, 0.0);
    assert_eq!(triangle.ground_speed, Knots(110.0));

    // One cruise segment, no TOC/TOD.
    assert_eq!(computed.phases.segments.len(), 1);
    assert_eq!(computed.phases.segments[0].kind, PhaseKind::Cruise);
    assert!(computed.phases.toc.is_none());
    assert!(computed.phases.tod.is_none());

    // ETE = distance / TAS: ~35.7 km ≈ 19.3 NM at 110 kt ≈ 10.53 min.
    let distance_nm = nm(route::total_distance(&doc.route).0);
    let expected_ete = distance_nm / 110.0 * 60.0;
    assert!((computed.navlog.totals.ete.0 - expected_ete).abs() < 1e-9);
    let row = &computed.navlog.rows[1];
    assert!((row.ete.unwrap().0 - expected_ete).abs() < 1e-9);
    // Trip fuel = flow × time at the same exactness.
    let expected_trip = 35.0 * (distance_nm / 110.0);
    assert!((computed.fuel.trip.0 - expected_trip).abs() < 1e-9);
    // No alternate → a zero alternate rung.
    assert_eq!(computed.fuel.alternate, Liters(0.0));

    assert!(computed.conflicts.is_empty(), "{:?}", computed.conflicts);
}

// ── free-point endpoint semantics ──────────────────────────────────────

/// Shared driver: computes `doc` over `terrain` with otherwise-benign
/// sources and returns the computed flight.
fn compute_over_terrain(
    doc: &FlightDoc,
    terrain: &dyn crate::sources::ElevationSource,
) -> ComputedFlight {
    let obstacles = VecObstacles(Vec::new());
    let airspaces = VecAirspaces(Vec::new());
    let wind = calm();
    let magvar = ConstMagvar(0.0);
    let sources = Sources {
        elevation: terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    compute(doc, &aircraft(), &sources, &ComputeParams::default())
        .expect("computes")
        .computed()
        .expect("computable")
}

/// A ~33 km due-north route along the 8.0°E meridian between two free
/// points, at the fixture's 1066.8 m cruise.
fn northbound_free_route() -> FlightDoc {
    flight(vec![wp("DEP", 50.0, 8.0), wp("DEST", 50.3, 8.0)])
}

/// Free-point departure over flat terrain whose elevation is *unknown at
/// the exact departure coordinate* (the gate's bug: the climb used to be
/// evaluated from the sea-level fallback, raising "terrain above planned
/// altitude" at 0.0 NM). The corridor's ground at station 0 is the honest
/// departure elevation — no conflict anywhere.
#[test]
fn free_point_departure_over_flat_terrain_is_clean() {
    // 200 m everywhere, but the point query at the departure coordinate
    // itself is outside coverage.
    let terrain = FnTerrain(|p: LatLon| {
        ((p.lat() - 50.0).abs() > 1e-9 || (p.lon() - 8.0).abs() > 1e-9).then_some(200.0)
    });
    let computed = compute_over_terrain(&northbound_free_route(), &terrain);

    let start = computed.phases.segments[0].start_altitude;
    assert_eq!(
        start,
        MetersAmsl(200.0),
        "climb starts on the corridor ground"
    );
    assert!(
        computed.conflicts.is_empty(),
        "flat free-point departure must not conflict: {:?}",
        computed.conflicts
    );
}

/// Sloping terrain abeam the track: a 700 m plateau ~1.4 km east of the
/// route (well inside the 5 NM corridor) while the track itself sits at
/// 200 m. The corridor's worst case is 700 m at *every* station — a free
/// departure starts on that ground reference, so neither the climb-out
/// nor the cruise (1066.8 − 700 = 366.8 m > 304.8 m buffer) conflicts.
/// Before the fix this raised "terrain above planned altitude" at 0.0 NM.
#[test]
fn free_point_departure_beside_sloping_terrain_is_clean() {
    let terrain = FnTerrain(|p: LatLon| Some(if p.lon() > 8.02 { 700.0 } else { 200.0 }));
    let computed = compute_over_terrain(&northbound_free_route(), &terrain);

    let segments = &computed.phases.segments;
    assert_eq!(segments[0].start_altitude, MetersAmsl(700.0));
    assert_eq!(
        segments.last().unwrap().end_altitude,
        MetersAmsl(700.0),
        "the free destination mirrors the departure semantics"
    );
    assert!(
        computed.conflicts.is_empty(),
        "free endpoints beside higher corridor terrain must not conflict: {:?}",
        computed.conflicts
    );
}

/// Named endpoints keep the field-elevation point query — an airport's
/// published elevation is authoritative even when the corridor sees
/// higher ground abeam.
#[test]
fn named_endpoints_keep_the_field_elevation() {
    let named = |id: &str, lat: f64| {
        RouteWaypoint::new(RoutePoint::Named(crate::flight::NamedPoint {
            kind: crate::flight::NamedPointKind::Airport,
            id: id.to_owned(),
            name: id.to_owned(),
            position: ll(lat, 8.0),
        }))
    };
    let doc = flight(vec![named("EDXA", 50.0), named("EDXB", 50.3)]);
    let terrain = FnTerrain(|p: LatLon| Some(if p.lon() > 8.02 { 700.0 } else { 200.0 }));
    let computed = compute_over_terrain(&doc, &terrain);

    let segments = &computed.phases.segments;
    assert_eq!(segments[0].start_altitude, MetersAmsl(200.0));
    assert_eq!(segments.last().unwrap().end_altitude, MetersAmsl(200.0));
}

/// A genuine ridge under the climb-out, beyond the 1 NM grace, still
/// conflicts: 800 m across the track at ~3 NM, planned altitude there
/// ≈ 592 m — flagged, while nothing fires inside the grace.
#[test]
fn ridge_three_nm_out_still_conflicts_for_a_free_departure() {
    let terrain = FnTerrain(|p: LatLon| {
        Some(if (50.045..=50.055).contains(&p.lat()) {
            800.0
        } else {
            200.0
        })
    });
    let computed = compute_over_terrain(&northbound_free_route(), &terrain);

    let terrain_conflicts: Vec<_> = computed
        .conflicts
        .iter()
        .filter(|c| c.kind == ConflictKind::Terrain)
        .collect();
    assert_eq!(terrain_conflicts.len(), 1, "{:?}", computed.conflicts);
    let ConflictLocation::Station { along_track, .. } = terrain_conflicts[0].location else {
        panic!("terrain conflicts anchor at a station");
    };
    // The ridge spans lat 50.045–50.055 ≈ 5.0–6.1 km along track: well
    // beyond the grace, well before TOC.
    assert!(
        (4_000.0..7_000.0).contains(&along_track.0),
        "conflict at {} m, expected on the ridge",
        along_track.0
    );
    assert!(
        along_track.0 > METERS_PER_NAUTICAL_MILE,
        "nothing may fire inside the departure grace"
    );
}

// ── latency budget ─────────────────────────────────────────────────────

/// Bench-style measurement over a 6-leg, ~125 NM route with hilly analytic
/// terrain, ten obstacles and five airspaces at default corridor
/// resolution (≈465 stations × 9 lateral samples). The plan budget is
/// <10 ms typical (release); the assertion uses a debug-build headroom of
/// 100 ms, and the measured time is printed for the record.
#[test]
fn six_leg_route_meets_the_latency_budget() {
    let aircraft = aircraft();
    // Gentle analytic hills, 150–450 m.
    let terrain = FnTerrain(|p: LatLon| {
        Some(300.0 + 150.0 * (p.lat() * 37.0).sin() * (p.lon() * 23.0).cos())
    });
    let obstacles = VecObstacles(
        (0..10)
            .map(|i| Obstacle {
                name: Some(format!("mast {i}")),
                kind: ObstacleKind::WindTurbine,
                position: ll(50.0 + 0.02 * f64::from(i), 7.2 + 0.3 * f64::from(i)),
                height: MetersAgl(200.0),
                elevation_top: MetersAmsl(550.0),
                lighted: true,
            })
            .collect(),
    );
    let airspaces = VecAirspaces(
        (0..5)
            .map(|i| {
                let lon = 7.1 + 0.6 * f64::from(i);
                Airspace {
                    name: format!("AREA {i}"),
                    class: AirspaceClass::D,
                    kind: AirspaceKind::Ctr,
                    lower: VerticalLimit::gnd(),
                    upper: VerticalLimit::fl(75),
                    geometry: rect(49.9, lon, 50.3, lon + 0.2),
                    airac: None,
                }
            })
            .collect(),
    );
    let wind = westerly();
    let magvar = ConstMagvar(3.0);
    let sources = Sources {
        elevation: &terrain,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &wind,
        magvar: &magvar,
    };
    // 6 legs zigzagging east: ~125 NM total.
    let mut doc = flight(vec![
        wp("W0", 50.0, 7.0),
        wp("W1", 50.2, 7.5),
        wp("W2", 50.0, 8.0),
        wp("W3", 50.2, 8.5),
        wp("W4", 50.0, 9.0),
        wp("W5", 50.2, 9.5),
        wp("W6", 50.0, 10.0),
    ]);
    doc.alternates = vec![RoutePoint::Free(FreePoint {
        name: Some("ALT".into()),
        position: ll(50.1, 10.2),
    })];
    let params = ComputeParams::default();

    // Warm-up (and shape sanity: the route is long enough to be honest).
    let computed = compute(&doc, &aircraft, &sources, &params)
        .expect("computes")
        .computed()
        .expect("computable");
    assert!(computed.corridor.samples.len() > 400);
    assert!(!computed.conflicts.is_empty());

    let runs = 10;
    let mut best = Duration::MAX;
    let mut sum = Duration::ZERO;
    for _ in 0..runs {
        let start = Instant::now();
        let result = compute(&doc, &aircraft, &sources, &params)
            .expect("computes")
            .computed()
            .expect("computable");
        let elapsed = start.elapsed();
        assert_eq!(result.legs.len(), 6);
        best = best.min(elapsed);
        sum += elapsed;
    }
    let mean = sum / runs;
    eprintln!(
        "compute latency over 6-leg {:.1} NM route ({} stations): best {best:?}, mean {mean:?}",
        nm(route::total_distance(&doc.route).0),
        computed.corridor.samples.len(),
    );
    assert!(
        mean < Duration::from_millis(100),
        "mean compute latency {mean:?} blows the budget"
    );
}

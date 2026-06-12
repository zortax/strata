use strata_data::domain::{LatLon, Meters, MetersAmsl, Qnh, RunwaySurface};

use super::*;
use crate::aircraft::{
    AircraftId, AircraftProfile, ClimbPerformance, DescentPerformance, PowerSetting,
};
use crate::flight::{FreePoint, PlannedAltitude, RoutePoint, RouteWaypoint};
use crate::units::{Celsius, DegreesTrue, FeetPerMinute, Knots, LitersPerHour};

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

fn wp(lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: None,
        position: ll(lat, lon),
    }))
}

fn wp_alt(lat: f64, lon: f64, meters: f64) -> RouteWaypoint {
    let mut waypoint = wp(lat, lon);
    waypoint.leg_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(meters)));
    waypoint
}

/// Test aircraft: cruise 65 % = 110 kt @ 35 L/h (first) and 75 % = 120 kt
/// @ 40 L/h; climb 70 kt / 500 ft/min / 40 L/h; descent 90 kt / 500 ft/min
/// / 20 L/h; takeoff roll 300 m, landing roll 250 m, template factors.
fn aircraft() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("test").unwrap());
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
    profile.distances.takeoff_roll = Meters(300.0);
    profile.distances.landing_roll = Meters(250.0);
    profile
}

/// Reference conditions for the distance-chain worked examples:
/// field 1000 ft, QNH 1013 hPa, OAT 25 °C (see the worked DA below).
fn warm_conditions() -> RunwayConditions {
    RunwayConditions {
        field_elevation: MetersAmsl::from_feet(1000.0),
        qnh: Qnh::Hpa(1013),
        temperature: Celsius(25.0),
        headwind_component: Knots(0.0),
        surface: RunwaySurface::Asphalt,
        slope_percent: 0.0,
        wet: false,
    }
}

fn assert_gap_free(plan: &PhasePlan, total: f64) {
    assert!(!plan.segments.is_empty());
    assert!(plan.segments[0].start_along_track.0.abs() < 1e-6);
    for pair in plan.segments.windows(2) {
        assert!(
            (pair[1].start_along_track.0 - pair[0].end_along_track.0).abs() < 1e-6,
            "gap between {pair:?}"
        );
        assert!(
            (pair[1].start_altitude.0 - pair[0].end_altitude.0).abs() < 0.011,
            "altitude jump between {pair:?}"
        );
    }
    // `total` is the hand-worked value, quoted to millimeters.
    let last = plan.segments.last().unwrap();
    assert!(
        (last.end_along_track.0 - total).abs() < 0.01,
        "{last:?} vs {total}"
    );
    // Totals are the segment sums.
    let duration: f64 = plan.segments.iter().map(|s| s.duration.0).sum();
    let fuel: f64 = plan.segments.iter().map(|s| s.fuel.0).sum();
    assert!((plan.total_duration.0 - duration).abs() < 1e-9);
    assert!((plan.total_fuel.0 - fuel).abs() < 1e-9);
}

// ---------------------------------------------------------------- ISA

#[test]
fn isa_temperature_lapse() {
    assert_eq!(isa_temperature(MetersAmsl(0.0)).0, 15.0);
    // 1524 m (5000 ft): 15 − 0.0065·1524 = 5.094 °C.
    assert!((isa_temperature(MetersAmsl(1524.0)).0 - 5.094).abs() < 1e-12);
}

#[test]
fn planned_altitude_resolution() {
    assert_eq!(
        planned_altitude_amsl(PlannedAltitude::Amsl(MetersAmsl(1234.5))),
        MetersAmsl(1234.5)
    );
    // FL100 = 10 000 ft = 3048 m (pressure altitude treated as AMSL).
    assert!((planned_altitude_amsl(PlannedAltitude::FlightLevel(100)).0 - 3048.0).abs() < 1e-9);
}

// ------------------------------------------------- pressure/density altitude

#[test]
fn pressure_altitude_qnh_correction() {
    // 1000 ft field, QNH 1013 hPa: ΔPA = (1013.25 − 1013)·27.3 = 6.825 ft.
    let pa = pressure_altitude(MetersAmsl::from_feet(1000.0), Qnh::Hpa(1013));
    assert!((pa.as_feet() - 1006.825).abs() < 1e-6, "{}", pa.as_feet());
    // High pressure 1033 hPa: ΔPA = (1013.25 − 1033)·27.3 = −539.175 ft.
    let high = pressure_altitude(MetersAmsl::from_feet(1000.0), Qnh::Hpa(1033));
    assert!(
        (high.as_feet() - 460.825).abs() < 1e-6,
        "{}",
        high.as_feet()
    );
}

#[test]
fn density_altitude_worked_example() {
    // Field 1000 ft, QNH 1013 hPa, OAT 25 °C. Worked independently:
    //   PA      = 1000 + (1013.25 − 1013)·27.3 = 1006.825 ft = 306.880260 m
    //   ISA(PA) = 15 − 0.0065·306.880260       = 13.005278 °C
    //   ISA dev = 25 − 13.005278               = 11.994722 °C
    //   DA      = 1006.825 + 120·11.99472169   = 2446.191603 ft
    let da = density_altitude(MetersAmsl::from_feet(1000.0), Qnh::Hpa(1013), Celsius(25.0));
    assert!(
        (da.as_feet() - 2446.191603).abs() < 0.01,
        "{}",
        da.as_feet()
    );
}

#[test]
fn density_altitude_equals_pressure_altitude_at_isa() {
    // OAT exactly ISA at the pressure altitude ⇒ DA = PA.
    let pa = pressure_altitude(MetersAmsl::from_feet(0.0), Qnh::Hpa(1013));
    let da = density_altitude(
        MetersAmsl::from_feet(0.0),
        Qnh::Hpa(1013),
        isa_temperature(pa),
    );
    assert!((da.0 - pa.0).abs() < 1e-9);
}

#[test]
fn density_altitude_monotonicity() {
    let field = MetersAmsl::from_feet(1000.0);
    // Rises with temperature.
    let mut previous = f64::NEG_INFINITY;
    for temp in [-10.0, 0.0, 15.0, 30.0, 40.0] {
        let da = density_altitude(field, Qnh::Hpa(1013), Celsius(temp)).0;
        assert!(da > previous, "DA must rise with temperature");
        previous = da;
    }
    // Falls with rising QNH.
    previous = f64::INFINITY;
    for qnh in [990, 1003, 1013, 1023, 1033] {
        let da = density_altitude(field, Qnh::Hpa(qnh), Celsius(20.0)).0;
        assert!(da < previous, "DA must fall with rising QNH");
        previous = da;
    }
    // Rises with elevation.
    previous = f64::NEG_INFINITY;
    for feet in [0.0, 1000.0, 2000.0, 4000.0] {
        let da = density_altitude(MetersAmsl::from_feet(feet), Qnh::Hpa(1013), Celsius(20.0)).0;
        assert!(da > previous, "DA must rise with elevation");
        previous = da;
    }
}

// ------------------------------------------------------- distance chain

#[test]
fn takeoff_distance_worked_example() {
    // Base roll 300 m; DA = 2446.191603 ft (worked above); headwind 10 kt;
    // dry grass; 2 % upslope; safety factor 1.33. Chain:
    //   DA      : 1 + 0.10·2.446191603 = 1.2446192
    //   wind    : 1 − 0.10·(10/10)     = 0.90
    //   surface : 1.20   (grass)
    //   slope   : 1 + 0.10·2 = 1.20 (upslope is adverse on takeoff)
    //   300 · 1.2446192 · 0.90 · 1.20 · 1.20 · 1.33 = 643.597546 m
    let mut conditions = warm_conditions();
    conditions.headwind_component = Knots(10.0);
    conditions.surface = RunwaySurface::Grass;
    conditions.slope_percent = 2.0;
    let distance = takeoff_distance(&aircraft(), &conditions).unwrap();
    assert!((distance.0 - 643.597546).abs() < 0.01, "{}", distance.0);
}

#[test]
fn landing_distance_worked_example() {
    // Base roll 250 m; same DA; 5 kt tailwind; wet asphalt; 1.5 % downslope
    // (adverse on landing); safety factor 1.43. Chain:
    //   DA      : 1.2446192
    //   wind    : 1 + 0.40·(5/10) = 1.20
    //   surface : 1.0 (paved)   wet: 1.15
    //   slope   : 1 + 0.10·1.5 = 1.15
    //   250 · 1.2446192 · 1.20 · 1.15 · 1.15 · 1.43 = 706.137785 m
    let mut conditions = warm_conditions();
    conditions.headwind_component = Knots(-5.0);
    conditions.wet = true;
    conditions.slope_percent = -1.5;
    let distance = landing_distance(&aircraft(), &conditions).unwrap();
    assert!((distance.0 - 706.137785).abs() < 0.01, "{}", distance.0);
}

#[test]
fn favorable_slope_earns_no_credit() {
    let aircraft = aircraft();
    let level = warm_conditions();
    // Downslope on takeoff: no credit, same as level.
    let mut downhill = level;
    downhill.slope_percent = -2.0;
    assert_eq!(
        takeoff_distance(&aircraft, &downhill).unwrap(),
        takeoff_distance(&aircraft, &level).unwrap()
    );
    // Upslope on landing: no credit either.
    let mut uphill = level;
    uphill.slope_percent = 2.0;
    assert_eq!(
        landing_distance(&aircraft, &uphill).unwrap(),
        landing_distance(&aircraft, &level).unwrap()
    );
}

#[test]
fn below_isa_density_altitude_earns_no_credit() {
    // Cold high-pressure day: DA is well below 0 ft, clamped to 0 — the
    // distance is exactly base · safety factor: 300 · 1.33 = 399 m.
    let conditions = RunwayConditions {
        field_elevation: MetersAmsl::from_feet(0.0),
        qnh: Qnh::Hpa(1033),
        temperature: Celsius(-20.0),
        headwind_component: Knots(0.0),
        surface: RunwaySurface::Asphalt,
        slope_percent: 0.0,
        wet: false,
    };
    let distance = takeoff_distance(&aircraft(), &conditions).unwrap();
    assert!((distance.0 - 399.0).abs() < 1e-9, "{}", distance.0);
}

#[test]
fn distance_rises_with_density_altitude() {
    // Monotonicity through the chain: hotter ⇒ longer.
    let aircraft = aircraft();
    let mut previous = 0.0;
    for temp in [0.0, 10.0, 20.0, 30.0, 40.0] {
        let mut conditions = warm_conditions();
        conditions.temperature = Celsius(temp);
        let distance = takeoff_distance(&aircraft, &conditions).unwrap().0;
        assert!(distance > previous, "distance must rise with temperature");
        previous = distance;
    }
}

#[test]
fn unknown_surfaces_are_treated_as_unpaved() {
    let aircraft = aircraft();
    let paved = takeoff_distance(&aircraft, &warm_conditions()).unwrap();
    for surface in [
        RunwaySurface::Gravel,
        RunwaySurface::Snow,
        RunwaySurface::Unknown,
    ] {
        let mut conditions = warm_conditions();
        conditions.surface = surface;
        let distance = takeoff_distance(&aircraft, &conditions).unwrap();
        // The unpaved factor (+20 %) applies.
        assert!((distance.0 / paved.0 - 1.2).abs() < 1e-9, "{surface:?}");
    }
}

#[test]
fn unset_base_distances_error() {
    let blank = AircraftProfile::new(AircraftId::new("blank").unwrap());
    assert!(matches!(
        takeoff_distance(&blank, &warm_conditions()),
        Err(PerfError::InvalidProfile(_))
    ));
    assert!(matches!(
        landing_distance(&blank, &warm_conditions()),
        Err(PerfError::InvalidProfile(_))
    ));
}

// ------------------------------------------------ head/crosswind, margin

#[test]
fn wind_component_decomposition_signs() {
    // Runway 36 (000°), wind 090°/20 kt: pure crosswind from the right.
    let c = wind_components(DegreesTrue::new(360.0), DegreesTrue::new(90.0), Knots(20.0));
    assert!(c.headwind.0.abs() < 1e-9);
    assert!((c.crosswind.0 - 20.0).abs() < 1e-9);
    // Wind 270°/20 kt: from the left.
    let c = wind_components(
        DegreesTrue::new(360.0),
        DegreesTrue::new(270.0),
        Knots(20.0),
    );
    assert!((c.crosswind.0 - -20.0).abs() < 1e-9);
    // Runway 24 (240°), wind 300°/16 kt: Δ = 60° right of the nose.
    //   headwind  = 16·cos(60°) = 8.0 kt
    //   crosswind = 16·sin(60°) = 13.856406 kt (from the right)
    let c = wind_components(
        DegreesTrue::new(240.0),
        DegreesTrue::new(300.0),
        Knots(16.0),
    );
    assert!((c.headwind.0 - 8.0).abs() < 1e-9);
    assert!((c.crosswind.0 - 13.856406).abs() < 1e-5);
    // Wrap across north: runway 01 (010°), wind 350°/10 kt: Δ = −20°.
    //   headwind = 10·cos(20°) = 9.396926, crosswind = −10·sin(20°) = −3.420201.
    let c = wind_components(DegreesTrue::new(10.0), DegreesTrue::new(350.0), Knots(10.0));
    assert!((c.headwind.0 - 9.396926).abs() < 1e-5);
    assert!((c.crosswind.0 - -3.420201).abs() < 1e-5);
    // Direct tailwind: runway 36, wind 180°/12 kt.
    let c = wind_components(
        DegreesTrue::new(360.0),
        DegreesTrue::new(180.0),
        Knots(12.0),
    );
    assert!((c.headwind.0 - -12.0).abs() < 1e-9);
}

#[test]
fn runway_margin_readout() {
    let fits = runway_margin(Meters(600.0), Meters(800.0));
    assert_eq!(fits.margin, Meters(200.0));
    assert!((fits.ratio - 800.0 / 600.0).abs() < 1e-12);
    let short = runway_margin(Meters(600.0), Meters(480.0));
    assert_eq!(short.margin, Meters(-120.0));
    assert!((short.ratio - 0.8).abs() < 1e-12);
}

// ----------------------------------------------------------- plan_phases

// Reference leg: (50,10)→(51,10), one meridian degree on the R1 sphere =
// 111 195.080 m (see route tests). Climb gradient = 2.54 m/s ÷ 36.011111
// m/s ⇒ 14.177603 m along per m up; descent ⇒ 18.228346 m per m down.

#[test]
fn phases_climb_cruise_descent_worked_example() {
    // Cruise 1524 m (5000 ft), departure/destination at sea level.
    //   climb   : 1524·14.177603 = 21 606.667 m, 1524/2.54 = 600 s = 10 min,
    //             40 L/h · 1/6 h = 6.666667 L
    //   descent : 1524·18.228346 = 27 780.000 m ⇒ TOD at
    //             111 195.080 − 27 780.000 = 83 415.080 m; 10 min; 3.333333 L
    //   cruise  : 83 415.080 − 21 606.667 = 61 808.413 m = 33.373873 NM
    //             @ 110 kt = 18.203931 min; 35 L/h ⇒ 10.618960 L
    //   totals  : 38.203931 min, 20.618960 L
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(1524.0))),
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();

    let total = 111_195.080;
    assert_gap_free(&plan, total);
    assert_eq!(plan.segments.len(), 3);

    let climb = &plan.segments[0];
    assert_eq!(climb.kind, PhaseKind::Climb);
    assert!(
        (climb.end_along_track.0 - 21_606.667).abs() < 0.01,
        "{climb:?}"
    );
    assert_eq!(climb.start_altitude, MetersAmsl(0.0));
    assert_eq!(climb.end_altitude, MetersAmsl(1524.0));
    assert_eq!(climb.tas, Knots(70.0));
    assert!((climb.duration.0 - 10.0).abs() < 1e-9);
    assert!((climb.fuel.0 - 6.666667).abs() < 1e-5);

    let cruise = &plan.segments[1];
    assert_eq!(cruise.kind, PhaseKind::Cruise);
    assert_eq!(cruise.tas, Knots(110.0)); // first power setting by default
    assert!((cruise.duration.0 - 18.203931).abs() < 1e-4);
    assert!((cruise.fuel.0 - 10.618960).abs() < 1e-4);

    let descent = &plan.segments[2];
    assert_eq!(descent.kind, PhaseKind::Descent);
    assert!(
        (descent.start_along_track.0 - 83_415.080).abs() < 0.01,
        "{descent:?}"
    );
    assert!((descent.duration.0 - 10.0).abs() < 1e-9);
    assert!((descent.fuel.0 - 3.333333).abs() < 1e-5);

    // Markers: TOC ends the initial climb, TOD starts the final descent;
    // on a meridian leg latitude is linear in along-track distance:
    //   TOC lat = 50 + 21 606.667/111 195.080 = 50.194313
    //   TOD lat = 50 + 83 415.080/111 195.080 = 50.750169
    let toc = plan.toc.unwrap();
    assert!((toc.along_track.0 - 21_606.667).abs() < 0.01);
    assert_eq!(toc.altitude, MetersAmsl(1524.0));
    assert!((toc.position.lat() - 50.194313).abs() < 1e-4);
    let tod = plan.tod.unwrap();
    assert!((tod.along_track.0 - 83_415.080).abs() < 0.01);
    assert!((tod.position.lat() - 50.750169).abs() < 1e-4);

    assert!((plan.total_duration.0 - 38.203931).abs() < 1e-4);
    assert!((plan.total_fuel.0 - 20.618960).abs() < 1e-4);
}

#[test]
fn phases_cap_the_climb_when_toc_would_pass_the_destination() {
    // One tenth of a degree: 11 119.508 m — far too short to climb to
    // 1524 m (needs 21 606.667 m). The climb is capped where it meets the
    // descent line positioned back from the destination:
    //   apex h = 11 119.508 / (14.177603 + 18.228346) = 343.131686 m
    //   apex x = 343.131686 · 14.177603 = 4 864.785 m
    //   climb/descent each take 343.131686/2.54 = 135.0913 s = 2.251521 min
    let route = [wp(50.0, 10.0), wp(50.1, 10.0)];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(1524.0))),
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();

    let total = 11_119.508;
    assert_gap_free(&plan, total);
    assert_eq!(plan.segments.len(), 2);
    assert_eq!(plan.segments[0].kind, PhaseKind::Climb);
    assert_eq!(plan.segments[1].kind, PhaseKind::Descent);
    // Never reaches a cruise altitude — no TOC/TOD.
    assert_eq!(plan.toc, None);
    assert_eq!(plan.tod, None);

    let apex_x = plan.segments[0].end_along_track.0;
    let apex_h = plan.segments[0].end_altitude.0;
    assert!((apex_x - 4_864.785).abs() < 0.01, "{apex_x}");
    assert!((apex_h - 343.131686).abs() < 0.001, "{apex_h}");
    assert!((plan.segments[0].duration.0 - 2.251521).abs() < 1e-4);
    assert!((plan.segments[1].duration.0 - 2.251521).abs() < 1e-4);
}

#[test]
fn phases_step_climb_per_leg_altitudes() {
    // Two legs of 55 597.540 m each; leg 0 planned at 914.4 m (3000 ft),
    // leg 1 at 1524 m (5000 ft). Expected profile:
    //   climb 0→12 964.000 m (914.4·14.177603), 6 min
    //   cruise at 914.4 to the waypoint (55 597.540 m)
    //   step climb to 1524 ending at 55 597.540 + 609.6·14.177603
    //     = 64 240.205 m, 4 min
    //   cruise at 1524 to TOD (83 415.080 m), descent 10 min to the end.
    let route = [
        wp_alt(50.0, 10.0, 914.4),
        wp_alt(50.5, 10.0, 1524.0),
        wp(51.0, 10.0),
    ];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();

    assert_gap_free(&plan, 111_195.080);
    let kinds: Vec<PhaseKind> = plan.segments.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            PhaseKind::Climb,
            PhaseKind::Cruise,
            PhaseKind::Climb,
            PhaseKind::Cruise,
            PhaseKind::Descent
        ]
    );
    assert!((plan.segments[0].end_along_track.0 - 12_964.000).abs() < 0.01);
    assert!((plan.segments[1].end_along_track.0 - 55_597.540).abs() < 0.01);
    assert!((plan.segments[2].end_along_track.0 - 64_240.205).abs() < 0.01);
    assert!((plan.segments[3].end_along_track.0 - 83_415.080).abs() < 0.01);
    assert!((plan.segments[2].duration.0 - 4.0).abs() < 1e-9);

    // TOC marks the end of the *initial* climb; TOD the final descent.
    let toc = plan.toc.unwrap();
    assert!((toc.along_track.0 - 12_964.000).abs() < 0.01);
    assert_eq!(toc.altitude, MetersAmsl(914.4));
    let tod = plan.tod.unwrap();
    assert!((tod.along_track.0 - 83_415.080).abs() < 0.01);
    assert!((tod.altitude.0 - 1524.0).abs() < 0.011);
}

#[test]
fn phases_step_descent_starts_at_the_waypoint() {
    // Leg 0 at 1524 m, leg 1 at 914.4 m: the step descent begins at the
    // shared waypoint (55 597.540 m) and takes 609.6·18.228346 =
    // 11 112.000 m, ending at 66 709.540 m. The final descent from 914.4 m
    // starts at 111 195.080 − 914.4·18.228346 = 94 527.080 m.
    let route = [
        wp_alt(50.0, 10.0, 1524.0),
        wp_alt(50.5, 10.0, 914.4),
        wp(51.0, 10.0),
    ];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();

    assert_gap_free(&plan, 111_195.080);
    let kinds: Vec<PhaseKind> = plan.segments.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            PhaseKind::Climb,
            PhaseKind::Cruise,
            PhaseKind::Descent,
            PhaseKind::Cruise,
            PhaseKind::Descent
        ]
    );
    assert!((plan.segments[2].start_along_track.0 - 55_597.540).abs() < 0.01);
    assert!((plan.segments[2].end_along_track.0 - 66_709.540).abs() < 0.01);
    assert!((plan.segments[4].start_along_track.0 - 94_527.080).abs() < 0.01);
    // The TOD is the start of the *final* descent.
    let tod = plan.tod.unwrap();
    assert!((tod.along_track.0 - 94_527.080).abs() < 0.01);
    assert!((tod.altitude.0 - 914.4).abs() < 0.011);
}

#[test]
fn phases_final_climb_to_a_higher_destination() {
    // Cruise at 300 m toward a destination at 500 m elevation: the profile
    // ends with a climb positioned back from the route end:
    //   initial climb to 300 m ends at 300·14.177603 = 4 253.281 m
    //   final climb takes 200·14.177603 = 2 835.521 m, starting at
    //   111 195.080 − 2 835.521 = 108 359.559 m; 200/2.54 s = 1.312336 min
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(300.0))),
        MetersAmsl(0.0),
        MetersAmsl(500.0),
    )
    .unwrap();

    assert_gap_free(&plan, 111_195.080);
    let kinds: Vec<PhaseKind> = plan.segments.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [PhaseKind::Climb, PhaseKind::Cruise, PhaseKind::Climb]
    );
    assert!((plan.segments[0].end_along_track.0 - 4_253.281).abs() < 0.01);
    assert!((plan.segments[2].start_along_track.0 - 108_359.559).abs() < 0.01);
    assert!((plan.segments[2].duration.0 - 1.312336).abs() < 1e-5);
    // TOC exists (climb into cruise), but no TOD — the plan ends climbing.
    assert!(plan.toc.is_some());
    assert_eq!(plan.tod, None);
}

#[test]
fn phases_direct_descent_when_route_is_too_short() {
    // From a 1000 m field down to sea level in ~4.4 km: the modeled
    // descent needs 18 228.346 m — impossible. A single direct (steeper)
    // descent is emitted: 1000/2.54 s = 6.561680 min, 20 L/h ⇒ 2.187227 L.
    let route = [wp(50.0, 10.0), wp(50.04, 10.0)];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(1000.0))),
        MetersAmsl(1000.0),
        MetersAmsl(0.0),
    )
    .unwrap();

    assert_eq!(plan.segments.len(), 1);
    let segment = &plan.segments[0];
    assert_eq!(segment.kind, PhaseKind::Descent);
    assert_eq!(segment.start_altitude, MetersAmsl(1000.0));
    assert_eq!(segment.end_altitude, MetersAmsl(0.0));
    assert!((segment.duration.0 - 6.561680).abs() < 1e-5);
    assert!((segment.fuel.0 - 2.187227).abs() < 1e-5);
    assert_eq!(plan.toc, None);
    assert_eq!(plan.tod, None);
}

#[test]
fn phases_direct_climb_when_route_is_too_short() {
    // The mirror case: 0 → 1000 m over ~4.4 km needs 14 177.603 m of
    // climb. Direct climb: 6.561680 min, 40 L/h ⇒ 4.374453 L.
    let route = [wp(50.0, 10.0), wp(50.04, 10.0)];
    let plan = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(1000.0))),
        MetersAmsl(0.0),
        MetersAmsl(1000.0),
    )
    .unwrap();

    assert_eq!(plan.segments.len(), 1);
    assert_eq!(plan.segments[0].kind, PhaseKind::Climb);
    assert!((plan.segments[0].duration.0 - 6.561680).abs() < 1e-5);
    assert!((plan.segments[0].fuel.0 - 4.374453).abs() < 1e-5);
}

#[test]
fn phases_flat_route_is_pure_cruise_without_climb_data() {
    // Departure, cruise and destination all at 914.4 m: no transitions, so
    // missing climb/descent performance is never consulted.
    let mut profile = AircraftProfile::new(AircraftId::new("cruiser").unwrap());
    profile.performance.cruise_settings = vec![PowerSetting {
        name: "eco".into(),
        tas: Knots(100.0),
        fuel_flow: LitersPerHour(30.0),
    }];
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let plan = plan_phases(
        &route,
        &profile,
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(914.4))),
        MetersAmsl(914.4),
        MetersAmsl(914.4),
    )
    .unwrap();

    assert_eq!(plan.segments.len(), 1);
    let cruise = &plan.segments[0];
    assert_eq!(cruise.kind, PhaseKind::Cruise);
    assert_eq!(cruise.start_altitude, MetersAmsl(914.4));
    assert_eq!(cruise.end_altitude, MetersAmsl(914.4));
    // 60.040540 NM @ 100 kt = 36.024324 min.
    assert!((cruise.duration.0 - 36.024324).abs() < 1e-4);
    assert_eq!(plan.toc, None);
    assert_eq!(plan.tod, None);
}

#[test]
fn phases_power_setting_selection() {
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let cruise_alt = Some(PlannedAltitude::Amsl(MetersAmsl(1524.0)));
    // Named setting.
    let plan = plan_phases(
        &route,
        &aircraft(),
        Some("75 %"),
        cruise_alt,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    assert_eq!(plan.segments[1].tas, Knots(120.0));
    // Unknown name errors.
    let unknown = plan_phases(
        &route,
        &aircraft(),
        Some("85 %"),
        cruise_alt,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    );
    assert!(
        matches!(unknown, Err(PerfError::UnknownPowerSetting(ref name)) if name == "85 %"),
        "{unknown:?}"
    );
    // No settings at all.
    let blank = AircraftProfile::new(AircraftId::new("blank").unwrap());
    let none = plan_phases(
        &route,
        &blank,
        None,
        cruise_alt,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    );
    assert!(matches!(none, Err(PerfError::NoCruiseSetting)), "{none:?}");
}

#[test]
fn phases_missing_planned_altitude_errors_with_leg_index() {
    let route = [wp_alt(50.0, 10.0, 1524.0), wp(50.5, 10.0), wp(51.0, 10.0)];
    let result = plan_phases(
        &route,
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    );
    assert!(
        matches!(result, Err(PerfError::NoPlannedAltitude(1))),
        "{result:?}"
    );
}

#[test]
fn phases_missing_climb_performance_errors_when_needed() {
    let mut profile = aircraft();
    profile.performance.climb = ClimbPerformance::default(); // zeros
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let result = plan_phases(
        &route,
        &profile,
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl(1524.0))),
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    );
    assert!(
        matches!(result, Err(PerfError::InvalidProfile(_))),
        "{result:?}"
    );
}

#[test]
fn phases_flight_level_cruise_matches_equivalent_amsl() {
    // FL050 = 5000 ft = 1524 m: identical profile to the AMSL variant.
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let by_fl = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::FlightLevel(50)),
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    let by_amsl = plan_phases(
        &route,
        &aircraft(),
        None,
        Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(5000.0))),
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    assert_eq!(by_fl, by_amsl);
}

#[test]
fn phases_short_routes_yield_an_empty_plan() {
    let empty = plan_phases(
        &[],
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    assert!(empty.segments.is_empty());
    assert_eq!(empty.total_duration.0, 0.0);

    let single = plan_phases(
        &[wp(50.0, 10.0)],
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    assert!(single.segments.is_empty());

    // Coincident waypoints: zero-length route.
    let coincident = plan_phases(
        &[wp_alt(50.0, 10.0, 1000.0), wp(50.0, 10.0)],
        &aircraft(),
        None,
        None,
        MetersAmsl(0.0),
        MetersAmsl(0.0),
    )
    .unwrap();
    assert!(coincident.segments.is_empty());
}

#[test]
fn perf_types_serde_round_trip() {
    let components = WindComponents {
        headwind: Knots(8.0),
        crosswind: Knots(-3.5),
    };
    let json = serde_json::to_string(&components).unwrap();
    assert_eq!(
        serde_json::from_str::<WindComponents>(&json).unwrap(),
        components
    );

    let margin = runway_margin(Meters(600.0), Meters(800.0));
    let json = serde_json::to_string(&margin).unwrap();
    assert_eq!(serde_json::from_str::<RunwayMargin>(&json).unwrap(), margin);
}

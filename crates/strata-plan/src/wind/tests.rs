use std::cell::RefCell;

use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use strata_data::domain::{LatLon, MetersAmsl, PressureLevel};

use super::*;
use crate::flight::{FreePoint, ManualWind, PlannedAltitude, RoutePoint, RouteWaypoint};
use crate::sources::{MagvarSource, Provenance, SourceError, WindsAloft, WindsAloftSampler};
use crate::units::{Celsius, DegreesTrue, Knots, MagneticVariation};

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

fn wp(lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: None,
        position: ll(lat, lon),
    }))
}

fn amsl(meters: f64) -> PlannedAltitude {
    PlannedAltitude::Amsl(MetersAmsl(meters))
}

fn departure() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 14, 10, 0, 0).unwrap()
}

// ---------------------------------------------------------------- triangle

#[test]
fn triangle_textbook_quartering_headwind() {
    // TT 090°, TAS 100 kt, wind 045°/20 kt. Worked independently:
    //   Δ         = 045 − 090 = −45°
    //   crosswind = 20·sin(−45°) = −14.142136 kt  (from the left)
    //   headwind  = 20·cos(−45°) = +14.142136 kt
    //   WCA       = asin(−0.14142136)             = −8.130102°
    //   TH        = 90 − 8.130102                 = 81.869898°
    //   GS        = 100·√(1 − 0.02) − 14.142136
    //             = 98.994949 − 14.142136         = 84.852814 kt
    let t = solve_wind_triangle(
        DegreesTrue::new(90.0),
        Knots(100.0),
        DegreesTrue::new(45.0),
        Knots(20.0),
    )
    .unwrap();
    assert!(
        (t.wind_correction_angle_deg - -8.130102).abs() < 1e-4,
        "{t:?}"
    );
    assert!((t.true_heading.0 - 81.869898).abs() < 1e-4, "{t:?}");
    assert!((t.ground_speed.0 - 84.852814).abs() < 1e-4, "{t:?}");
}

#[test]
fn triangle_pure_crosswind_from_the_left() {
    // TT 360°, TAS 100 kt, wind 270°/30 kt (pure left crosswind):
    //   WCA = asin(30·sin(270°)/100) = asin(−0.3) = −17.457603°
    //   TH  = 360 − 17.457603 = 342.542397°
    //   GS  = 100·√(1 − 0.09) − 0 = 95.393920 kt
    let t = solve_wind_triangle(
        DegreesTrue::new(360.0),
        Knots(100.0),
        DegreesTrue::new(270.0),
        Knots(30.0),
    )
    .unwrap();
    assert!(
        (t.wind_correction_angle_deg - -17.457603).abs() < 1e-4,
        "{t:?}"
    );
    assert!((t.true_heading.0 - 342.542397).abs() < 1e-4, "{t:?}");
    assert!((t.ground_speed.0 - 95.393920).abs() < 1e-4, "{t:?}");
}

#[test]
fn triangle_wind_from_the_right_corrects_right() {
    // TT 000°, TAS 100 kt, wind 090°/20 kt: wind from the right ⇒ positive
    // WCA (correct right, into the wind).
    //   WCA = asin(0.2) = +11.536959°
    //   GS  = 100·√(1 − 0.04) = 97.979590 kt
    let t = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(100.0),
        DegreesTrue::new(90.0),
        Knots(20.0),
    )
    .unwrap();
    assert!(
        (t.wind_correction_angle_deg - 11.536959).abs() < 1e-4,
        "{t:?}"
    );
    assert!((t.true_heading.0 - 11.536959).abs() < 1e-4, "{t:?}");
    assert!((t.ground_speed.0 - 97.979590).abs() < 1e-4, "{t:?}");
}

#[test]
fn triangle_zero_wind() {
    let t = solve_wind_triangle(
        DegreesTrue::new(123.0),
        Knots(95.0),
        DegreesTrue::new(0.0),
        Knots(0.0),
    )
    .unwrap();
    assert_eq!(t.wind_correction_angle_deg, 0.0);
    assert_eq!(t.true_heading.0, 123.0);
    assert_eq!(t.ground_speed.0, 95.0);
}

#[test]
fn triangle_direct_headwind() {
    // TT 180°, wind 180°/30 kt, TAS 100 kt: GS = 70 kt, no correction.
    let t = solve_wind_triangle(
        DegreesTrue::new(180.0),
        Knots(100.0),
        DegreesTrue::new(180.0),
        Knots(30.0),
    )
    .unwrap();
    assert!(t.wind_correction_angle_deg.abs() < 1e-10);
    assert!((t.true_heading.0 - 180.0).abs() < 1e-10);
    assert!((t.ground_speed.0 - 70.0).abs() < 1e-9);
}

#[test]
fn triangle_direct_tailwind() {
    // TT 180°, wind 000°/30 kt, TAS 100 kt: GS = 130 kt.
    let t = solve_wind_triangle(
        DegreesTrue::new(180.0),
        Knots(100.0),
        DegreesTrue::new(0.0),
        Knots(30.0),
    )
    .unwrap();
    assert!(t.wind_correction_angle_deg.abs() < 1e-10);
    assert!((t.ground_speed.0 - 130.0).abs() < 1e-9);
}

#[test]
fn triangle_angles_wrap_across_north() {
    // TT 350°, TAS 100 kt, wind 010°/20 kt:
    //   Δ = 010 − 350 = +20° (wind 20° right of track)
    //   crosswind = 20·sin(20°) = +6.840403 kt
    //   WCA = asin(0.06840403) = +3.922271°
    //   TH  = 350 + 3.922271 = 353.922271°
    //   GS  = 100·cos(3.922271°) − 20·cos(20°)
    //       = 99.765769 − 18.793852 = 80.971917 kt
    let t = solve_wind_triangle(
        DegreesTrue::new(350.0),
        Knots(100.0),
        DegreesTrue::new(10.0),
        Knots(20.0),
    )
    .unwrap();
    assert!(
        (t.wind_correction_angle_deg - 3.922271).abs() < 1e-4,
        "{t:?}"
    );
    assert!((t.true_heading.0 - 353.922271).abs() < 1e-4, "{t:?}");
    assert!((t.ground_speed.0 - 80.971917).abs() < 1e-4, "{t:?}");
}

#[test]
fn triangle_unsolvable_when_crosswind_exceeds_tas() {
    let result = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(20.0),
        DegreesTrue::new(90.0),
        Knots(30.0),
    );
    assert!(
        matches!(result, Err(WindError::Unsolvable { .. })),
        "{result:?}"
    );
}

#[test]
fn triangle_unsolvable_without_positive_ground_speed() {
    // Headwind equal to TAS: GS = 0 — track held but no progress.
    let equal = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(50.0),
        DegreesTrue::new(0.0),
        Knots(50.0),
    );
    assert!(
        matches!(equal, Err(WindError::Unsolvable { .. })),
        "{equal:?}"
    );
    // Headwind above TAS: GS would be negative.
    let stronger = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(50.0),
        DegreesTrue::new(0.0),
        Knots(60.0),
    );
    assert!(
        matches!(stronger, Err(WindError::Unsolvable { .. })),
        "{stronger:?}"
    );
}

#[test]
fn triangle_strong_pure_tailwind_is_solvable() {
    // Wind stronger than TAS but from directly behind: the track is held
    // and GS = 50 + 80 = 130 kt.
    let t = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(50.0),
        DegreesTrue::new(180.0),
        Knots(80.0),
    )
    .unwrap();
    assert!(t.wind_correction_angle_deg.abs() < 1e-10);
    assert!((t.ground_speed.0 - 130.0).abs() < 1e-9);
}

#[test]
fn triangle_rejects_degenerate_inputs() {
    let zero_tas = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(0.0),
        DegreesTrue::new(90.0),
        Knots(10.0),
    );
    assert!(matches!(zero_tas, Err(WindError::Unsolvable { .. })));
    let negative_wind = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(100.0),
        DegreesTrue::new(90.0),
        Knots(-10.0),
    );
    assert!(matches!(negative_wind, Err(WindError::Unsolvable { .. })));
    let nan = solve_wind_triangle(
        DegreesTrue::new(0.0),
        Knots(f64::NAN),
        DegreesTrue::new(90.0),
        Knots(10.0),
    );
    assert!(matches!(nan, Err(WindError::Unsolvable { .. })));
}

// ------------------------------------------------------------ magnetic_at

struct RecordingMagvar {
    variation: f64,
    calls: RefCell<Vec<(LatLon, NaiveDate)>>,
}

impl RecordingMagvar {
    fn new(variation: f64) -> Self {
        Self {
            variation,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl MagvarSource for RecordingMagvar {
    fn magvar(&self, p: LatLon, date: NaiveDate) -> Result<MagneticVariation, SourceError> {
        self.calls.borrow_mut().push((p, date));
        Ok(MagneticVariation(self.variation))
    }
}

#[test]
fn magnetic_at_east_variation_decreases() {
    // "East is least": true 100° with 4°E variation → magnetic 096°.
    let source = RecordingMagvar::new(4.0);
    let midpoint = ll(50.5, 10.0);
    let date = NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
    let m = magnetic_at(DegreesTrue::new(100.0), midpoint, date, &source).unwrap();
    assert!((m.0 - 96.0).abs() < 1e-12);
    // The source was asked exactly at the leg midpoint and flight date.
    let calls = source.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, midpoint);
    assert_eq!(calls[0].1, date);
}

#[test]
fn magnetic_at_west_variation_increases() {
    // True 100° with 2°W (−2 east-positive) variation → magnetic 102°.
    let source = RecordingMagvar::new(-2.0);
    let date = NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
    let m = magnetic_at(DegreesTrue::new(100.0), ll(50.0, 10.0), date, &source).unwrap();
    assert!((m.0 - 102.0).abs() < 1e-12);
}

// -------------------------------------------------------------- leg_winds

struct RecordingSampler {
    wind: Option<WindsAloft>,
    calls: RefCell<Vec<(LatLon, MetersAmsl, DateTime<Utc>)>>,
}

impl RecordingSampler {
    fn new(wind: Option<WindsAloft>) -> Self {
        Self {
            wind,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(LatLon, MetersAmsl, DateTime<Utc>)> {
        self.calls.borrow().clone()
    }
}

impl WindsAloftSampler for RecordingSampler {
    fn sample(
        &self,
        position: LatLon,
        altitude: MetersAmsl,
        valid_time: DateTime<Utc>,
    ) -> Result<Option<WindsAloft>, SourceError> {
        self.calls
            .borrow_mut()
            .push((position, altitude, valid_time));
        Ok(self.wind)
    }
}

struct FailingSampler;

impl WindsAloftSampler for FailingSampler {
    fn sample(
        &self,
        _: LatLon,
        _: MetersAmsl,
        _: DateTime<Utc>,
    ) -> Result<Option<WindsAloft>, SourceError> {
        Err(SourceError::new("grid unavailable"))
    }
}

fn westerly(speed: f64) -> WindsAloft {
    WindsAloft {
        direction: DegreesTrue::new(270.0),
        speed: Knots(speed),
        temperature: Celsius(2.0),
        temperature_provenance: Provenance::Real,
    }
}

#[test]
fn manual_override_beats_sampler() {
    // Two meridian legs; the first carries a manual wind override.
    let mut first = wp(50.0, 10.0);
    first.leg_wind = Some(ManualWind {
        direction: DegreesTrue::new(90.0),
        speed: Knots(10.0),
    });
    let route = [first, wp(51.0, 10.0), wp(52.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(30.0)));

    let winds = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    )
    .unwrap();

    assert_eq!(winds.len(), 2);
    // Leg 0: the override, never sampled. ISA temperature at 1524 m:
    // 15 − 0.0065·1524 = 5.094 °C.
    assert_eq!(winds[0].origin, LegWindOrigin::Manual);
    assert_eq!(winds[0].wind.direction.0, 90.0);
    assert_eq!(winds[0].wind.speed.0, 10.0);
    assert!((winds[0].wind.temperature.0 - 5.094).abs() < 1e-9);
    // Leg 1: sampled (temperature comes from the model).
    assert_eq!(winds[1].origin, LegWindOrigin::Sampled);
    assert_eq!(winds[1].wind.direction.0, 270.0);
    assert_eq!(winds[1].wind.speed.0, 30.0);
    assert_eq!(winds[1].wind.temperature.0, 2.0);
    // Only the second leg hit the sampler.
    assert_eq!(sampler.calls().len(), 1);
}

#[test]
fn samples_at_midpoint_altitude_and_estimated_time() {
    // One leg (50,10)→(51,10): 111 195.080 m = 60.040540 NM. At TAS 120 kt
    // the half-leg ETE is 60.040540/120/2 h = 15.010135 min, so the sample
    // time is 10:15:00.6Z. The midpoint of a meridian leg is lat 50.5.
    let mut first = wp(50.0, 10.0);
    first.leg_altitude = Some(amsl(914.4));
    let route = [first, wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(0.0)));

    leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(120.0),
        &sampler,
    )
    .unwrap();

    let calls = sampler.calls();
    assert_eq!(calls.len(), 1);
    let (position, altitude, time) = calls[0];
    assert!((position.lat() - 50.5).abs() < 1e-9);
    assert!((position.lon() - 10.0).abs() < 1e-9);
    // The leg's own altitude wins over the flight cruise altitude.
    assert_eq!(altitude, MetersAmsl(914.4));
    let expected = departure() + chrono::Duration::milliseconds((15.010135 * 60_000.0) as i64);
    assert!((time - expected).num_milliseconds().abs() < 1000, "{time}");
}

#[test]
fn passage_time_accumulates_solved_ground_speed() {
    // Two meridian legs of 60.040540 NM each, TAS 100 kt, wind 360°/50 kt
    // (direct headwind on a 000° track): GS = 50 kt.
    //   Leg 0 sample time = 10:00 + half ETE @ TAS = 10:00 + 18.012162 min
    //   Leg 0 ETE @ GS 50 = 60.040540/50 h        = 72.048648 min
    //   Leg 1 sample time = 10:00 + 72.048648 + 18.012162 = 10:00 + 90.060810 min
    let route = [wp(50.0, 10.0), wp(51.0, 10.0), wp(52.0, 10.0)];
    let headwind = WindsAloft {
        direction: DegreesTrue::new(360.0),
        speed: Knots(50.0),
        temperature: Celsius(0.0),
        temperature_provenance: Provenance::Real,
    };
    let sampler = RecordingSampler::new(Some(headwind));

    let winds = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    )
    .unwrap();
    assert!((winds[0].triangle.ground_speed.0 - 50.0).abs() < 1e-6);

    let calls = sampler.calls();
    assert_eq!(calls.len(), 2);
    let first = departure() + chrono::Duration::milliseconds((18.012162 * 60_000.0) as i64);
    let second = departure() + chrono::Duration::milliseconds((90.060810 * 60_000.0) as i64);
    assert!(
        (calls[0].2 - first).num_milliseconds().abs() < 1000,
        "{:?}",
        calls[0].2
    );
    assert!(
        (calls[1].2 - second).num_milliseconds().abs() < 1000,
        "{:?}",
        calls[1].2
    );
}

#[test]
fn sampled_wind_solves_the_triangle() {
    // Meridian track 000°, wind 270°/30 kt, TAS 100 kt — the same numbers
    // as `triangle_pure_crosswind_from_the_left`.
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(30.0)));
    let winds = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    )
    .unwrap();
    let t = winds[0].triangle;
    assert!((t.wind_correction_angle_deg - -17.457603).abs() < 1e-4);
    assert!((t.ground_speed.0 - 95.393920).abs() < 1e-4);
}

#[test]
fn calm_isa_fallback_outside_model_coverage() {
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(None);
    let winds = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    )
    .unwrap();
    assert_eq!(sampler.calls().len(), 1);
    assert_eq!(winds[0].origin, LegWindOrigin::IsaFallback);
    assert_eq!(winds[0].wind.speed.0, 0.0);
    assert!((winds[0].wind.temperature.0 - 5.094).abs() < 1e-9);
    assert_eq!(winds[0].wind.temperature_provenance, Provenance::Isa);
    assert_eq!(winds[0].triangle.ground_speed.0, 100.0);
}

#[test]
fn calm_fallback_without_departure_time_never_samples() {
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(30.0)));
    let winds = leg_winds(&route, Some(amsl(1524.0)), None, Knots(100.0), &sampler).unwrap();
    assert!(sampler.calls().is_empty());
    assert_eq!(winds[0].wind.speed.0, 0.0);
    assert_eq!(winds[0].origin, LegWindOrigin::IsaFallback);
}

#[test]
fn calm_fallback_without_any_planned_altitude() {
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(30.0)));
    let winds = leg_winds(&route, None, Some(departure()), Knots(100.0), &sampler).unwrap();
    assert!(sampler.calls().is_empty());
    assert_eq!(winds[0].wind.speed.0, 0.0);
    // ISA temperature falls back to sea level: 15 °C.
    assert_eq!(winds[0].wind.temperature.0, 15.0);
}

#[test]
fn flight_level_converts_to_pressure_altitude_feet() {
    // FL100 → 10 000 ft → 3048 m (treated as AMSL, documented).
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(0.0)));
    leg_winds(
        &route,
        Some(PlannedAltitude::FlightLevel(100)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    )
    .unwrap();
    assert!((sampler.calls()[0].1.0 - 3048.0).abs() < 1e-9);
}

#[test]
fn sampler_errors_propagate() {
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let result = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &FailingSampler,
    );
    assert!(matches!(result, Err(WindError::Source(_))), "{result:?}");
}

#[test]
fn unsolvable_leg_propagates() {
    // 200 kt pure crosswind at TAS 100 kt.
    let route = [wp(50.0, 10.0), wp(51.0, 10.0)];
    let sampler = RecordingSampler::new(Some(westerly(200.0)));
    let result = leg_winds(
        &route,
        Some(amsl(1524.0)),
        Some(departure()),
        Knots(100.0),
        &sampler,
    );
    assert!(
        matches!(result, Err(WindError::Unsolvable { .. })),
        "{result:?}"
    );
}

#[test]
fn empty_and_single_point_routes_have_no_legs() {
    let sampler = RecordingSampler::new(None);
    assert!(
        leg_winds(
            &[],
            Some(amsl(1524.0)),
            Some(departure()),
            Knots(100.0),
            &sampler
        )
        .unwrap()
        .is_empty()
    );
    let single = [wp(50.0, 10.0)];
    assert!(
        leg_winds(
            &single,
            Some(amsl(1524.0)),
            Some(departure()),
            Knots(100.0),
            &sampler
        )
        .unwrap()
        .is_empty()
    );
}

// ------------------------------------------------- vertical interpolation

fn level_sample(level: PressureLevel, u: f64, v: f64, t: f64) -> PressureLevelSample {
    PressureLevelSample {
        level,
        wind_u: u,
        wind_v: v,
        temperature: Celsius(t),
        temperature_provenance: Provenance::Real,
    }
}

fn four_levels() -> Vec<PressureLevelSample> {
    vec![
        level_sample(PressureLevel::P950, 0.0, -5.0, 10.0),
        level_sample(PressureLevel::P850, 10.0, 0.0, 5.0),
        level_sample(PressureLevel::P700, 20.0, 10.0, -5.0),
        level_sample(PressureLevel::P500, 30.0, 30.0, -20.0),
    ]
}

#[test]
fn interpolation_is_exact_on_a_level() {
    // At the 850 hPa ISA altitude the 850 hPa values come out unchanged:
    // u=10, v=0 m/s → blows toward east ⇒ from 270°, 10 m/s = 19.438445 kt.
    let aloft = interpolate_levels(&four_levels(), PressureLevel::P850.isa_altitude()).unwrap();
    assert!((aloft.direction.0 - 270.0).abs() < 1e-9);
    assert!((aloft.speed.0 - 19.438445).abs() < 1e-5);
    assert!((aloft.temperature.0 - 5.0).abs() < 1e-9);
}

#[test]
fn interpolation_midway_between_levels() {
    // Halfway (in altitude) between 850 and 700 hPa the components are the
    // arithmetic means: u = 15, v = 5 m/s, t = 0 °C.
    //   speed = √(15² + 5²) = 15.811388 m/s = 30.734872 kt
    //   from  = atan2(−15, −5) = 251.565051°
    let h850 = PressureLevel::P850.isa_altitude().0;
    let h700 = PressureLevel::P700.isa_altitude().0;
    let aloft = interpolate_levels(&four_levels(), MetersAmsl((h850 + h700) / 2.0)).unwrap();
    assert!((aloft.direction.0 - 251.565051).abs() < 1e-5, "{aloft:?}");
    assert!((aloft.speed.0 - 30.734872).abs() < 1e-5, "{aloft:?}");
    assert!((aloft.temperature.0 - 0.0).abs() < 1e-9);
}

#[test]
fn interpolation_clamps_below_and_above_the_span() {
    // Below the 950 hPa ISA altitude (~540 m): the 950 values, unchanged.
    let low = interpolate_levels(&four_levels(), MetersAmsl(0.0)).unwrap();
    assert!((low.direction.0 - 0.0).abs() < 1e-9); // v=−5 ⇒ from north
    assert!((low.temperature.0 - 10.0).abs() < 1e-9);
    // Above the 500 hPa ISA altitude (~5574 m): the 500 values.
    let high = interpolate_levels(&four_levels(), MetersAmsl(8000.0)).unwrap();
    assert!((high.temperature.0 - -20.0).abs() < 1e-9);
}

#[test]
fn interpolation_handles_unsorted_and_duplicate_levels() {
    let mut samples = four_levels();
    samples.reverse();
    // A duplicate 850 hPa with different values: the first occurrence in
    // input order wins (after the reverse that is still the u=10 sample,
    // because dedup happens before sorting).
    samples.push(level_sample(PressureLevel::P850, -99.0, -99.0, -99.0));
    let aloft = interpolate_levels(&samples, PressureLevel::P850.isa_altitude()).unwrap();
    assert!((aloft.temperature.0 - 5.0).abs() < 1e-9);
}

#[test]
fn interpolation_single_level_is_constant() {
    let samples = [level_sample(PressureLevel::P850, 10.0, 0.0, 5.0)];
    for altitude in [0.0, 1457.0, 9000.0] {
        let aloft = interpolate_levels(&samples, MetersAmsl(altitude)).unwrap();
        assert!((aloft.speed.0 - 19.438445).abs() < 1e-5);
    }
}

#[test]
fn interpolated_temperature_provenance_is_real_only_when_both_levels_are() {
    // All-real levels: real everywhere, including clamped ends.
    let real = four_levels();
    let h850 = PressureLevel::P850.isa_altitude().0;
    let h700 = PressureLevel::P700.isa_altitude().0;
    let mid = MetersAmsl((h850 + h700) / 2.0);
    assert_eq!(
        interpolate_levels(&real, mid)
            .unwrap()
            .temperature_provenance,
        Provenance::Real
    );
    assert_eq!(
        interpolate_levels(&real, MetersAmsl(0.0))
            .unwrap()
            .temperature_provenance,
        Provenance::Real,
        "clamped below the span inherits the level's provenance"
    );

    // The 700 hPa temperature grid was missing (ISA-pinned level): every
    // altitude touching that level is honestly Isa, the 950/850 band stays
    // real.
    let mut mixed = four_levels();
    mixed[2].temperature_provenance = Provenance::Isa;
    assert_eq!(
        interpolate_levels(&mixed, mid)
            .unwrap()
            .temperature_provenance,
        Provenance::Isa,
        "a value mixed from real and ISA levels is not Real"
    );
    let h950 = PressureLevel::P950.isa_altitude().0;
    assert_eq!(
        interpolate_levels(&mixed, MetersAmsl((h950 + h850) / 2.0))
            .unwrap()
            .temperature_provenance,
        Provenance::Real
    );
    assert_eq!(
        interpolate_levels(&mixed, PressureLevel::P700.isa_altitude())
            .unwrap()
            .temperature_provenance,
        Provenance::Isa,
        "exactly on the ISA-pinned level"
    );
}

#[test]
fn interpolation_of_nothing_is_none() {
    assert_eq!(interpolate_levels(&[], MetersAmsl(1000.0)), None);
}

#[test]
fn wind_components_to_direction_and_speed() {
    // u=−5 (toward west) ⇒ blows from the east: 090°, 5 m/s = 9.719222 kt.
    let (dir, speed) = wind_from_components(-5.0, 0.0);
    assert!((dir.0 - 90.0).abs() < 1e-9);
    assert!((speed.0 - 9.719222).abs() < 1e-5);
    // v=−10 (toward south) ⇒ from the north: 000°.
    let (dir, speed) = wind_from_components(0.0, -10.0);
    assert!((dir.0 - 0.0).abs() < 1e-9);
    assert!((speed.0 - 19.438445).abs() < 1e-5);
    // u=3, v=−4 ⇒ toward 143.130102° ⇒ from 323.130102°, 5 m/s.
    let (dir, speed) = wind_from_components(3.0, -4.0);
    assert!((dir.0 - 323.130102).abs() < 1e-5);
    assert!((speed.0 - 9.719222).abs() < 1e-5);
    // Calm: 0° by convention.
    let (dir, speed) = wind_from_components(0.0, 0.0);
    assert_eq!(dir.0, 0.0);
    assert_eq!(speed.0, 0.0);
}

#[test]
fn pressure_level_sample_serde_round_trip() {
    let sample = level_sample(PressureLevel::P700, 1.5, -2.5, -3.0);
    let json = serde_json::to_string(&sample).unwrap();
    let back: PressureLevelSample = serde_json::from_str(&json).unwrap();
    assert_eq!(back, sample);
}

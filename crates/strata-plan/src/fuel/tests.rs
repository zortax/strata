use strata_data::domain::{Meters, MetersAmsl};

use super::*;
use crate::aircraft::{AircraftId, PowerSetting};
use crate::perf::PhaseSegment;
use crate::units::Knots;

#[track_caller]
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

/// An invented trainer: taxi flow 6 L/h; cruise "65 %" at 32 L/h (the
/// default pick) and "55 %" at 26 L/h.
fn trainer() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("trainer").unwrap());
    profile.performance.taxi_fuel_flow = LitersPerHour(6.0);
    profile.performance.cruise_settings = vec![
        PowerSetting {
            name: "65 %".to_owned(),
            tas: Knots(108.0),
            fuel_flow: LitersPerHour(32.0),
        },
        PowerSetting {
            name: "55 %".to_owned(),
            tas: Knots(100.0),
            fuel_flow: LitersPerHour(26.0),
        },
    ];
    profile
}

/// A phase segment where only kind/duration/fuel matter to fuel math.
fn seg(kind: PhaseKind, minutes: f64, fuel: f64) -> PhaseSegment {
    PhaseSegment {
        kind,
        start_along_track: Meters(0.0),
        end_along_track: Meters(0.0),
        start_altitude: MetersAmsl(0.0),
        end_altitude: MetersAmsl(0.0),
        tas: Knots(100.0),
        duration: Minutes(minutes),
        fuel: Liters(fuel),
    }
}

fn plan(segments: Vec<PhaseSegment>) -> PhasePlan {
    let total_duration = Minutes(segments.iter().map(|s| s.duration.0).sum());
    let total_fuel = Liters(segments.iter().map(|s| s.fuel.0).sum());
    PhasePlan {
        segments,
        toc: None,
        tod: None,
        total_duration,
        total_fuel,
    }
}

/// Climb 10 min / 8 L, cruise 90 min / 45 L (= 30 L/h — deliberately *not*
/// a profile cruise-table flow, proving the reserve flow is derived from
/// the plan), descent 12 min / 7 L. Trip total: 60 L in 112 min.
fn trip_plan() -> PhasePlan {
    plan(vec![
        seg(PhaseKind::Climb, 10.0, 8.0),
        seg(PhaseKind::Cruise, 90.0, 45.0),
        seg(PhaseKind::Descent, 12.0, 7.0),
    ])
}

// ---------------------------------------------------------------------------
// Worked ladder, default NCO-template policy (10 min taxi, 5 % contingency,
// 30 min final reserve, 0 extra). By hand:
//
//   taxi          10 min × 6 L/h          =  1.00 L
//   trip          from the phase plan     = 60.00 L
//   contingency   5 % × 60 L              =  3.00 L
//   alternate     (none)                  =  0.00 L
//   final reserve 30 min × 45 L/1.5 h     = 15.00 L   (plan cruise flow 30 L/h)
//   extra                                 =  0.00 L
//   ─ minimum required                    = 79.00 L
//   loaded 100 L → margin = 100 − 79      = 21.00 L
// ---------------------------------------------------------------------------

#[test]
fn worked_ladder_default_policy() {
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &trainer(),
        None,
        &trip_plan(),
        None,
        Liters(100.0),
    )
    .unwrap();

    assert_close(ladder.taxi.0, 1.0);
    assert_close(ladder.trip.0, 60.0);
    assert_close(ladder.contingency.0, 3.0);
    assert_close(ladder.alternate.0, 0.0);
    assert_close(ladder.final_reserve.0, 15.0);
    assert_close(ladder.extra.0, 0.0);
    assert_close(ladder.minimum_required.0, 79.0);
    assert_close(ladder.loaded.0, 100.0);
    assert_close(ladder.margin.0, 21.0);
}

#[test]
fn alternate_plan_adds_its_trip_fuel() {
    // Alternate leg: climb 3 min / 2 L + cruise 20 min / 10 L = 12 L.
    // Minimum becomes 79 + 12 = 91 L; margin 100 − 91 = 9 L. The alternate
    // plan's cruise flow (30 L/h here too) must NOT change the reserve —
    // the reserve flow comes from the *trip* plan.
    let alternate = plan(vec![
        seg(PhaseKind::Climb, 3.0, 2.0),
        seg(PhaseKind::Cruise, 20.0, 10.0),
    ]);
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &trainer(),
        None,
        &trip_plan(),
        Some(&alternate),
        Liters(100.0),
    )
    .unwrap();

    assert_close(ladder.alternate.0, 12.0);
    assert_close(ladder.final_reserve.0, 15.0);
    assert_close(ladder.minimum_required.0, 91.0);
    assert_close(ladder.margin.0, 9.0);
}

#[test]
fn contingency_switches_between_percentage_and_fixed() {
    // 5 % of the 60 L trip = 3 L vs a fixed 10 L floor.
    let percent = FuelPolicy {
        contingency: Contingency::PercentOfTrip(5.0),
        ..FuelPolicy::default()
    };
    let fixed = FuelPolicy {
        contingency: Contingency::Fixed(Liters(10.0)),
        ..FuelPolicy::default()
    };

    let percent_ladder = compute_fuel_ladder(
        &percent,
        &trainer(),
        None,
        &trip_plan(),
        None,
        Liters(100.0),
    )
    .unwrap();
    let fixed_ladder =
        compute_fuel_ladder(&fixed, &trainer(), None, &trip_plan(), None, Liters(100.0)).unwrap();

    assert_close(percent_ladder.contingency.0, 3.0);
    assert_close(fixed_ladder.contingency.0, 10.0);
    assert_close(percent_ladder.minimum_required.0, 79.0);
    assert_close(fixed_ladder.minimum_required.0, 86.0);
}

#[test]
fn under_fueled_margin_is_negative() {
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &trainer(),
        None,
        &trip_plan(),
        None,
        Liters(50.0),
    )
    .unwrap();
    assert_close(ladder.margin.0, -29.0);
}

#[test]
fn reserve_flow_falls_back_to_first_cruise_setting() {
    // No cruise segment (climb straight into the descent): the reserve uses
    // the profile's first cruise setting (32 L/h → 16 L in 30 min).
    //   taxi 1 + trip 15 + contingency 0.75 + reserve 16 = 32.75 L
    let no_cruise = plan(vec![
        seg(PhaseKind::Climb, 15.0, 10.0),
        seg(PhaseKind::Descent, 10.0, 5.0),
    ]);
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &trainer(),
        None,
        &no_cruise,
        None,
        Liters(100.0),
    )
    .unwrap();
    assert_close(ladder.final_reserve.0, 16.0);
    assert_close(ladder.minimum_required.0, 32.75);
}

#[test]
fn reserve_flow_falls_back_to_selected_setting_when_the_trip_never_cruises() {
    let no_cruise = plan(vec![
        seg(PhaseKind::Climb, 15.0, 10.0),
        seg(PhaseKind::Descent, 10.0, 5.0),
    ]);
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &trainer(),
        Some("55 %"),
        &no_cruise,
        None,
        Liters(100.0),
    )
    .unwrap();
    assert_close(ladder.final_reserve.0, 13.0);
    assert_close(ladder.minimum_required.0, 29.75);
}

#[test]
fn unresolvable_reserve_flow_errors() {
    // No cruise segment and an empty cruise table: the 30 min reserve has
    // no flow to price at.
    let mut profile = trainer();
    profile.performance.cruise_settings.clear();
    let no_cruise = plan(vec![seg(PhaseKind::Climb, 15.0, 10.0)]);
    let err = compute_fuel_ladder(
        &FuelPolicy::default(),
        &profile,
        None,
        &no_cruise,
        None,
        Liters(100.0),
    )
    .unwrap_err();
    assert!(matches!(err, FuelError::MissingData(_)));

    // A zero-fuel cruise segment (placeholder flow 0) does not count either.
    let zero_flow_cruise = plan(vec![seg(PhaseKind::Cruise, 60.0, 0.0)]);
    let err = compute_fuel_ladder(
        &FuelPolicy::default(),
        &profile,
        None,
        &zero_flow_cruise,
        None,
        Liters(100.0),
    )
    .unwrap_err();
    assert!(matches!(err, FuelError::MissingData(_)));
}

#[test]
fn zero_reserve_needs_no_cruise_flow() {
    let mut profile = trainer();
    profile.performance.cruise_settings.clear();
    let policy = FuelPolicy {
        final_reserve: Minutes(0.0),
        ..FuelPolicy::default()
    };
    let no_cruise = plan(vec![seg(PhaseKind::Climb, 15.0, 10.0)]);
    let ladder =
        compute_fuel_ladder(&policy, &profile, None, &no_cruise, None, Liters(100.0)).unwrap();
    assert_close(ladder.final_reserve.0, 0.0);
}

#[test]
fn negative_policy_values_clamp_to_zero() {
    // Malformed (hand-edited) policy values must never reduce the minimum:
    // every clamped rung is 0, so minimum == trip fuel.
    let policy = FuelPolicy {
        taxi: Minutes(-5.0),
        contingency: Contingency::PercentOfTrip(-5.0),
        final_reserve: Minutes(-10.0),
        extra: Liters(-2.0),
    };
    let ladder =
        compute_fuel_ladder(&policy, &trainer(), None, &trip_plan(), None, Liters(100.0)).unwrap();
    assert_close(ladder.taxi.0, 0.0);
    assert_close(ladder.contingency.0, 0.0);
    assert_close(ladder.final_reserve.0, 0.0);
    assert_close(ladder.extra.0, 0.0);
    assert_close(ladder.minimum_required.0, 60.0);

    let fixed = FuelPolicy {
        contingency: Contingency::Fixed(Liters(-3.0)),
        ..policy
    };
    let ladder =
        compute_fuel_ladder(&fixed, &trainer(), None, &trip_plan(), None, Liters(100.0)).unwrap();
    assert_close(ladder.contingency.0, 0.0);
}

#[test]
fn extra_fuel_raises_the_minimum() {
    let policy = FuelPolicy {
        extra: Liters(8.0),
        ..FuelPolicy::default()
    };
    let ladder =
        compute_fuel_ladder(&policy, &trainer(), None, &trip_plan(), None, Liters(100.0)).unwrap();
    assert_close(ladder.extra.0, 8.0);
    assert_close(ladder.minimum_required.0, 87.0);
}

// --- endurance ---------------------------------------------------------------

#[test]
fn endurance_uses_first_setting_by_default() {
    // 100 L at 32 L/h = 3.125 h = 187.5 min.
    let minutes = endurance(&trainer(), None, Liters(100.0)).unwrap();
    assert_close(minutes.0, 187.5);
}

#[test]
fn endurance_uses_named_setting() {
    // 100 L at 26 L/h = 3.846153… h = 230.769230… min (= 100/26·60).
    let minutes = endurance(&trainer(), Some("55 %"), Liters(100.0)).unwrap();
    assert_close(minutes.0, 100.0 / 26.0 * 60.0);
}

#[test]
fn endurance_unknown_setting_errors() {
    let err = endurance(&trainer(), Some("99 %"), Liters(100.0)).unwrap_err();
    assert!(matches!(err, FuelError::UnknownPowerSetting(name) if name == "99 %"));
}

#[test]
fn endurance_guards_zero_flow() {
    // Placeholder profile with a 0 L/h setting: an error, never a division
    // by zero / infinite endurance.
    let mut profile = trainer();
    profile.performance.cruise_settings[0].fuel_flow = LitersPerHour(0.0);
    let err = endurance(&profile, None, Liters(100.0)).unwrap_err();
    assert!(matches!(err, FuelError::MissingData(_)));
    let err = endurance(&profile, Some("65 %"), Liters(100.0)).unwrap_err();
    assert!(matches!(err, FuelError::MissingData(_)));
}

#[test]
fn endurance_without_cruise_table_errors() {
    let mut profile = trainer();
    profile.performance.cruise_settings.clear();
    let err = endurance(&profile, None, Liters(100.0)).unwrap_err();
    assert!(matches!(err, FuelError::MissingData(_)));
}

#[test]
fn endurance_clamps_non_positive_fuel_to_zero() {
    assert_close(endurance(&trainer(), None, Liters(0.0)).unwrap().0, 0.0);
    assert_close(endurance(&trainer(), None, Liters(-5.0)).unwrap().0, 0.0);
}

#[test]
fn zero_taxi_flow_zeroes_the_taxi_rung() {
    let mut profile = trainer();
    profile.performance.taxi_fuel_flow = LitersPerHour(0.0);
    let ladder = compute_fuel_ladder(
        &FuelPolicy::default(),
        &profile,
        None,
        &trip_plan(),
        None,
        Liters(100.0),
    )
    .unwrap();
    assert_close(ladder.taxi.0, 0.0);
    assert_close(ladder.minimum_required.0, 78.0);
}

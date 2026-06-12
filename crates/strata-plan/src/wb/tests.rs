use strata_data::domain::Meters;

use super::*;
use crate::aircraft::{AircraftId, AircraftProfile, WbStation, WeightBalance};
use crate::flight::StationLoad;
use crate::units::{Kilograms, KilogramsPerLiter, Liters};

#[track_caller]
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

fn ep(arm: f64, mass: f64) -> EnvelopePoint {
    EnvelopePoint {
        arm: Meters(arm),
        mass: Kilograms(mass),
    }
}

fn cg(arm: f64, mass: f64) -> CgPoint {
    CgPoint {
        arm: Meters(arm),
        mass: Kilograms(mass),
    }
}

fn station(name: &str, arm: f64, kind: StationKind) -> WbStation {
    WbStation {
        name: name.to_owned(),
        arm: Meters(arm),
        kind,
        max_load: None,
    }
}

fn load(name: &str, kg: f64) -> StationLoad {
    StationLoad {
        station: name.to_owned(),
        mass: Kilograms(kg),
    }
}

/// An invented (plausible, not POH-accurate) 4-seat trainer. All worked
/// values below are hand-computed from these numbers.
fn trainer() -> AircraftProfile {
    let mut profile = AircraftProfile::new(AircraftId::new("trainer").unwrap());
    profile.fuel.density = KilogramsPerLiter(0.72);
    profile.weight_balance = WeightBalance {
        empty_mass: Kilograms(750.0),
        empty_arm: Meters(1.00),
        stations: vec![
            station("Front seats", 0.90, StationKind::Seat),
            station("Rear seats", 1.80, StationKind::Seat),
            station("Baggage", 2.40, StationKind::Baggage),
            station("Fuel tanks", 1.20, StationKind::Fuel),
        ],
        max_takeoff: Kilograms(1100.0),
        max_landing: Some(Kilograms(1050.0)),
        max_zero_fuel: Some(Kilograms(1010.0)),
        max_ramp: Some(Kilograms(1105.0)),
        // Forward limit 0.89 m up to 885 kg, then sloping aft to 1.02 m at
        // the 1100 kg ceiling; aft limit 1.20 m.
        envelope: vec![
            ep(0.89, 700.0),
            ep(0.89, 885.0),
            ep(1.02, 1100.0),
            ep(1.20, 1100.0),
            ep(1.20, 700.0),
        ],
    };
    profile
}

/// Front 160 kg, rear 70 kg, baggage 20 kg, 100 L fuel.
fn standard_loading() -> LoadingScenario {
    LoadingScenario {
        name: "Standard".to_owned(),
        station_loads: vec![
            load("Front seats", 160.0),
            load("Rear seats", 70.0),
            load("Baggage", 20.0),
        ],
        fuel: Liters(100.0),
    }
}

fn state(report: &WbReport, kind: WbStateKind) -> WbState {
    *report
        .states
        .iter()
        .find(|s| s.kind == kind)
        .unwrap_or_else(|| panic!("missing state {kind:?}"))
}

// ---------------------------------------------------------------------------
// Worked example. By hand:
//
//   item          mass [kg]                 arm [m]   moment [kg·m]
//   empty           750.00                   1.00       750.000
//   front seats     160.00                   0.90       144.000
//   rear seats       70.00                   1.80       126.000
//   baggage          20.00                   2.40        48.000
//   ─ zero-fuel    1000.00                              1068.000   → arm 1.068
//
//   fuel on board (ramp):   100 L × 0.72 kg/L = 72.00 kg @ 1.20 → 86.400
//   ramp:    1072.00 kg, moment 1154.400 → arm 1154.400/1072.00 = 1.076866…
//
//   taxi 8 L → takeoff fuel 92 L = 66.24 kg @ 1.20 → 79.488
//   takeoff: 1066.24 kg, moment 1147.488 → arm 1147.488/1066.24 = 1.076200…
//
//   trip 60 L → landing fuel 32 L = 23.04 kg @ 1.20 → 27.648
//   landing: 1023.04 kg, moment 1095.648 → arm 1095.648/1023.04 = 1.070978…
//
// Limits: ramp 1072 ≤ 1105, takeoff 1066.24 ≤ 1100, ZFW 1000 ≤ 1010,
// landing 1023.04 ≤ 1050 — and every CG is inside the envelope (forward
// limit at 1072 kg interpolates to 0.89 + 0.13·(1072−885)/215 = 1.0031 m).
// ---------------------------------------------------------------------------

#[test]
fn worked_example_states() {
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();

    let kinds: Vec<WbStateKind> = report.states.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            WbStateKind::Ramp,
            WbStateKind::Takeoff,
            WbStateKind::ZeroFuel,
            WbStateKind::Landing,
        ]
    );

    let ramp = state(&report, WbStateKind::Ramp);
    assert_close(ramp.mass.0, 1072.0);
    assert_close(ramp.arm.0, 1154.4 / 1072.0); // = 1.0768656716…

    let takeoff = state(&report, WbStateKind::Takeoff);
    assert_close(takeoff.mass.0, 1066.24);
    assert_close(takeoff.arm.0, 1147.488 / 1066.24); // = 1.0762004804…

    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(zfw.mass.0, 1000.0);
    assert_close(zfw.arm.0, 1.068);

    let landing = state(&report, WbStateKind::Landing);
    assert_close(landing.mass.0, 1023.04);
    assert_close(landing.arm.0, 1095.648 / 1023.04); // = 1.0709771437…

    for s in &report.states {
        assert!(s.within_envelope, "{:?} unexpectedly flagged", s.kind);
    }
}

#[test]
fn fuel_mass_uses_profile_density() {
    // Volume → mass via the profile density: 100 L × 0.72 kg/L = 72 kg is
    // exactly the ramp-minus-zero-fuel mass difference.
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    let ramp = state(&report, WbStateKind::Ramp);
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(ramp.mass.0 - zfw.mass.0, 72.0);

    // Different density, same volume: 100 L × 0.80 kg/L = 80 kg.
    let mut diesel = trainer();
    diesel.fuel.density = KilogramsPerLiter(0.80);
    let report =
        compute_weight_balance(&diesel, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    let ramp = state(&report, WbStateKind::Ramp);
    assert_close(ramp.mass.0, 1080.0);
}

#[test]
fn zfw_and_landing_states_diverge() {
    // Landing still has 32 L on board, so it is heavier than zero-fuel and —
    // with the fuel arm (1.20 m) aft of the loaded CG — sits further aft.
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    let takeoff = state(&report, WbStateKind::Takeoff);
    let zfw = state(&report, WbStateKind::ZeroFuel);
    let landing = state(&report, WbStateKind::Landing);

    assert!(landing.mass.0 > zfw.mass.0);
    assert!(landing.arm.0 > zfw.arm.0);
    assert!(takeoff.arm.0 > landing.arm.0);
}

// --- mass limit checks ------------------------------------------------------

#[test]
fn over_mtow_flags_takeoff_only() {
    let mut profile = trainer();
    profile.weight_balance.max_takeoff = Kilograms(1060.0); // takeoff is 1066.24
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(!state(&report, WbStateKind::Takeoff).within_envelope);
    // Ramp has its own (published) limit of 1105 kg.
    assert!(state(&report, WbStateKind::Ramp).within_envelope);
    assert!(state(&report, WbStateKind::ZeroFuel).within_envelope);
    assert!(state(&report, WbStateKind::Landing).within_envelope);
}

#[test]
fn ramp_limit_falls_back_to_mtow() {
    let mut profile = trainer();
    profile.weight_balance.max_takeoff = Kilograms(1060.0);
    profile.weight_balance.max_ramp = None; // ramp (1072) now checked vs MTOW
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(!state(&report, WbStateKind::Ramp).within_envelope);
}

#[test]
fn over_max_ramp_flags_ramp_only() {
    let mut profile = trainer();
    profile.weight_balance.max_ramp = Some(Kilograms(1070.0)); // ramp is 1072
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(!state(&report, WbStateKind::Ramp).within_envelope);
    assert!(state(&report, WbStateKind::Takeoff).within_envelope);
}

#[test]
fn over_mzfw_flags_zero_fuel_only() {
    let mut profile = trainer();
    profile.weight_balance.max_zero_fuel = Some(Kilograms(990.0)); // ZFW is 1000
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(!state(&report, WbStateKind::ZeroFuel).within_envelope);
    assert!(state(&report, WbStateKind::Takeoff).within_envelope);
    assert!(state(&report, WbStateKind::Landing).within_envelope);
}

#[test]
fn over_mlw_flags_landing_only() {
    let mut profile = trainer();
    profile.weight_balance.max_landing = Some(Kilograms(1020.0)); // landing is 1023.04
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(!state(&report, WbStateKind::Landing).within_envelope);
    assert!(state(&report, WbStateKind::Takeoff).within_envelope);
}

#[test]
fn absent_optional_limits_are_not_checked() {
    let mut profile = trainer();
    profile.weight_balance.max_landing = None;
    profile.weight_balance.max_zero_fuel = None;
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    assert!(state(&report, WbStateKind::ZeroFuel).within_envelope);
    assert!(state(&report, WbStateKind::Landing).within_envelope);
}

#[test]
fn aft_cg_flags_all_states_despite_legal_mass() {
    // Rear 180 kg + baggage 50 kg, nothing in front, no fuel:
    //   mass   = 750 + 180 + 50 = 980 kg (below every limit)
    //   moment = 750 + 180·1.80 + 50·2.40 = 750 + 324 + 120 = 1194 kg·m
    //   arm    = 1194/980 = 1.21836… > 1.20 m aft limit → outside polygon.
    let loading = LoadingScenario {
        name: "Aft heavy".to_owned(),
        station_loads: vec![load("Rear seats", 180.0), load("Baggage", 50.0)],
        fuel: Liters(0.0),
    };
    let report = compute_weight_balance(&trainer(), &loading, Liters(0.0), Liters(0.0)).unwrap();
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(zfw.mass.0, 980.0);
    assert_close(zfw.arm.0, 1194.0 / 980.0);
    for s in &report.states {
        assert!(!s.within_envelope, "{:?} not flagged", s.kind);
    }
}

// --- envelope containment ---------------------------------------------------

#[test]
fn envelope_rejects_points_beyond_each_edge() {
    // Plain rectangle: forward 0.9, aft 1.2, masses 700–1100.
    let square = [
        ep(0.9, 700.0),
        ep(0.9, 1100.0),
        ep(1.2, 1100.0),
        ep(1.2, 700.0),
    ];
    assert!(within_envelope(&square, cg(1.0, 900.0))); // inside
    assert!(!within_envelope(&square, cg(0.85, 900.0))); // forward of fwd limit
    assert!(!within_envelope(&square, cg(1.25, 900.0))); // aft of aft limit
    assert!(!within_envelope(&square, cg(1.0, 1150.0))); // above max mass
    assert!(!within_envelope(&square, cg(1.0, 650.0))); // below min mass
}

#[test]
fn envelope_honors_sloped_forward_limit() {
    let envelope = trainer().weight_balance.envelope;
    // Forward limit at 1090 kg: 0.89 + 0.13·(1090−885)/215 = 1.01395 m.
    assert!(!within_envelope(&envelope, cg(0.95, 1090.0))); // fwd of the slope
    assert!(within_envelope(&envelope, cg(1.05, 1090.0))); // aft of the slope
    // Same arm is fine at low mass where the limit is still 0.89 m.
    assert!(within_envelope(&envelope, cg(0.95, 800.0)));
}

#[test]
fn degenerate_envelope_contains_nothing() {
    assert!(!within_envelope(&[], cg(1.0, 900.0)));
    assert!(!within_envelope(
        &[ep(0.9, 700.0), ep(1.2, 1100.0)],
        cg(1.0, 900.0)
    ));
}

// --- burn track -------------------------------------------------------------

#[test]
fn burn_track_runs_from_takeoff_to_zero_fuel() {
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    let takeoff = state(&report, WbStateKind::Takeoff);
    let zfw = state(&report, WbStateKind::ZeroFuel);

    let track = &report.burn_track;
    assert_eq!(track.len(), 17);
    assert_close(track[0].mass.0, takeoff.mass.0);
    assert_close(track[0].arm.0, takeoff.arm.0);
    assert_close(track[16].mass.0, zfw.mass.0);
    assert_close(track[16].arm.0, zfw.arm.0);
    for pair in track.windows(2) {
        assert!(pair[1].mass.0 < pair[0].mass.0, "mass not decreasing");
    }
}

#[test]
fn burn_track_follows_the_constant_moment_arc() {
    // With zero-fuel moment M₀ = 1068 kg·m, zero-fuel mass m₀ = 1000 kg and
    // fuel arm a_f = 1.20 m, every fuel state satisfies
    //   arm(m) = a_f + (M₀ − m₀·a_f)/m = 1.20 − 132/m
    // — a hyperbola in (arm, mass) space. The landing CG lies on it too.
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    for point in &report.burn_track {
        assert_close(point.arm.0, 1.20 - 132.0 / point.mass.0);
    }
    let landing = state(&report, WbStateKind::Landing);
    assert_close(landing.arm.0, 1.20 - 132.0 / landing.mass.0);
}

#[test]
fn burn_track_without_takeoff_fuel_is_a_single_point() {
    let mut loading = standard_loading();
    loading.fuel = Liters(0.0);
    let report = compute_weight_balance(&trainer(), &loading, Liters(0.0), Liters(0.0)).unwrap();
    assert_eq!(report.burn_track.len(), 1);
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(report.burn_track[0].mass.0, zfw.mass.0);
    assert_close(report.burn_track[0].arm.0, zfw.arm.0);
}

// --- fuel clamping ----------------------------------------------------------

#[test]
fn taxi_fuel_clamps_at_empty_tanks() {
    // 5 L on board, 8 L taxi allowance: takeoff fuel clamps to 0 (the fuel
    // ladder, not W&B, flags the shortage).
    let mut loading = standard_loading();
    loading.fuel = Liters(5.0);
    let report = compute_weight_balance(&trainer(), &loading, Liters(8.0), Liters(0.0)).unwrap();
    let takeoff = state(&report, WbStateKind::Takeoff);
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(takeoff.mass.0, zfw.mass.0);
    assert_eq!(report.burn_track.len(), 1);
}

#[test]
fn trip_fuel_clamps_at_takeoff_fuel() {
    // Trip 200 L > 92 L at takeoff: landing fuel clamps to 0 → landing
    // equals the zero-fuel state.
    let report =
        compute_weight_balance(&trainer(), &standard_loading(), Liters(8.0), Liters(200.0))
            .unwrap();
    let landing = state(&report, WbStateKind::Landing);
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(landing.mass.0, zfw.mass.0);
    assert_close(landing.arm.0, zfw.arm.0);
}

// --- station handling -------------------------------------------------------

#[test]
fn fuel_splits_equally_across_fuel_stations() {
    // Two tanks at 1.10 m and 1.30 m, equal split → mean arm 1.20 m: the
    // ramp state matches the single-tank worked example exactly.
    let mut profile = trainer();
    profile.weight_balance.stations = vec![
        station("Front seats", 0.90, StationKind::Seat),
        station("Rear seats", 1.80, StationKind::Seat),
        station("Baggage", 2.40, StationKind::Baggage),
        station("Left tank", 1.10, StationKind::Fuel),
        station("Right tank", 1.30, StationKind::Fuel),
    ];
    let report =
        compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0)).unwrap();
    let ramp = state(&report, WbStateKind::Ramp);
    assert_close(ramp.mass.0, 1072.0);
    assert_close(ramp.arm.0, 1154.4 / 1072.0);
}

#[test]
fn duplicate_station_loads_sum() {
    // Front seats loaded as two 80 kg entries == one 160 kg entry.
    let mut loading = standard_loading();
    loading.station_loads = vec![
        load("Front seats", 80.0),
        load("Front seats", 80.0),
        load("Rear seats", 70.0),
        load("Baggage", 20.0),
    ];
    let report = compute_weight_balance(&trainer(), &loading, Liters(8.0), Liters(60.0)).unwrap();
    let ramp = state(&report, WbStateKind::Ramp);
    assert_close(ramp.mass.0, 1072.0);
    assert_close(ramp.arm.0, 1154.4 / 1072.0);
}

#[test]
fn load_on_fuel_station_is_fixed_mass() {
    // A 10 kg station load on the fuel station is dead weight at 1.20 m —
    // it does not burn: zero-fuel mass = 1000 + 10, moment = 1068 + 12.
    let mut loading = standard_loading();
    loading.station_loads.push(load("Fuel tanks", 10.0));
    loading.fuel = Liters(0.0);
    let report = compute_weight_balance(&trainer(), &loading, Liters(0.0), Liters(0.0)).unwrap();
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(zfw.mass.0, 1010.0);
    assert_close(zfw.arm.0, 1080.0 / 1010.0);
}

// --- errors -----------------------------------------------------------------

#[test]
fn unknown_station_errors() {
    let mut loading = standard_loading();
    loading.station_loads.push(load("Ski rack", 12.0));
    let err = compute_weight_balance(&trainer(), &loading, Liters(8.0), Liters(60.0)).unwrap_err();
    assert!(matches!(err, WbError::UnknownStation(name) if name == "Ski rack"));
}

#[test]
fn missing_or_degenerate_envelope_errors() {
    let mut profile = trainer();
    profile.weight_balance.envelope.clear();
    let err = compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0))
        .unwrap_err();
    assert!(matches!(err, WbError::NoEnvelope));

    profile.weight_balance.envelope = vec![ep(0.9, 700.0), ep(1.2, 1100.0)];
    let err = compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0))
        .unwrap_err();
    assert!(matches!(err, WbError::NoEnvelope));
}

#[test]
fn fuel_without_fuel_station_errors() {
    let mut profile = trainer();
    profile
        .weight_balance
        .stations
        .retain(|s| s.kind != StationKind::Fuel);
    let err = compute_weight_balance(&profile, &standard_loading(), Liters(8.0), Liters(60.0))
        .unwrap_err();
    assert!(matches!(err, WbError::NoFuelStation));

    // No fuel on board: a profile without fuel stations is fine (motorglider
    // day-trip style scenario).
    let mut loading = standard_loading();
    loading.fuel = Liters(0.0);
    let report = compute_weight_balance(&profile, &loading, Liters(0.0), Liters(0.0)).unwrap();
    let zfw = state(&report, WbStateKind::ZeroFuel);
    assert_close(zfw.mass.0, 1000.0);
}

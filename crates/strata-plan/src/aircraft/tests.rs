use serde_json::json;
use strata_data::domain::Meters;

use super::*;
use crate::units::{FeetPerMinute, Kilograms, KilogramsPerLiter, Knots, Liters, LitersPerHour};
use crate::versioned::VersionError;

fn id(s: &str) -> AircraftId {
    AircraftId::new(s).unwrap()
}

/// A fully-populated C172-ish profile (example numbers, not POH-accurate).
fn full_profile() -> AircraftProfile {
    AircraftProfile {
        registration: "D-EABC".to_owned(),
        type_designator: "C172".to_owned(),
        callsign: "DEABC".to_owned(),
        name: Some("Skyhawk — club".to_owned()),
        performance: Performance {
            cruise_settings: vec![
                PowerSetting {
                    name: "65 %".to_owned(),
                    tas: Knots(108.0),
                    fuel_flow: LitersPerHour(32.0),
                },
                PowerSetting {
                    name: "75 %".to_owned(),
                    tas: Knots(115.0),
                    fuel_flow: LitersPerHour(36.0),
                },
            ],
            climb: ClimbPerformance {
                ias: Knots(74.0),
                rate: FeetPerMinute(650.0),
                fuel_flow: LitersPerHour(40.0),
            },
            descent: DescentPerformance {
                ias: Knots(110.0),
                rate: FeetPerMinute(500.0),
                fuel_flow: LitersPerHour(28.0),
            },
            taxi_fuel_flow: LitersPerHour(7.0),
        },
        fuel: FuelSystem {
            usable: Liters(201.0),
            tabs: Some(Liters(132.0)),
            fuel_type: FuelType::Avgas100Ll,
            density: KilogramsPerLiter(0.72),
        },
        weight_balance: WeightBalance {
            empty_mass: Kilograms(767.0),
            empty_arm: Meters(0.99),
            stations: vec![
                WbStation {
                    name: "Front seats".to_owned(),
                    arm: Meters(0.94),
                    kind: StationKind::Seat,
                    max_load: None,
                },
                WbStation {
                    name: "Baggage A".to_owned(),
                    arm: Meters(2.41),
                    kind: StationKind::Baggage,
                    max_load: Some(Kilograms(54.0)),
                },
                WbStation {
                    name: "Fuel".to_owned(),
                    arm: Meters(1.21),
                    kind: StationKind::Fuel,
                    max_load: None,
                },
            ],
            max_takeoff: Kilograms(1157.0),
            max_landing: Some(Kilograms(1157.0)),
            max_zero_fuel: None,
            max_ramp: Some(Kilograms(1160.0)),
            envelope: vec![
                EnvelopePoint {
                    arm: Meters(0.89),
                    mass: Kilograms(767.0),
                },
                EnvelopePoint {
                    arm: Meters(0.89),
                    mass: Kilograms(885.0),
                },
                EnvelopePoint {
                    arm: Meters(1.00),
                    mass: Kilograms(1157.0),
                },
                EnvelopePoint {
                    arm: Meters(1.20),
                    mass: Kilograms(1157.0),
                },
                EnvelopePoint {
                    arm: Meters(1.20),
                    mass: Kilograms(767.0),
                },
            ],
        },
        distances: Distances {
            takeoff_roll: Meters(296.0),
            takeoff_over_50ft: Some(Meters(497.0)),
            landing_roll: Meters(175.0),
            landing_over_50ft: Some(Meters(407.0)),
            ..Distances::default()
        },
        equipment: FplEquipment {
            com_nav_approach: "SDFGO".to_owned(),
            surveillance: "S".to_owned(),
        },
        ..AircraftProfile::new(id("d-eabc"))
    }
}

#[test]
fn aircraft_id_validation() {
    assert_eq!(AircraftId::new("D-EABC").unwrap().as_str(), "d-eabc");
    assert_eq!(AircraftId::new("c172_club").unwrap().as_str(), "c172_club");
    assert!(AircraftId::new("").is_err());
    assert!(AircraftId::new("has space").is_err());
    assert!(AircraftId::new("dot.dot").is_err());
    assert!(AircraftId::new(&"x".repeat(65)).is_err());
    assert_eq!(
        AircraftId::new(&"x".repeat(64)).unwrap().as_str(),
        "x".repeat(64)
    );
}

#[test]
fn round_trip_preserves_profile() {
    let profile = full_profile();
    let saved = profile.to_json_string().unwrap();
    let loaded = AircraftProfile::from_json_str(&saved).unwrap();
    assert_eq!(profile, loaded);
}

#[test]
fn saved_json_carries_format_version() {
    let saved = full_profile().to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(value["format_version"], json!(AIRCRAFT_FORMAT_VERSION));
}

#[test]
fn unknown_fields_are_tolerated() {
    let json = r#"{
        "format_version": 1,
        "id": "d-eabc",
        "registration": "D-EABC",
        "future_block": {"anything": true},
        "performance": {
            "taxi_fuel_flow": 7.5,
            "future_perf_field": [1, 2]
        }
    }"#;
    let profile = AircraftProfile::from_json_str(json).unwrap();
    assert_eq!(profile.registration, "D-EABC");
    assert_eq!(profile.performance.taxi_fuel_flow, LitersPerHour(7.5));
    // Missing blocks fall back to template defaults.
    assert_eq!(profile.fuel.density, KilogramsPerLiter(0.72));
    assert_eq!(profile.distances.takeoff_safety_factor, 1.33);
    assert_eq!(profile.distances.landing_safety_factor, 1.43);
    assert_eq!(profile.equipment, FplEquipment::default());
}

#[test]
fn id_is_required() {
    assert!(matches!(
        AircraftProfile::from_json_str(r#"{"format_version": 1}"#),
        Err(AircraftError::Version(VersionError::Json(_)))
    ));
}

#[test]
fn newer_format_version_is_refused() {
    let result = AircraftProfile::from_json_str(r#"{"format_version": 7, "id": "x"}"#);
    assert!(matches!(
        result,
        Err(AircraftError::Version(VersionError::TooNew {
            found: 7,
            supported: 1
        }))
    ));
}

#[test]
fn missing_format_version_is_version_one() {
    let profile = AircraftProfile::from_json_str(r#"{"id": "d-eabc"}"#).unwrap();
    assert_eq!(profile.format_version, AIRCRAFT_FORMAT_VERSION);
}

#[test]
fn distance_factor_template_defaults() {
    let factors = DistanceFactors::default();
    assert_eq!(factors.per_1000_ft_density_altitude, 0.10);
    assert_eq!(factors.per_10_kt_headwind, -0.10);
    assert_eq!(factors.per_10_kt_tailwind, 0.40);
    assert_eq!(factors.grass, 0.20);
    assert_eq!(factors.wet, 0.15);
    assert_eq!(factors.per_percent_slope, 0.10);
}

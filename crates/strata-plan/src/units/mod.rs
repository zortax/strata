//! Planning-side unit newtypes.
//!
//! `strata-data` owns the metric/datum-carrying core ([`Meters`],
//! [`MetersAmsl`], [`MetersAgl`]); this module adds the aviation-facing
//! units pilots plan in (NM, kt, ft/min, L, L/h, kg, min, °C) plus the
//! true/magnetic angle discipline. POH-style aircraft data is *stored* in
//! these units and converted to SI at the computation edges.
//!
//! [`Meters`]: strata_data::domain::Meters
//! [`MetersAmsl`]: strata_data::domain::MetersAmsl
//! [`MetersAgl`]: strata_data::domain::MetersAgl

use serde::{Deserialize, Serialize};
use strata_data::domain::Meters;

/// Meters per international nautical mile (exact).
pub const METERS_PER_NAUTICAL_MILE: f64 = 1852.0;

/// Horizontal distance in nautical miles (display/planning unit; route
/// geometry computes in [`Meters`] and converts here at the edge).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct NauticalMiles(pub f64);

impl NauticalMiles {
    pub fn from_meters(m: Meters) -> Self {
        Self(m.0 / METERS_PER_NAUTICAL_MILE)
    }

    pub fn as_meters(self) -> Meters {
        Meters(self.0 * METERS_PER_NAUTICAL_MILE)
    }
}

/// Speed in knots (TAS, GS, IAS, wind speed).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Knots(pub f64);

impl Knots {
    pub fn as_meters_per_second(self) -> f64 {
        self.0 * METERS_PER_NAUTICAL_MILE / 3600.0
    }
}

/// Vertical speed in feet per minute (POH climb/descent rates).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FeetPerMinute(pub f64);

impl FeetPerMinute {
    pub fn as_meters_per_second(self) -> f64 {
        self.0 / (strata_data::domain::FEET_PER_METER * 60.0)
    }
}

/// Fuel volume in liters.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Liters(pub f64);

/// Fuel flow in liters per hour.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct LitersPerHour(pub f64);

/// Mass in kilograms.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Kilograms(pub f64);

/// Density in kilograms per liter (fuel mass for W&B).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct KilogramsPerLiter(pub f64);

/// Duration in minutes (reserve policies, ETE).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Minutes(pub f64);

impl Minutes {
    pub fn from_hours(hours: f64) -> Self {
        Self(hours * 60.0)
    }

    pub fn as_hours(self) -> f64 {
        self.0 / 60.0
    }
}

/// Temperature in degrees Celsius.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Celsius(pub f64);

/// An angle relative to **true** north, degrees. Constructors normalize
/// into `[0, 360)`; the raw field is public for literals, so math helpers
/// re-normalize defensively.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DegreesTrue(pub f64);

impl DegreesTrue {
    /// Normalizes any finite angle into `[0, 360)`.
    pub fn new(degrees: f64) -> Self {
        Self(normalize_degrees(degrees))
    }

    /// True → magnetic: subtracts the (east-positive) variation
    /// ("east is least").
    pub fn to_magnetic(self, variation: MagneticVariation) -> DegreesMagnetic {
        DegreesMagnetic::new(self.0 - variation.0)
    }
}

/// An angle relative to **magnetic** north, degrees.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DegreesMagnetic(pub f64);

impl DegreesMagnetic {
    /// Normalizes any finite angle into `[0, 360)`.
    pub fn new(degrees: f64) -> Self {
        Self(normalize_degrees(degrees))
    }

    /// Magnetic → true: adds the (east-positive) variation back.
    pub fn to_true(self, variation: MagneticVariation) -> DegreesTrue {
        DegreesTrue::new(self.0 + variation.0)
    }
}

/// Magnetic variation (declination) in degrees, **east positive** (the WMM
/// convention): `magnetic = true - variation`.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MagneticVariation(pub f64);

/// Wraps an angle into `[0, 360)`.
fn normalize_degrees(degrees: f64) -> f64 {
    let wrapped = degrees % 360.0;
    if wrapped < 0.0 { wrapped + 360.0 } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nautical_mile_round_trip() {
        let nm = NauticalMiles::from_meters(Meters(1852.0));
        assert!((nm.0 - 1.0).abs() < 1e-12);
        assert!((nm.as_meters().0 - 1852.0).abs() < 1e-9);
        assert!((NauticalMiles(10.0).as_meters().0 - 18_520.0).abs() < 1e-9);
    }

    #[test]
    fn knots_to_mps() {
        // 1 kt = 1852 m / 3600 s.
        assert!((Knots(1.0).as_meters_per_second() - 0.514_444_444).abs() < 1e-6);
    }

    #[test]
    fn feet_per_minute_to_mps() {
        // 500 ft/min = 2.54 m/s.
        assert!((FeetPerMinute(500.0).as_meters_per_second() - 2.54).abs() < 1e-9);
    }

    #[test]
    fn minutes_hours_round_trip() {
        assert_eq!(Minutes::from_hours(1.5).0, 90.0);
        assert!((Minutes(45.0).as_hours() - 0.75).abs() < 1e-12);
    }

    #[test]
    fn degrees_normalize() {
        assert_eq!(DegreesTrue::new(370.0).0, 10.0);
        assert_eq!(DegreesTrue::new(-10.0).0, 350.0);
        assert_eq!(DegreesTrue::new(360.0).0, 0.0);
        assert_eq!(DegreesMagnetic::new(-0.0).0, 0.0);
    }

    #[test]
    fn true_magnetic_conversion_east_is_least() {
        // 3° east variation: true 100° -> magnetic 97°.
        let var = MagneticVariation(3.0);
        assert_eq!(DegreesTrue::new(100.0).to_magnetic(var).0, 97.0);
        // Wraps below zero: true 1° with 5°E variation -> magnetic 356°.
        assert_eq!(DegreesTrue::new(1.0).to_magnetic(MagneticVariation(5.0)).0, 356.0);
        // West variation (negative): true 100° with 2°W -> magnetic 102°.
        assert_eq!(DegreesTrue::new(100.0).to_magnetic(MagneticVariation(-2.0)).0, 102.0);
        // Round trip.
        let back = DegreesTrue::new(100.0).to_magnetic(var).to_true(var);
        assert!((back.0 - 100.0).abs() < 1e-12);
    }
}

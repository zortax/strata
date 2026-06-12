//! Parametric performance: cruise power settings, climb/descent, taxi flow.

use serde::{Deserialize, Serialize};

use crate::units::{FeetPerMinute, Knots, LitersPerHour};

/// One cruise power setting from the POH cruise table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerSetting {
    /// Label as the pilot knows it, e.g. `"65 %"` or `"2300 RPM"`.
    pub name: String,
    pub tas: Knots,
    pub fuel_flow: LitersPerHour,
}

/// Climb planning values (single-segment model; plan §3 `perf/`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimbPerformance {
    pub ias: Knots,
    pub rate: FeetPerMinute,
    pub fuel_flow: LitersPerHour,
}

/// Descent planning values (single-segment model).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DescentPerformance {
    pub ias: Knots,
    pub rate: FeetPerMinute,
    pub fuel_flow: LitersPerHour,
}

/// The parametric performance block of an
/// [`AircraftProfile`](super::AircraftProfile). Defaults are zeros —
/// placeholders until POH values are entered, never flyable numbers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Performance {
    /// POH cruise settings; the flight picks one by name
    /// ([`FlightDoc::power_setting`](crate::flight::FlightDoc::power_setting)).
    pub cruise_settings: Vec<PowerSetting>,
    pub climb: ClimbPerformance,
    pub descent: DescentPerformance,
    /// Ground/taxi fuel flow (taxi fuel = policy taxi time × this flow).
    pub taxi_fuel_flow: LitersPerHour,
}

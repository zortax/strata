//! Performance phases and runway distances (plan §3 `perf/`): climb/
//! cruise/descent splitting with TOC/TOD, per-phase time/fuel/distance,
//! density altitude, and the takeoff/landing correction chain.
//!
//! Submodules: [`plan_phases`] (the vertical profile), [`density_altitude`]
//! / [`pressure_altitude`] (the E6B rules of thumb, documented at the
//! implementations), [`takeoff_distance`] / [`landing_distance`] (the
//! correction-factor chain over
//! [`DistanceFactors`](crate::aircraft::DistanceFactors)),
//! [`wind_components`] (head/crosswind decomposition per runway) and
//! [`runway_margin`] (required vs declared length).

mod density;
mod isa;
mod phases;
mod runway;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use strata_data::domain::{LatLon, Meters, MetersAmsl, Qnh, RunwaySurface};
use thiserror::Error;

use crate::units::{Celsius, Knots, Liters, Minutes};

pub use density::{density_altitude, pressure_altitude};
pub use isa::{
    ISA_LAPSE_CELSIUS_PER_METER, ISA_SEA_LEVEL_CELSIUS, isa_temperature, planned_altitude_amsl,
};
pub use phases::plan_phases;
pub(crate) use phases::resolve_cruise;
pub use runway::{
    RunwayMargin, WindComponents, landing_distance, runway_margin, takeoff_distance,
    wind_components,
};

/// Flight phase of a profile segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseKind {
    Climb,
    Cruise,
    Descent,
}

/// One constant-phase segment of the vertical profile.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhaseSegment {
    pub kind: PhaseKind,
    /// Along-track interval from departure.
    pub start_along_track: Meters,
    pub end_along_track: Meters,
    /// Altitudes at the segment ends (equal for cruise).
    pub start_altitude: MetersAmsl,
    pub end_altitude: MetersAmsl,
    /// TAS planned through the segment.
    pub tas: Knots,
    pub duration: Minutes,
    pub fuel: Liters,
}

/// A point marker on the profile (TOC/TOD).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfileMarker {
    pub along_track: Meters,
    pub position: LatLon,
    pub altitude: MetersAmsl,
}

/// The planned vertical/temporal profile: contiguous segments from
/// departure to destination with top-of-climb / top-of-descent markers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhasePlan {
    /// Segments in along-track order, gap-free from 0 to route end.
    pub segments: Vec<PhaseSegment>,
    /// `None` when the route never reaches a cruise altitude (e.g. climb
    /// straight into the descent point).
    pub toc: Option<ProfileMarker>,
    pub tod: Option<ProfileMarker>,
    pub total_duration: Minutes,
    /// Trip fuel (sum of segments; excludes taxi/reserves — see
    /// [`crate::fuel`]).
    pub total_fuel: Liters,
}

/// Conditions for one runway-distance assessment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RunwayConditions {
    pub field_elevation: MetersAmsl,
    pub qnh: Qnh,
    pub temperature: Celsius,
    /// Wind component along the runway; negative = tailwind.
    pub headwind_component: Knots,
    pub surface: RunwaySurface,
    /// Runway slope in percent; positive = upslope in the direction of
    /// the run.
    pub slope_percent: f64,
    pub wet: bool,
}

/// Errors from performance planning.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PerfError {
    #[error("aircraft profile has no cruise power settings")]
    NoCruiseSetting,
    #[error("aircraft profile has no power setting named {0:?}")]
    UnknownPowerSetting(String),
    #[error("invalid performance data: {0}")]
    InvalidProfile(String),
    /// The leg has neither its own altitude nor a flight cruise altitude
    /// to fall back to — the vertical profile is undefined.
    #[error("leg {0} has no planned altitude (no leg altitude and no flight cruise altitude)")]
    NoPlannedAltitude(usize),
}

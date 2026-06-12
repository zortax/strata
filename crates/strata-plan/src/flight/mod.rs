//! The flight document — the single unit everything in planning mode reads
//! and edits, persisted as a pretty-JSON `.strata-flight` file (plan §2.5).
//!
//! Serde is tolerant (unknown fields ignored, missing fields defaulted) and
//! versioned through [`FLIGHT_FORMAT_VERSION`] with a step-wise migration
//! scaffold in [`FlightDoc::from_json_str`].

mod loading;
mod point;
mod policy;
mod snapshot;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use loading::{LoadingScenario, StationLoad};
pub use point::{
    FreePoint, ManualWind, NamedPoint, NamedPointKind, PlannedAltitude, RoutePoint, RouteWaypoint,
};
pub use policy::{Contingency, FuelPolicy};
pub use snapshot::{
    NOTAM_SNAPSHOT_FORMAT_VERSION, NotamSnapshot, WEATHER_SNAPSHOT_FORMAT_VERSION, WeatherSnapshot,
};

use crate::aircraft::AircraftId;
use crate::versioned::{self, VersionError};

/// Current on-disk format version of [`FlightDoc`].
pub const FLIGHT_FORMAT_VERSION: u32 = 1;

/// Errors loading or saving a flight document.
#[derive(Debug, Error)]
pub enum FlightError {
    #[error(transparent)]
    Version(#[from] VersionError),
    #[error("serializing flight document: {0}")]
    Json(#[from] serde_json::Error),
}

/// Flight rules of the plan. VFR-only today; the enum is the IFR seam
/// (plan §3): adding `Ifr` later is new variants, not a rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlightRules {
    #[default]
    Vfr,
}

/// The flight document: route + alternates, aircraft reference, loading
/// scenario, fuel policy, and the weather/NOTAM snapshots it was planned
/// with. Computed outputs are *not* stored — they are recomputed from this
/// document (see [`mod@crate::compute`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlightDoc {
    /// On-disk format version; forced to [`FLIGHT_FORMAT_VERSION`] on load
    /// and save.
    pub format_version: u32,
    /// Display name, e.g. `"EDFE → EDQN"`.
    pub name: String,
    pub rules: FlightRules,
    /// Aircraft profile this flight is planned with (file reference);
    /// `None` until the user picks one.
    pub aircraft_id: Option<AircraftId>,
    /// Name of the aircraft profile's cruise [`PowerSetting`] to plan with;
    /// `None` = the profile's first setting.
    ///
    /// [`PowerSetting`]: crate::aircraft::PowerSetting
    pub power_setting: Option<String>,
    /// Planned off-block/departure time (UTC).
    pub departure_time: Option<DateTime<Utc>>,
    /// Default cruise altitude for legs without their own
    /// [`RouteWaypoint::leg_altitude`].
    pub cruise_altitude: Option<PlannedAltitude>,
    /// The route, departure first. Leg-scoped fields on a waypoint describe
    /// the leg *from* that waypoint to its successor.
    pub route: Vec<RouteWaypoint>,
    /// Alternate destinations (fuel planning + briefing scope).
    pub alternates: Vec<RoutePoint>,
    /// Active loading scenario (W&B station loads + fuel at engine start).
    pub loading: LoadingScenario,
    pub fuel_policy: FuelPolicy,
    pub weather_snapshot: Option<WeatherSnapshot>,
    pub notam_snapshot: Option<NotamSnapshot>,
}

impl Default for FlightDoc {
    fn default() -> Self {
        Self {
            format_version: FLIGHT_FORMAT_VERSION,
            name: String::new(),
            rules: FlightRules::Vfr,
            aircraft_id: None,
            power_setting: None,
            departure_time: None,
            cruise_altitude: None,
            route: Vec::new(),
            alternates: Vec::new(),
            loading: LoadingScenario::default(),
            fuel_policy: FuelPolicy::default(),
            weather_snapshot: None,
            notam_snapshot: None,
        }
    }
}

impl FlightDoc {
    /// An empty flight with `name` and template defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Loads from JSON, migrating older format versions step-by-step and
    /// refusing newer ones. Unknown fields are ignored, missing fields
    /// defaulted (tolerant loading, like the app config).
    pub fn from_json_str(json: &str) -> Result<Self, FlightError> {
        let mut doc: FlightDoc = versioned::load_versioned(json, FLIGHT_FORMAT_VERSION, migrate)?;
        doc.format_version = FLIGHT_FORMAT_VERSION;
        Ok(doc)
    }

    /// Serializes as pretty JSON with the current [`FLIGHT_FORMAT_VERSION`].
    pub fn to_json_string(&self) -> Result<String, FlightError> {
        let mut doc = self.clone();
        doc.format_version = FLIGHT_FORMAT_VERSION;
        Ok(serde_json::to_string_pretty(&doc)?)
    }
}

/// Migration scaffold: transforms a raw document `Value` from
/// `format_version == from` to `from + 1`.
///
/// No migrations exist yet (version 1 is the first format). When version 2
/// lands, this becomes `match from { 1 => migrate_v1_to_v2(value), _ =>
/// Err(..) }` plus a fixture test for the old shape.
fn migrate(_value: Value, from: u32) -> Result<Value, VersionError> {
    Err(VersionError::NoMigration { from })
}

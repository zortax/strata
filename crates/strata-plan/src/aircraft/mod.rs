//! Aircraft profiles (plan §3 `aircraft/`): identity, parametric
//! performance, fuel system, weight & balance, takeoff/landing distances
//! and FPL equipment defaults.
//!
//! Profiles are files next to the flights
//! (`aircraft/<id>.strata-aircraft`, pretty JSON), versioned and loaded
//! tolerantly exactly like [`FlightDoc`](crate::flight::FlightDoc).
//! All numeric values are POH-style aviation units (kt, ft/min, L, kg) —
//! entered from the POH, converted to SI at the computation edges.

mod distances;
mod equipment;
mod fuel_system;
mod performance;
#[cfg(test)]
mod tests;
mod weight_balance;

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use distances::{DistanceFactors, Distances};
pub use equipment::FplEquipment;
pub use fuel_system::{FuelSystem, FuelType};
pub use performance::{ClimbPerformance, DescentPerformance, Performance, PowerSetting};
pub use weight_balance::{EnvelopePoint, StationKind, WbStation, WeightBalance};

use crate::versioned::{self, VersionError};

/// Current on-disk format version of [`AircraftProfile`].
pub const AIRCRAFT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum AircraftIdError {
    #[error("invalid aircraft id {0:?} (expected 1–64 chars of [a-z0-9_-])")]
    Invalid(String),
}

/// Stable identifier of an aircraft profile — also its file stem, hence the
/// strict slug alphabet. Normalized to lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct AircraftId(String);

impl AircraftId {
    pub fn new(id: &str) -> Result<Self, AircraftIdError> {
        let normalized = id.to_ascii_lowercase();
        let valid_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
        if normalized.is_empty() || normalized.len() > 64 || !normalized.chars().all(valid_char) {
            return Err(AircraftIdError::Invalid(id.to_owned()));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AircraftId {
    type Error = AircraftIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<AircraftId> for String {
    fn from(id: AircraftId) -> Self {
        id.0
    }
}

impl fmt::Display for AircraftId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors loading or saving an aircraft profile.
#[derive(Debug, Error)]
pub enum AircraftError {
    #[error(transparent)]
    Version(#[from] VersionError),
    #[error("serializing aircraft profile: {0}")]
    Json(#[from] serde_json::Error),
}

/// A parametric aircraft profile. Bundled example profiles are templates —
/// "example data, replace with your POH values" (design §3.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AircraftProfile {
    /// On-disk format version; forced to [`AIRCRAFT_FORMAT_VERSION`] on
    /// load and save.
    #[serde(default = "first_version")]
    pub format_version: u32,
    pub id: AircraftId,
    /// Registration, e.g. `"D-EABC"`.
    #[serde(default)]
    pub registration: String,
    /// ICAO type designator, e.g. `"C172"` (FPL item 9).
    #[serde(default)]
    pub type_designator: String,
    /// Default radio callsign / FPL item 7 identification. Empty = derive
    /// from [`Self::registration`] (the GA norm — `D-EABC` files as
    /// `DEABC`); set it for operator callsigns.
    #[serde(default)]
    pub callsign: String,
    /// Optional display name, e.g. `"Skyhawk — club"`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub performance: Performance,
    #[serde(default)]
    pub fuel: FuelSystem,
    #[serde(default)]
    pub weight_balance: WeightBalance,
    #[serde(default)]
    pub distances: Distances,
    #[serde(default)]
    pub equipment: FplEquipment,
}

impl AircraftProfile {
    /// An empty profile with `id` and template defaults; every performance
    /// value starts at zero and must be filled from the POH.
    pub fn new(id: AircraftId) -> Self {
        Self {
            format_version: AIRCRAFT_FORMAT_VERSION,
            id,
            registration: String::new(),
            type_designator: String::new(),
            callsign: String::new(),
            name: None,
            performance: Performance::default(),
            fuel: FuelSystem::default(),
            weight_balance: WeightBalance::default(),
            distances: Distances::default(),
            equipment: FplEquipment::default(),
        }
    }

    /// Loads from JSON with the same tolerant + versioned semantics as
    /// [`FlightDoc::from_json_str`](crate::flight::FlightDoc::from_json_str).
    pub fn from_json_str(json: &str) -> Result<Self, AircraftError> {
        let mut profile: AircraftProfile =
            versioned::load_versioned(json, AIRCRAFT_FORMAT_VERSION, migrate)?;
        profile.format_version = AIRCRAFT_FORMAT_VERSION;
        Ok(profile)
    }

    /// Serializes as pretty JSON with the current [`AIRCRAFT_FORMAT_VERSION`].
    pub fn to_json_string(&self) -> Result<String, AircraftError> {
        let mut profile = self.clone();
        profile.format_version = AIRCRAFT_FORMAT_VERSION;
        Ok(serde_json::to_string_pretty(&profile)?)
    }
}

/// Migration scaffold; see [`crate::flight`]'s `migrate` for the pattern.
fn migrate(_value: Value, from: u32) -> Result<Value, VersionError> {
    Err(VersionError::NoMigration { from })
}

/// Files predating the version field are version 1 — keep at 1 forever.
fn first_version() -> u32 {
    1
}

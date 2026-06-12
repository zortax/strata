//! Loading scenarios: what is in the aircraft for this flight.

use serde::{Deserialize, Serialize};

use crate::units::{Kilograms, Liters};

/// Mass loaded at one W&B station of the aircraft profile, referenced by
/// station name (see [`WbStation`](crate::aircraft::WbStation)).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationLoad {
    /// Name of the profile's W&B station, e.g. `"Front seats"`.
    pub station: String,
    pub mass: Kilograms,
}

/// A named loading variant for the flight: per-station masses plus fuel at
/// engine start. Fuel mass for W&B derives from the aircraft profile's fuel
/// density at its fuel station(s).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadingScenario {
    pub name: String,
    pub station_loads: Vec<StationLoad>,
    /// Usable fuel on board at engine start.
    pub fuel: Liters,
}

impl Default for LoadingScenario {
    fn default() -> Self {
        Self {
            name: "Standard".to_owned(),
            station_loads: Vec::new(),
            fuel: Liters(0.0),
        }
    }
}

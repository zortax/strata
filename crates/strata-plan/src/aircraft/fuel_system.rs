//! Fuel system: capacities and fuel type/density.

use serde::{Deserialize, Serialize};

use crate::units::{KilogramsPerLiter, Liters};

/// Fuel type — informational; the W&B math uses [`FuelSystem::density`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuelType {
    #[default]
    Avgas100Ll,
    Mogas,
    JetA1,
    Diesel,
    Other,
}

/// The fuel system of an aircraft profile.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FuelSystem {
    /// Total usable fuel.
    pub usable: Liters,
    /// "Filled to tabs" partial level, if the tanks have tabs.
    pub tabs: Option<Liters>,
    pub fuel_type: FuelType,
    /// Mass density used for W&B. Template default: 0.72 kg/L (avgas).
    pub density: KilogramsPerLiter,
}

impl Default for FuelSystem {
    fn default() -> Self {
        Self {
            usable: Liters(0.0),
            tabs: None,
            fuel_type: FuelType::Avgas100Ll,
            density: KilogramsPerLiter(0.72),
        }
    }
}

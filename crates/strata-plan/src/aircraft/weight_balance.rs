//! Weight & balance data: stations, limits, CG envelope polygon.
//!
//! Arms are in **meters aft of the datum** (negative = forward of datum);
//! masses in kilograms. POHs in lb/in are converted on entry.

use serde::{Deserialize, Serialize};
use strata_data::domain::Meters;

use crate::units::Kilograms;

/// What a loading station holds — lets the loading UI pick widgets and ties
/// fuel stations to the fuel quantity of the loading scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationKind {
    Seat,
    Baggage,
    /// Loaded from the scenario's fuel quantity × fuel density, not a
    /// free mass entry.
    Fuel,
    Other,
}

/// One loading station of the W&B sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbStation {
    /// Unique within the profile; loading scenarios reference it by name.
    pub name: String,
    /// Arm aft of datum.
    pub arm: Meters,
    pub kind: StationKind,
    /// Structural limit of the station, if published.
    pub max_load: Option<Kilograms>,
}

/// A vertex of the CG envelope polygon, in (arm, mass) space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnvelopePoint {
    pub arm: Meters,
    pub mass: Kilograms,
}

/// The W&B block of an aircraft profile. Defaults are zeros/empty —
/// placeholders until POH values are entered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeightBalance {
    /// Basic empty mass.
    pub empty_mass: Kilograms,
    /// Arm of the basic empty mass (moment = mass × arm).
    pub empty_arm: Meters,
    pub stations: Vec<WbStation>,
    pub max_takeoff: Kilograms,
    pub max_landing: Option<Kilograms>,
    pub max_zero_fuel: Option<Kilograms>,
    pub max_ramp: Option<Kilograms>,
    /// CG envelope polygon vertices in published order (unclosed ring,
    /// like `strata_data`'s `Polygon` convention).
    pub envelope: Vec<EnvelopePoint>,
}

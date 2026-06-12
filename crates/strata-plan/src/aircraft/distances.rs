//! Takeoff/landing base distances and the correction-factor chain.
//!
//! Factor defaults are the classic safety-leaflet rules of thumb (CAA
//! Safety Sense / EASA safety promotion) — **templates**, clearly labelled
//! in the UI; replace with POH/operator data. The correction chain itself
//! is applied in [`crate::perf`].

use serde::{Deserialize, Serialize};
use strata_data::domain::Meters;

/// Multiplicative correction factors. Each field is the *fractional
/// increase* (0.10 = +10 %) applied per unit named; negative values
/// decrease the distance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DistanceFactors {
    /// Per 1000 ft of density altitude above ISA sea level.
    /// Template: +0.10.
    pub per_1000_ft_density_altitude: f64,
    /// Per 10 kt of headwind component. Template: −0.10.
    pub per_10_kt_headwind: f64,
    /// Per 10 kt of tailwind component. Template: +0.40 (conservative).
    pub per_10_kt_tailwind: f64,
    /// Dry grass surface. Template: +0.20.
    pub grass: f64,
    /// Wet surface, additional to the surface factor. Template: +0.15.
    pub wet: f64,
    /// Per 1 % of adverse slope (upslope on takeoff, downslope on
    /// landing). Template: +0.10.
    pub per_percent_slope: f64,
}

impl Default for DistanceFactors {
    fn default() -> Self {
        Self {
            per_1000_ft_density_altitude: 0.10,
            per_10_kt_headwind: -0.10,
            per_10_kt_tailwind: 0.40,
            grass: 0.20,
            wet: 0.15,
            per_percent_slope: 0.10,
        }
    }
}

/// Base distances (ISA sea level, MTOW, paved level dry runway — the POH
/// reference condition) plus correction factors and the recommended
/// regulatory safety margins.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Distances {
    /// Takeoff ground roll.
    pub takeoff_roll: Meters,
    /// Takeoff distance over a 50 ft obstacle, if published.
    pub takeoff_over_50ft: Option<Meters>,
    /// Landing ground roll.
    pub landing_roll: Meters,
    /// Landing distance over a 50 ft obstacle, if published.
    pub landing_over_50ft: Option<Meters>,
    /// Overall safety factor on the corrected takeoff distance.
    /// Template: 1.33 (recommended for non-commercial operations).
    pub takeoff_safety_factor: f64,
    /// Overall safety factor on the corrected landing distance.
    /// Template: 1.43.
    pub landing_safety_factor: f64,
    pub factors: DistanceFactors,
}

impl Default for Distances {
    fn default() -> Self {
        Self {
            takeoff_roll: Meters(0.0),
            takeoff_over_50ft: None,
            landing_roll: Meters(0.0),
            landing_over_50ft: None,
            takeoff_safety_factor: 1.33,
            landing_safety_factor: 1.43,
            factors: DistanceFactors::default(),
        }
    }
}

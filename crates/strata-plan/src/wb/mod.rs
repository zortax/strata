//! Weight & balance arithmetic (plan §3 `wb/`): loading sums, CG, envelope
//! containment, the takeoff/zero-fuel/landing states and the fuel-burn CG
//! track.
//!
//! Model: moment = mass × arm (meters aft of datum), CG arm = total moment /
//! total mass. The loading scenario's fuel quantity is converted to mass via
//! the profile's fuel density and split **equally** across the profile's
//! [`StationKind::Fuel`] stations (equal split + proportional burn is
//! equivalent to a single tank at the mean fuel arm). Everything else in the
//! scenario is fixed (zero-fuel) load.
//!
//! A state is `within_envelope` when its CG point lies inside the profile's
//! (arm, mass) envelope polygon **and** its mass does not exceed the
//! applicable certificate limit (ramp → max ramp, falling back to MTOW;
//! takeoff → MTOW; zero-fuel → MZFW; landing → MLW; absent optional limits
//! are not checked).

use serde::{Deserialize, Serialize};
use strata_data::domain::Meters;
use thiserror::Error;

#[cfg(test)]
mod tests;

use crate::aircraft::{AircraftProfile, EnvelopePoint, StationKind};
use crate::flight::LoadingScenario;
use crate::units::{Kilograms, Liters};

/// Number of points of the fuel-burn CG track (takeoff fuel down to zero,
/// inclusive of both ends, evenly spaced in fuel mass).
const BURN_TRACK_SAMPLES: usize = 17;

/// Which loading state a [`WbState`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbStateKind {
    /// At engine start (ramp mass).
    Ramp,
    Takeoff,
    ZeroFuel,
    Landing,
}

/// One computed mass-and-CG state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WbState {
    pub kind: WbStateKind,
    pub mass: Kilograms,
    /// CG arm aft of datum.
    pub arm: Meters,
    /// Inside the profile's CG envelope polygon *and* below the applicable
    /// mass limit.
    pub within_envelope: bool,
}

/// A point of the fuel-burn CG track in (arm, mass) space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CgPoint {
    pub arm: Meters,
    pub mass: Kilograms,
}

/// The full W&B result for one loading scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbReport {
    /// Ramp, takeoff, zero-fuel and landing states (in that order).
    pub states: Vec<WbState>,
    /// CG track from takeoff to zero-fuel as fuel burns, sampled at evenly
    /// spaced fuel states (a hyperbolic arc `arm = a_fuel + c / mass` in
    /// (arm, mass) space — straight only in (moment, mass) space). The
    /// landing CG lies on this track.
    pub burn_track: Vec<CgPoint>,
}

/// Errors from W&B computation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WbError {
    #[error("loading references unknown station {0:?}")]
    UnknownStation(String),
    #[error("aircraft profile has no CG envelope")]
    NoEnvelope,
    #[error("loading has fuel on board but the aircraft profile has no fuel station")]
    NoFuelStation,
}

/// Computes ramp/takeoff/zero-fuel/landing states and the burn track for
/// `loading`, with `taxi_fuel` burned before takeoff and `trip_fuel`
/// burned by landing.
///
/// Fuel quantities are clamped to physical bounds: taxi fuel can at most
/// drain the tanks, trip fuel can at most drain what is left at takeoff
/// (the *fuel ladder* — not W&B — flags under-fueling).
///
/// Errors when the envelope has fewer than three vertices
/// ([`WbError::NoEnvelope`]), a station load references a station name the
/// profile does not have ([`WbError::UnknownStation`]), or fuel is loaded
/// without any [`StationKind::Fuel`] station ([`WbError::NoFuelStation`]).
/// Station loads naming a fuel station are accepted as fixed mass at that
/// arm (the scenario's fuel quantity is the only *burnable* mass).
pub fn compute_weight_balance(
    aircraft: &AircraftProfile,
    loading: &LoadingScenario,
    taxi_fuel: Liters,
    trip_fuel: Liters,
) -> Result<WbReport, WbError> {
    let wb = &aircraft.weight_balance;
    if wb.envelope.len() < 3 {
        return Err(WbError::NoEnvelope);
    }

    // Fixed (zero-fuel) load: empty mass + every station load at its arm.
    let mut zero_fuel_mass = wb.empty_mass.0;
    let mut zero_fuel_moment = wb.empty_mass.0 * wb.empty_arm.0;
    for load in &loading.station_loads {
        let station = wb
            .stations
            .iter()
            .find(|s| s.name == load.station)
            .ok_or_else(|| WbError::UnknownStation(load.station.clone()))?;
        zero_fuel_mass += load.mass.0;
        zero_fuel_moment += load.mass.0 * station.arm.0;
    }

    // Effective fuel arm: equal split across fuel stations = mean arm.
    let fuel_arms: Vec<f64> = wb
        .stations
        .iter()
        .filter(|s| s.kind == StationKind::Fuel)
        .map(|s| s.arm.0)
        .collect();
    let ramp_fuel = loading.fuel.0.max(0.0);
    let fuel_arm = if fuel_arms.is_empty() {
        if ramp_fuel > 0.0 {
            return Err(WbError::NoFuelStation);
        }
        0.0
    } else {
        fuel_arms.iter().sum::<f64>() / fuel_arms.len() as f64
    };

    let density = aircraft.fuel.density.0;
    let takeoff_fuel = (ramp_fuel - taxi_fuel.0.max(0.0)).max(0.0);
    let landing_fuel = (takeoff_fuel - trip_fuel.0.max(0.0)).max(0.0);

    let cg_at = |fuel_liters: f64| -> CgPoint {
        let fuel_mass = fuel_liters * density;
        let mass = zero_fuel_mass + fuel_mass;
        let moment = zero_fuel_moment + fuel_mass * fuel_arm;
        // Guard the all-zeros placeholder profile (mass 0): no meaningful CG.
        let arm = if mass > 0.0 { moment / mass } else { 0.0 };
        CgPoint {
            arm: Meters(arm),
            mass: Kilograms(mass),
        }
    };

    let state_at = |kind: WbStateKind, fuel_liters: f64| -> WbState {
        let point = cg_at(fuel_liters);
        let limit = match kind {
            // No published ramp limit: MTOW is the practical ramp limit.
            WbStateKind::Ramp => Some(wb.max_ramp.unwrap_or(wb.max_takeoff)),
            WbStateKind::Takeoff => Some(wb.max_takeoff),
            WbStateKind::ZeroFuel => wb.max_zero_fuel,
            WbStateKind::Landing => wb.max_landing,
        };
        let under_limit = limit.is_none_or(|l| point.mass.0 <= l.0);
        WbState {
            kind,
            mass: point.mass,
            arm: point.arm,
            within_envelope: under_limit && within_envelope(&wb.envelope, point),
        }
    };

    let states = vec![
        state_at(WbStateKind::Ramp, ramp_fuel),
        state_at(WbStateKind::Takeoff, takeoff_fuel),
        state_at(WbStateKind::ZeroFuel, 0.0),
        state_at(WbStateKind::Landing, landing_fuel),
    ];

    let burn_track = if takeoff_fuel > 0.0 {
        (0..BURN_TRACK_SAMPLES)
            .map(|i| {
                let fraction = i as f64 / (BURN_TRACK_SAMPLES - 1) as f64;
                cg_at(takeoff_fuel * (1.0 - fraction))
            })
            .collect()
    } else {
        // Nothing to burn: the "track" is the single zero-fuel point.
        vec![cg_at(0.0)]
    };

    Ok(WbReport { states, burn_track })
}

/// Point-in-polygon containment of `point` in the (arm, mass) envelope
/// (even-odd ray casting, the same algorithm and convention as
/// `strata_data`'s `Polygon::contains` — which is lat/lon-typed and thus
/// not directly reusable here). The ring is unclosed; an explicitly closed
/// ring merely adds a degenerate edge and changes nothing. Behavior for
/// points exactly on an edge is unspecified, matching `strata_data`.
/// Fewer than three vertices contain nothing.
pub fn within_envelope(envelope: &[EnvelopePoint], point: CgPoint) -> bool {
    if envelope.len() < 3 {
        return false;
    }
    let (px, py) = (point.arm.0, point.mass.0);
    let mut inside = false;
    let mut j = envelope.len() - 1;
    for i in 0..envelope.len() {
        let (xi, yi) = (envelope[i].arm.0, envelope[i].mass.0);
        let (xj, yj) = (envelope[j].arm.0, envelope[j].mass.0);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

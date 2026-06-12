//! Wind triangle and per-leg wind resolution (plan §3 `wind/`).
//!
//! The sampler trait lives in [`crate::sources`]; the manual override model
//! ([`ManualWind`](crate::flight::ManualWind)) is part of the flight
//! document.
//!
//! Three concerns live here:
//!
//! - [`solve_wind_triangle`] — the classic triangle: wind correction angle,
//!   true heading and ground speed from true track, TAS and wind, plus
//!   [`magnetic_at`] for the true→magnetic step via a
//!   [`MagvarSource`](crate::sources::MagvarSource) at the leg midpoint.
//! - [`leg_winds`] — per-leg wind resolution over a route: the document's
//!   manual override beats the sampled model; sampling happens at the leg
//!   midpoint, planned altitude and estimated passage time.
//! - [`interpolate_levels`] — vertical interpolation between ICON pressure
//!   levels for app-side [`WindsAloftSampler`](crate::sources::WindsAloftSampler)
//!   implementations (plan §2.2).

mod aloft;
mod legs;
#[cfg(test)]
mod tests;
mod triangle;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sources::{SourceError, WindsAloft};
use crate::units::{DegreesTrue, Knots};

pub use aloft::{PressureLevelSample, interpolate_levels, wind_from_components};
pub use legs::leg_winds;
pub use triangle::{magnetic_at, solve_wind_triangle};

/// Solution of the wind triangle for one track/TAS/wind combination.
/// True-referenced; magnetic conversion happens in the nav log with the
/// per-leg variation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindTriangle {
    /// Wind correction angle in degrees, positive = correct right.
    pub wind_correction_angle_deg: f64,
    /// True heading = true track + WCA.
    pub true_heading: DegreesTrue,
    pub ground_speed: Knots,
}

/// Where a leg's wind came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegWindOrigin {
    /// Sampled from the gridded winds-aloft model at planned altitude/ETA.
    Sampled,
    /// The documented calm-ISA fallback: sampling was impossible (no
    /// departure time / planned altitude) or the model had no data, so the
    /// leg solved with a 0 kt wind and ISA temperature. Distinct from
    /// [`Self::Sampled`] so the briefing surfaces can say "ISA estimate —
    /// no forecast data" instead of passing the assumption off as a sample.
    IsaFallback,
    /// The document's manual per-leg override.
    Manual,
}

/// Wind and solved triangle for one leg.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LegWind {
    pub leg_index: usize,
    /// The wind used (sampled, fallback or override). Its OAT carries a
    /// [`Provenance`](crate::sources::Provenance): real where interpolated
    /// from fetched temperature grids, ISA for manual overrides and the
    /// calm-ISA fallback.
    pub wind: WindsAloft,
    pub origin: LegWindOrigin,
    pub triangle: WindTriangle,
}

/// Errors from wind resolution.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WindError {
    /// The wind is too strong relative to TAS for the aircraft to hold the
    /// track: either the crosswind component exceeds TAS (the WCA equation
    /// has no solution) or the resulting ground speed is not positive.
    /// (A pure tailwind stronger than TAS *is* solvable — the track can be
    /// held and ground speed stays positive.)
    #[error("wind {wind_speed:?} kt at TAS {tas:?} kt: no wind-triangle solution")]
    Unsolvable { tas: Knots, wind_speed: Knots },
    #[error(transparent)]
    Source(#[from] SourceError),
}

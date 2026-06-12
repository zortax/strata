//! The classic wind triangle and the true→magnetic step.
//!
//! Derivation (all angles relative to the desired true track `TT`):
//! the wind blows **from** `wind_from`, so its velocity vector points
//! toward `wind_from + 180°`. Decomposed onto the track,
//!
//! ```text
//! crosswind = wind_speed · sin(wind_from − TT)   (+ = wind from the right)
//! headwind  = wind_speed · cos(wind_from − TT)   (+ = headwind)
//! ```
//!
//! Holding the track requires the air-velocity cross-track component to
//! cancel the wind's: `TAS · sin(WCA) = crosswind`, hence
//!
//! ```text
//! WCA = asin(crosswind / TAS)                    (+ = correct right)
//! GS  = TAS · cos(WCA) − headwind
//! ```
//!
//! There is no solution when `|crosswind| > TAS` (the aircraft cannot hold
//! the track) or when the resulting ground speed is not positive (the
//! aircraft holds the track but does not progress along it).

use chrono::NaiveDate;
use strata_data::domain::LatLon;

use crate::sources::MagvarSource;
use crate::units::{DegreesMagnetic, DegreesTrue, Knots};

use super::{WindError, WindTriangle};

/// Solves the wind triangle: WCA, true heading and ground speed for a true
/// track flown at `tas` in a wind blowing *from* `wind_from` at
/// `wind_speed`. See the module docs for the derivation.
///
/// Errors with [`WindError::Unsolvable`] when TAS is not positive, the
/// crosswind component exceeds TAS, or the ground speed would not be
/// positive. Non-finite inputs are rejected the same way.
pub fn solve_wind_triangle(
    true_track: DegreesTrue,
    tas: Knots,
    wind_from: DegreesTrue,
    wind_speed: Knots,
) -> Result<WindTriangle, WindError> {
    let unsolvable = || WindError::Unsolvable { tas, wind_speed };

    // Finite checks first: the comparisons below are then well-defined
    // (NaN inputs fail `is_finite` and are rejected here).
    if !tas.0.is_finite() || !wind_speed.0.is_finite() || tas.0 <= 0.0 || wind_speed.0 < 0.0 {
        return Err(unsolvable());
    }

    // Defensive normalization: the newtype fields are public, so literals
    // may carry raw angles.
    let track = DegreesTrue::new(true_track.0);
    let from = DegreesTrue::new(wind_from.0);
    if !track.0.is_finite() || !from.0.is_finite() {
        return Err(unsolvable());
    }

    let relative = (from.0 - track.0).to_radians();
    let crosswind = wind_speed.0 * relative.sin();
    let headwind = wind_speed.0 * relative.cos();

    // All inputs are finite and TAS is positive, so both `sin_wca` and
    // `ground_speed` are finite from here on.
    let sin_wca = crosswind / tas.0;
    if sin_wca.abs() > 1.0 {
        return Err(unsolvable());
    }
    let wca = sin_wca.asin();
    let ground_speed = tas.0 * wca.cos() - headwind;
    if ground_speed <= 0.0 {
        return Err(unsolvable());
    }

    let wca_deg = wca.to_degrees();
    Ok(WindTriangle {
        wind_correction_angle_deg: wca_deg,
        true_heading: DegreesTrue::new(track.0 + wca_deg),
        ground_speed: Knots(ground_speed),
    })
}

/// Converts a true angle (heading or track) to magnetic with the variation
/// from `magvar` at the leg midpoint and flight date — "east is least":
/// an east-positive variation is *subtracted* (see
/// [`DegreesTrue::to_magnetic`]).
pub fn magnetic_at(
    angle: DegreesTrue,
    leg_midpoint: LatLon,
    date: NaiveDate,
    magvar: &dyn MagvarSource,
) -> Result<DegreesMagnetic, WindError> {
    let variation = magvar.magvar(leg_midpoint, date)?;
    Ok(angle.to_magnetic(variation))
}

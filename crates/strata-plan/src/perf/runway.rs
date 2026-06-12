//! Takeoff/landing distance correction chain, head/crosswind decomposition
//! and the runway margin check (design §4).
//!
//! **The chain** multiplies the profile's base distance (POH reference:
//! ISA sea level, level dry paved runway) by one factor per effect, then by
//! the overall safety factor:
//!
//! ```text
//! corrected = base
//!           · (1 + f.per_1000_ft_density_altitude · max(DA, 0) / 1000 ft)
//!           · wind        (headwind credit or tailwind penalty, linear per 10 kt)
//!           · surface     (1 + f.grass for any unpaved surface)
//!           · wet         (1 + f.wet when wet)
//!           · slope       (1 + f.per_percent_slope per % of *adverse* slope)
//!           · safety factor
//! ```
//!
//! **Conservative clamps, documented:** below-ISA density altitude earns no
//! credit (DA clamped at 0 ft); favorable slope earns no credit (only
//! upslope on takeoff / downslope on landing is factored); the headwind
//! credit is floored so the wind factor never goes negative. Snow/ice/
//! water/unknown surfaces apply the unpaved (`grass`) factor as a *floor* —
//! real contaminated-runway performance needs POH data, which the
//! single-factor model cannot express.

use serde::{Deserialize, Serialize};
use strata_data::domain::{Meters, RunwaySurface};

use crate::aircraft::{AircraftProfile, DistanceFactors};
use crate::units::{DegreesTrue, Knots};

use super::{PerfError, RunwayConditions, density_altitude};

/// Wind decomposed onto a runway.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindComponents {
    /// Along-runway component; positive = headwind, negative = tailwind.
    pub headwind: Knots,
    /// Across-runway component; positive = wind from the **right** of the
    /// runway heading, negative = from the left.
    pub crosswind: Knots,
}

/// Decomposes a wind blowing *from* `wind_from` at `wind_speed` onto a
/// runway with true heading `runway_true_heading`:
/// `headwind = speed · cos(Δ)`, `crosswind = speed · sin(Δ)` with
/// `Δ = wind_from − runway heading`.
pub fn wind_components(
    runway_true_heading: DegreesTrue,
    wind_from: DegreesTrue,
    wind_speed: Knots,
) -> WindComponents {
    let relative =
        (DegreesTrue::new(wind_from.0).0 - DegreesTrue::new(runway_true_heading.0).0).to_radians();
    WindComponents {
        headwind: Knots(wind_speed.0 * relative.cos()),
        crosswind: Knots(wind_speed.0 * relative.sin()),
    }
}

/// Required vs declared runway length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RunwayMargin {
    /// Corrected (safety-factored) distance required.
    pub required: Meters,
    /// Declared length available.
    pub available: Meters,
    /// `available − required`; negative = the runway is too short.
    pub margin: Meters,
    /// `available / required`; ≥ 1 means the distance fits. Compared
    /// against
    /// [`ConflictThresholds::min_runway_margin_ratio`](crate::conflict::ConflictThresholds::min_runway_margin_ratio)
    /// by the conflict engine.
    pub ratio: f64,
}

/// Margin of a `required` distance (from [`takeoff_distance`] /
/// [`landing_distance`], always positive) against the declared `available`
/// length.
pub fn runway_margin(required: Meters, available: Meters) -> RunwayMargin {
    RunwayMargin {
        required,
        available,
        margin: Meters(available.0 - required.0),
        ratio: available.0 / required.0,
    }
}

/// Corrected takeoff distance: the profile's base takeoff roll through the
/// correction chain (module docs) times the takeoff safety factor. Upslope
/// is adverse on takeoff.
pub fn takeoff_distance(
    aircraft: &AircraftProfile,
    conditions: &RunwayConditions,
) -> Result<Meters, PerfError> {
    corrected_distance(
        aircraft.distances.takeoff_roll,
        "takeoff roll",
        aircraft.distances.takeoff_safety_factor,
        &aircraft.distances.factors,
        conditions,
        SlopeAdverse::Upslope,
    )
}

/// Corrected landing distance; see [`takeoff_distance`]. Downslope is
/// adverse on landing.
pub fn landing_distance(
    aircraft: &AircraftProfile,
    conditions: &RunwayConditions,
) -> Result<Meters, PerfError> {
    corrected_distance(
        aircraft.distances.landing_roll,
        "landing roll",
        aircraft.distances.landing_safety_factor,
        &aircraft.distances.factors,
        conditions,
        SlopeAdverse::Downslope,
    )
}

/// Which slope sign is adverse for the assessed run.
enum SlopeAdverse {
    Upslope,
    Downslope,
}

fn corrected_distance(
    base: Meters,
    base_name: &str,
    safety_factor: f64,
    factors: &DistanceFactors,
    conditions: &RunwayConditions,
    adverse: SlopeAdverse,
) -> Result<Meters, PerfError> {
    if !base.0.is_finite() || base.0 <= 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "{base_name} is not set (must be > 0 m, got {} m)",
            base.0
        )));
    }
    if !safety_factor.is_finite() || safety_factor <= 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "safety factor must be > 0 (got {safety_factor})"
        )));
    }

    // Density altitude, clamped: no credit for below-ISA conditions.
    let da_feet = density_altitude(
        conditions.field_elevation,
        conditions.qnh,
        conditions.temperature,
    )
    .as_feet()
    .max(0.0);
    let da_factor = 1.0 + factors.per_1000_ft_density_altitude * da_feet / 1000.0;

    // Wind: linear per 10 kt of component; the template headwind factor is
    // negative (credit). Floored at 0 so extreme headwinds beyond the
    // linear model's validity cannot produce a negative distance.
    let headwind = conditions.headwind_component.0;
    let wind_factor = if headwind >= 0.0 {
        1.0 + factors.per_10_kt_headwind * headwind / 10.0
    } else {
        1.0 + factors.per_10_kt_tailwind * (-headwind) / 10.0
    }
    .max(0.0);

    let surface_factor = if is_paved(conditions.surface) {
        1.0
    } else {
        1.0 + factors.grass
    };
    let wet_factor = if conditions.wet {
        1.0 + factors.wet
    } else {
        1.0
    };

    // Slope: only the adverse sign is factored, no credit for the
    // favorable direction.
    let adverse_percent = match adverse {
        SlopeAdverse::Upslope => conditions.slope_percent.max(0.0),
        SlopeAdverse::Downslope => (-conditions.slope_percent).max(0.0),
    };
    let slope_factor = 1.0 + factors.per_percent_slope * adverse_percent;

    let corrected = base.0
        * da_factor
        * wind_factor
        * surface_factor
        * wet_factor
        * slope_factor
        * safety_factor;
    if !corrected.is_finite() || corrected < 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "{base_name} correction chain produced {corrected} m — check the distance factors"
        )));
    }
    Ok(Meters(corrected))
}

/// Paved surfaces take the POH reference distance; everything else
/// (including unknown) conservatively applies the unpaved factor.
fn is_paved(surface: RunwaySurface) -> bool {
    matches!(surface, RunwaySurface::Asphalt | RunwaySurface::Concrete)
}

//! Vertical interpolation between ICON pressure levels (plan §2.2).
//!
//! The gridded model publishes wind/temperature on **pressure surfaces**
//! (950/850/700/500 hPa). To sample at a planned altitude, each level is
//! pinned at its ICAO standard-atmosphere altitude
//! ([`PressureLevel::isa_altitude`]: 950 hPa ≈ 540 m, 850 ≈ 1457 m,
//! 700 ≈ 3012 m, 500 ≈ 5574 m) and the wind **components** and temperature
//! are interpolated linearly in geometric altitude between the two
//! bracketing levels.
//!
//! **Approximations, documented:**
//!
//! - The true altitude of a pressure surface varies with the actual weather
//!   (order ±100–300 m in mid-latitudes); the ISA mapping is the
//!   conventional planning approach, never an altimetry source.
//! - Interpolation is linear per u/v component (not in speed/direction
//!   space), which is well-defined across direction wrap-around and slightly
//!   smooths speed through a turning wind — fine at planning resolution.
//! - Altitudes **outside the level span are clamped** to the nearest level:
//!   below ≈540 m AMSL the 950 hPa wind is used unchanged (no surface-wind
//!   model), above ≈5574 m the 500 hPa wind — both outside normal German
//!   VFR cruise bands anyway.

use serde::{Deserialize, Serialize};
use strata_data::domain::{MetersAmsl, PressureLevel};

use crate::sources::{Provenance, WindsAloft};
use crate::units::{Celsius, DegreesTrue, Knots, METERS_PER_NAUTICAL_MILE};

/// Model values on one pressure surface at a fixed position and time, in
/// the provider's units (m/s components, °C) — what an app-side
/// [`WindsAloftSampler`](crate::sources::WindsAloftSampler) reads from the
/// gridded cache before calling [`interpolate_levels`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PressureLevelSample {
    pub level: PressureLevel,
    /// Eastward wind component, m/s (positive toward east).
    pub wind_u: f64,
    /// Northward wind component, m/s (positive toward north).
    pub wind_v: f64,
    pub temperature: Celsius,
    /// Whether `temperature` came from a fetched temperature grid
    /// ([`Provenance::Real`]) or is the ISA temperature at the level's ISA
    /// altitude ([`Provenance::Isa`] — the documented fallback while the
    /// grid is missing).
    pub temperature_provenance: Provenance,
}

/// Converts a wind vector (`u` eastward, `v` northward, m/s) into the
/// direction it blows **from** (true) and its speed in knots. A calm vector
/// reports 0° by convention.
pub fn wind_from_components(u_mps: f64, v_mps: f64) -> (DegreesTrue, Knots) {
    let speed_mps = u_mps.hypot(v_mps);
    let speed = Knots(speed_mps * 3600.0 / METERS_PER_NAUTICAL_MILE);
    if speed_mps < 1e-9 {
        return (DegreesTrue::new(0.0), Knots(0.0));
    }
    // The vector points toward (u, v); it blows from the opposite bearing.
    // Bearings are clockwise from north: atan2(east, north).
    let from = (-u_mps).atan2(-v_mps).to_degrees();
    (DegreesTrue::new(from), speed)
}

/// Interpolates pressure-level samples to `altitude` (see module docs for
/// the ISA mapping, component-wise interpolation and clamping). Samples may
/// arrive in any order; duplicate levels keep the first occurrence. Returns
/// `None` for an empty slice.
///
/// **Temperature provenance:** the result is [`Provenance::Real`] only when
/// every level it was derived from carried real data — a value mixed from
/// one real and one ISA level is honestly labelled [`Provenance::Isa`]
/// (conservative: "Real" never means "partly assumed").
pub fn interpolate_levels(
    samples: &[PressureLevelSample],
    altitude: MetersAmsl,
) -> Option<WindsAloft> {
    // At most four levels exist; collect + sort by ISA altitude (ascending).
    let mut sorted: Vec<&PressureLevelSample> = Vec::with_capacity(samples.len());
    for sample in samples {
        if !sorted.iter().any(|s| s.level == sample.level) {
            sorted.push(sample);
        }
    }
    sorted.sort_by(|a, b| {
        a.level
            .isa_altitude()
            .0
            .total_cmp(&b.level.isa_altitude().0)
    });

    let (first, last) = (*sorted.first()?, *sorted.last()?);

    // Clamp outside the span; a single level is constant everywhere.
    if altitude.0 <= first.level.isa_altitude().0 {
        return Some(to_winds_aloft(
            first.wind_u,
            first.wind_v,
            first.temperature.0,
            first.temperature_provenance,
        ));
    }
    if altitude.0 >= last.level.isa_altitude().0 {
        return Some(to_winds_aloft(
            last.wind_u,
            last.wind_v,
            last.temperature.0,
            last.temperature_provenance,
        ));
    }

    // Find the bracketing pair and interpolate linearly in altitude.
    for pair in sorted.windows(2) {
        let (lower, upper) = (pair[0], pair[1]);
        let (h0, h1) = (lower.level.isa_altitude().0, upper.level.isa_altitude().0);
        if altitude.0 <= h1 {
            let t = (altitude.0 - h0) / (h1 - h0);
            let u = lower.wind_u + t * (upper.wind_u - lower.wind_u);
            let v = lower.wind_v + t * (upper.wind_v - lower.wind_v);
            let temperature = lower.temperature.0 + t * (upper.temperature.0 - lower.temperature.0);
            let provenance = combined_provenance(
                lower.temperature_provenance,
                upper.temperature_provenance,
            );
            return Some(to_winds_aloft(u, v, temperature, provenance));
        }
    }
    // Unreachable: the clamp above bounds `altitude` inside the span.
    None
}

/// Real only when both contributing levels are real (see
/// [`interpolate_levels`]'s provenance note).
fn combined_provenance(lower: Provenance, upper: Provenance) -> Provenance {
    match (lower, upper) {
        (Provenance::Real, Provenance::Real) => Provenance::Real,
        _ => Provenance::Isa,
    }
}

fn to_winds_aloft(
    u_mps: f64,
    v_mps: f64,
    temperature_c: f64,
    temperature_provenance: Provenance,
) -> WindsAloft {
    let (direction, speed) = wind_from_components(u_mps, v_mps);
    WindsAloft {
        direction,
        speed,
        temperature: Celsius(temperature_c),
        temperature_provenance,
    }
}

//! Pressure and density altitude — the classic E6B rules of thumb.
//!
//! **Model, documented:**
//!
//! - Pressure altitude: `PA = field elevation + (1013.25 − QNH) · 27.3 ft`
//!   per hectopascal — the linearized ISA sea-level pressure gradient
//!   (1 hPa ≈ 8.32 m ≈ 27.3 ft). Exact at sea level, a few feet off per
//!   thousand feet of elevation; planning-grade.
//! - Density altitude: `DA = PA + 120 ft · (OAT − ISA(PA))` — the
//!   conventional 120 ft per °C of ISA deviation (E6B/POH rule of thumb;
//!   the exact coefficient is ≈118.8 near sea level).
//!
//! Both are planning aids, never altimetry.

use strata_data::domain::{FEET_PER_METER, MetersAmsl, Qnh};

use crate::units::Celsius;

use super::isa::isa_temperature;

/// ISA sea-level pressure, hPa.
const ISA_SEA_LEVEL_HPA: f64 = 1013.25;

/// Linearized ISA pressure gradient at sea level, feet per hectopascal.
const FEET_PER_HECTOPASCAL: f64 = 27.3;

/// Density-altitude increase per °C of ISA deviation, feet.
const FEET_PER_ISA_DEVIATION_CELSIUS: f64 = 120.0;

/// Pressure altitude of a field: elevation corrected onto the 1013.25 hPa
/// datum (see module docs for the linearization).
pub fn pressure_altitude(field_elevation: MetersAmsl, qnh: Qnh) -> MetersAmsl {
    let delta_feet = (ISA_SEA_LEVEL_HPA - f64::from(qnh.as_hpa())) * FEET_PER_HECTOPASCAL;
    MetersAmsl(field_elevation.0 + delta_feet / FEET_PER_METER)
}

/// Density altitude from field elevation, QNH and outside air temperature:
/// pressure altitude plus 120 ft per °C the OAT exceeds the ISA temperature
/// *at that pressure altitude* (see module docs).
///
/// `temperature` is the **actual OAT**: callers pass real data where
/// available — a METAR observation, or the winds-aloft sampler's OAT
/// (which carries a [`Provenance`](crate::sources::Provenance) since real
/// ICON temperature grids were wired) — and fall back to
/// [`isa_temperature`](super::isa_temperature) only as a *labelled*
/// estimate (an ISA OAT makes the deviation term vanish by construction,
/// silently reducing DA to PA).
///
/// Monotone by construction: rises with temperature and elevation, falls
/// with QNH. Can be below field elevation (cold/high-pressure days) — the
/// runway-distance chain clamps its DA credit separately, this function
/// reports the physical estimate.
pub fn density_altitude(field_elevation: MetersAmsl, qnh: Qnh, temperature: Celsius) -> MetersAmsl {
    let pa = pressure_altitude(field_elevation, qnh);
    let isa_deviation = temperature.0 - isa_temperature(pa).0;
    MetersAmsl(pa.0 + FEET_PER_ISA_DEVIATION_CELSIUS * isa_deviation / FEET_PER_METER)
}

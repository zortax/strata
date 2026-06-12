//! ICAO standard-atmosphere helpers and the planned-altitude → AMSL edge.
//!
//! Tropospheric model only (linear lapse, valid to 11 km — far above any
//! VFR planning altitude).

use strata_data::domain::MetersAmsl;

use crate::flight::PlannedAltitude;
use crate::units::Celsius;

/// ISA mean sea level temperature, °C.
pub const ISA_SEA_LEVEL_CELSIUS: f64 = 15.0;

/// ISA tropospheric lapse rate, °C per meter (6.5 °C/km).
pub const ISA_LAPSE_CELSIUS_PER_METER: f64 = 0.0065;

/// ISA temperature at `altitude`: `15 °C − 6.5 °C/km · h`.
pub fn isa_temperature(altitude: MetersAmsl) -> Celsius {
    Celsius(ISA_SEA_LEVEL_CELSIUS - ISA_LAPSE_CELSIUS_PER_METER * altitude.0)
}

/// Resolves a [`PlannedAltitude`] to meters AMSL for the vertical profile
/// and wind sampling.
///
/// **Approximation, documented:** a flight level is a *pressure* altitude
/// (FL `nn` = `nn`·100 ft on the 1013.25 hPa datum); treating it as
/// geometric AMSL ignores the actual QNH — a few hundred feet of error in
/// typical mid-latitude weather, consistent with the planning-grade ISA
/// conventions used throughout (cf.
/// `strata_data::domain::PressureLevel::isa_altitude`). Never an altimetry
/// source.
pub fn planned_altitude_amsl(altitude: PlannedAltitude) -> MetersAmsl {
    match altitude {
        PlannedAltitude::Amsl(meters) => meters,
        PlannedAltitude::FlightLevel(fl) => MetersAmsl::from_feet(f64::from(fl) * 100.0),
    }
}

//! Vertical limits with their datum. Meters internally; chart-style display
//! renders feet, because that is what pilots read.

use std::fmt;

use serde::{Deserialize, Serialize};

pub const FEET_PER_METER: f64 = 3.280_839_895_013_123;

/// Altitude above mean sea level, in meters.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MetersAmsl(pub f64);

impl MetersAmsl {
    pub fn from_feet(ft: f64) -> Self {
        Self(ft / FEET_PER_METER)
    }

    pub fn as_feet(self) -> f64 {
        self.0 * FEET_PER_METER
    }
}

/// Height above ground level, in meters.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MetersAgl(pub f64);

impl MetersAgl {
    pub fn from_feet(ft: f64) -> Self {
        Self(ft / FEET_PER_METER)
    }

    pub fn as_feet(self) -> f64 {
        self.0 * FEET_PER_METER
    }
}

/// A vertical position together with its reference datum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum VerticalReference {
    /// Flight level (pressure altitude in hundreds of feet).
    Fl(u16),
    /// Above mean sea level.
    Amsl(MetersAmsl),
    /// Above ground level.
    Agl(MetersAgl),
    /// Ground / surface.
    Gnd,
    /// Unlimited.
    Unl,
}

impl fmt::Display for VerticalReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fl(n) => write!(f, "FL {n}"),
            Self::Amsl(m) => write!(f, "{} ft MSL", m.as_feet().round() as i64),
            Self::Agl(m) => write!(f, "{} ft AGL", m.as_feet().round() as i64),
            Self::Gnd => write!(f, "GND"),
            Self::Unl => write!(f, "UNL"),
        }
    }
}

/// A lower or upper airspace limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VerticalLimit {
    pub reference: VerticalReference,
}

impl VerticalLimit {
    pub fn fl(level: u16) -> Self {
        Self {
            reference: VerticalReference::Fl(level),
        }
    }

    pub fn amsl(altitude: MetersAmsl) -> Self {
        Self {
            reference: VerticalReference::Amsl(altitude),
        }
    }

    pub fn agl(height: MetersAgl) -> Self {
        Self {
            reference: VerticalReference::Agl(height),
        }
    }

    pub fn gnd() -> Self {
        Self {
            reference: VerticalReference::Gnd,
        }
    }

    pub fn unl() -> Self {
        Self {
            reference: VerticalReference::Unl,
        }
    }
}

impl From<VerticalReference> for VerticalLimit {
    fn from(reference: VerticalReference) -> Self {
        Self { reference }
    }
}

impl fmt::Display for VerticalLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reference.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_flight_level() {
        assert_eq!(VerticalReference::Fl(100).to_string(), "FL 100");
        assert_eq!(VerticalReference::Fl(65).to_string(), "FL 65");
    }

    #[test]
    fn display_amsl_in_feet() {
        let limit = VerticalLimit::amsl(MetersAmsl::from_feet(2500.0));
        assert_eq!(limit.to_string(), "2500 ft MSL");
    }

    #[test]
    fn display_agl_in_feet() {
        let limit = VerticalLimit::agl(MetersAgl::from_feet(1000.0));
        assert_eq!(limit.to_string(), "1000 ft AGL");
    }

    #[test]
    fn display_gnd_and_unl() {
        assert_eq!(VerticalLimit::gnd().to_string(), "GND");
        assert_eq!(VerticalLimit::unl().to_string(), "UNL");
    }

    #[test]
    fn display_rounds_metric_values_to_whole_feet() {
        // 1000 m = 3280.84 ft -> rounds to 3281.
        let limit = VerticalLimit::amsl(MetersAmsl(1000.0));
        assert_eq!(limit.to_string(), "3281 ft MSL");
    }

    #[test]
    fn feet_round_trip() {
        let m = MetersAmsl::from_feet(4500.0);
        assert!((m.as_feet() - 4500.0).abs() < 1e-9);
        let agl = MetersAgl::from_feet(700.0);
        assert!((agl.as_feet() - 700.0).abs() < 1e-9);
    }
}

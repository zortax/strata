//! Airports, runways, and radio frequencies.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::geo::{LatLon, Meters};
use super::vertical::MetersAmsl;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum IcaoCodeError {
    #[error("invalid ICAO location indicator {0:?} (expected 4 ASCII letters/digits)")]
    Invalid(String),
}

/// A 4-character ICAO location indicator (e.g. `EDDB`). Stored uppercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct IcaoCode(String);

impl IcaoCode {
    pub fn new(code: &str) -> Result<Self, IcaoCodeError> {
        if code.len() == 4 && code.chars().all(|c| c.is_ascii_alphanumeric()) {
            Ok(Self(code.to_ascii_uppercase()))
        } else {
            Err(IcaoCodeError::Invalid(code.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for IcaoCode {
    type Error = IcaoCodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<IcaoCode> for String {
    fn from(code: IcaoCode) -> Self {
        code.0
    }
}

impl fmt::Display for IcaoCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A radio frequency, stored in hertz (covers NDB kHz and VHF MHz uniformly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RadioFrequency(u32);

impl RadioFrequency {
    pub fn from_hz(hz: u32) -> Self {
        Self(hz)
    }

    pub fn from_khz(khz: f64) -> Self {
        Self((khz * 1_000.0).round() as u32)
    }

    pub fn from_mhz(mhz: f64) -> Self {
        Self((mhz * 1_000_000.0).round() as u32)
    }

    pub fn hz(self) -> u32 {
        self.0
    }

    pub fn khz(self) -> f64 {
        f64::from(self.0) / 1_000.0
    }

    pub fn mhz(self) -> f64 {
        f64::from(self.0) / 1_000_000.0
    }
}

impl fmt::Display for RadioFrequency {
    /// VHF and above render as MHz with kHz resolution ("118.105 MHz"),
    /// NDB-range frequencies as kHz ("341 kHz" / "341.5 kHz").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 >= 30_000_000 {
            write!(f, "{:.3} MHz", self.mhz())
        } else {
            let khz = self.khz();
            if khz.fract() == 0.0 {
                write!(f, "{} kHz", khz as u32)
            } else {
                write!(f, "{khz:.1} kHz")
            }
        }
    }
}

/// What kind of landing site this is. `Other` carries the raw source code
/// (openAIP airport `type`) for kinds without a dedicated variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AirportKind {
    International,
    Regional,
    Airfield,
    GliderSite,
    Heliport,
    MilitaryAerodrome,
    UltraLightSite,
    WaterAirfield,
    LandingStrip,
    Closed,
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunwaySurface {
    Asphalt,
    Concrete,
    Grass,
    Sand,
    Water,
    Gravel,
    Earth,
    Snow,
    Ice,
    /// Raw source surface code without a dedicated variant.
    Other(u16),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runway {
    /// Direction designator as published, e.g. "07", "09R".
    pub designator: String,
    pub true_heading_deg: Option<u16>,
    pub length: Option<Meters>,
    pub width: Option<Meters>,
    pub surface: RunwaySurface,
    /// Marked as the main runway of the airport.
    pub main: bool,
}

/// Service kind of a COM frequency. `Other` carries the raw source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrequencyKind {
    Approach,
    Apron,
    Arrival,
    Center,
    Ctaf,
    Delivery,
    Departure,
    Fis,
    Gliding,
    Ground,
    Information,
    Multicom,
    Unicom,
    Radar,
    Tower,
    Atis,
    Radio,
    Awos,
    Volmet,
    Afis,
    Other(u16),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frequency {
    pub frequency: RadioFrequency,
    /// Published station name, e.g. "LANGEN INFORMATION".
    pub name: String,
    pub kind: FrequencyKind,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Airport {
    /// `None` for fields without an ICAO location indicator.
    pub ident: Option<IcaoCode>,
    pub name: String,
    pub kind: AirportKind,
    pub position: LatLon,
    pub elevation: MetersAmsl,
    pub runways: Vec<Runway>,
    pub frequencies: Vec<Frequency>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icao_code_validation() {
        assert_eq!(IcaoCode::new("eddb").unwrap().as_str(), "EDDB");
        assert!(IcaoCode::new("EDDB").is_ok());
        assert!(IcaoCode::new("ED").is_err());
        assert!(IcaoCode::new("EDDBX").is_err());
        assert!(IcaoCode::new("ED B").is_err());
    }

    #[test]
    fn radio_frequency_display() {
        assert_eq!(RadioFrequency::from_mhz(118.105).to_string(), "118.105 MHz");
        assert_eq!(RadioFrequency::from_mhz(122.88).to_string(), "122.880 MHz");
        assert_eq!(RadioFrequency::from_khz(341.0).to_string(), "341 kHz");
        assert_eq!(RadioFrequency::from_khz(341.5).to_string(), "341.5 kHz");
    }

    #[test]
    fn radio_frequency_round_trip() {
        let f = RadioFrequency::from_mhz(128.955);
        assert_eq!(f.hz(), 128_955_000);
        assert!((f.mhz() - 128.955).abs() < 1e-9);
    }
}

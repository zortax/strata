//! Airspace volumes.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::airac::AiracCycle;
use super::geo::Polygon;
use super::vertical::VerticalLimit;

/// ICAO airspace class. `Unclassified` covers special-use airspace that
/// openAIP reports without an ICAO class (icaoClass code 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AirspaceClass {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    Unclassified,
}

impl fmt::Display for AirspaceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::Unclassified => "—",
        };
        f.write_str(s)
    }
}

/// Operational kind of an airspace volume, mirroring the full openAIP
/// airspace `type` enumeration (codes 0..=36). [`AirspaceKind::Area`] is the
/// source's generic catch-all (type 0 "Other") — a *known* generic area,
/// usually labelled by its ICAO class alone. [`AirspaceKind::Other`] carries
/// the raw source type code for genuinely unknown codes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AirspaceKind {
    /// Generic area without a more specific kind (openAIP type 0 "Other").
    Area,
    /// Control zone.
    Ctr,
    /// Military control zone (MCTR).
    Mctr,
    /// Transponder mandatory zone.
    Tmz,
    /// Radio mandatory zone.
    Rmz,
    /// Danger area (ED-D).
    Danger,
    /// Restricted area (ED-R).
    Restricted,
    /// Prohibited area (ED-P).
    Prohibited,
    /// Terminal maneuvering area / terminal control area.
    Tma,
    /// Temporary reserved area.
    Tra,
    /// Temporary segregated area.
    Tsa,
    /// Control area.
    Cta,
    /// Airport traffic zone.
    Atz,
    /// Military airport traffic zone.
    Matz,
    /// Helicopter traffic zone.
    Htz,
    /// Traffic information zone.
    Tiz,
    /// Traffic information area.
    Tia,
    GliderSector,
    ParachuteJumpArea,
    /// Aerial sporting or recreational activity area.
    RecreationalActivity,
    /// Flight information region.
    Fir,
    /// Upper flight information region.
    Uir,
    /// Flight information service sector.
    FisSector,
    /// VFR sector.
    VfrSector,
    /// ACC (area control center) sector.
    AccSector,
    /// Air defense identification zone.
    Adiz,
    Airway,
    /// Military training route (MTR).
    MilitaryTrainingRoute,
    /// Military route (MRT).
    MilitaryRoute,
    /// Military training area.
    MilitaryTrainingArea,
    /// TSA/TRA feeding route (TFR).
    TsaTraFeedingRoute,
    AlertArea,
    WarningArea,
    ProtectedArea,
    /// Low altitude overflight restriction.
    OverflightRestriction,
    /// Transponder setting area (TRP).
    TransponderSetting,
    /// Lower traffic area.
    LowerTrafficArea,
    /// Upper traffic area.
    UpperTrafficArea,
    /// Unknown source type code without a dedicated variant.
    Other(u16),
}

impl fmt::Display for AirspaceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Area => f.write_str("Area"),
            Self::Ctr => f.write_str("CTR"),
            Self::Mctr => f.write_str("MCTR"),
            Self::Tmz => f.write_str("TMZ"),
            Self::Rmz => f.write_str("RMZ"),
            Self::Danger => f.write_str("Danger Area"),
            Self::Restricted => f.write_str("Restricted Area"),
            Self::Prohibited => f.write_str("Prohibited Area"),
            Self::Tma => f.write_str("TMA"),
            Self::Tra => f.write_str("TRA"),
            Self::Tsa => f.write_str("TSA"),
            Self::Cta => f.write_str("CTA"),
            Self::Atz => f.write_str("ATZ"),
            Self::Matz => f.write_str("MATZ"),
            Self::Htz => f.write_str("HTZ"),
            Self::Tiz => f.write_str("TIZ"),
            Self::Tia => f.write_str("TIA"),
            Self::GliderSector => f.write_str("Glider Sector"),
            Self::ParachuteJumpArea => f.write_str("Parachute Jump Area"),
            Self::RecreationalActivity => f.write_str("Recreational Activity"),
            Self::Fir => f.write_str("FIR"),
            Self::Uir => f.write_str("UIR"),
            Self::FisSector => f.write_str("FIS"),
            Self::VfrSector => f.write_str("VFR Sector"),
            Self::AccSector => f.write_str("ACC Sector"),
            Self::Adiz => f.write_str("ADIZ"),
            Self::Airway => f.write_str("Airway"),
            Self::MilitaryTrainingRoute => f.write_str("MTR"),
            Self::MilitaryRoute => f.write_str("MRT"),
            Self::MilitaryTrainingArea => f.write_str("MTA"),
            Self::TsaTraFeedingRoute => f.write_str("TSA/TRA Feeding Route"),
            Self::AlertArea => f.write_str("Alert Area"),
            Self::WarningArea => f.write_str("Warning Area"),
            Self::ProtectedArea => f.write_str("Protected Area"),
            Self::OverflightRestriction => f.write_str("Overflight Restriction"),
            Self::TransponderSetting => f.write_str("TRP"),
            Self::LowerTrafficArea => f.write_str("LTA"),
            Self::UpperTrafficArea => f.write_str("UTA"),
            Self::Other(code) => write!(f, "Other ({code})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Airspace {
    pub name: String,
    pub class: AirspaceClass,
    pub kind: AirspaceKind,
    pub lower: VerticalLimit,
    pub upper: VerticalLimit,
    pub geometry: Polygon,
    /// AIRAC cycle the source data belongs to, when known.
    pub airac: Option<AiracCycle>,
}

impl Airspace {
    /// Chart-style vertical band, e.g. `"FL 100 / 2500 ft MSL"`.
    pub fn vertical_band(&self) -> String {
        format!("{} / {}", self.upper, self.lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geo::LatLon;
    use crate::domain::vertical::MetersAmsl;

    #[test]
    fn vertical_band_renders_chart_style() {
        let geometry = Polygon::new(
            vec![
                LatLon::new(48.0, 9.0).unwrap(),
                LatLon::new(48.5, 9.5).unwrap(),
                LatLon::new(48.0, 10.0).unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let airspace = Airspace {
            name: "TMA TEST".into(),
            class: AirspaceClass::D,
            kind: AirspaceKind::Ctr,
            lower: VerticalLimit::amsl(MetersAmsl::from_feet(2500.0)),
            upper: VerticalLimit::fl(100),
            geometry,
            airac: None,
        };
        assert_eq!(airspace.vertical_band(), "FL 100 / 2500 ft MSL");
    }

    #[test]
    fn kind_labels_use_chart_abbreviations() {
        assert_eq!(AirspaceKind::Tma.to_string(), "TMA");
        assert_eq!(AirspaceKind::Cta.to_string(), "CTA");
        assert_eq!(AirspaceKind::Atz.to_string(), "ATZ");
        assert_eq!(AirspaceKind::Matz.to_string(), "MATZ");
        assert_eq!(AirspaceKind::Mctr.to_string(), "MCTR");
        assert_eq!(AirspaceKind::FisSector.to_string(), "FIS");
        assert_eq!(AirspaceKind::Tra.to_string(), "TRA");
        assert_eq!(AirspaceKind::Tsa.to_string(), "TSA");
        assert_eq!(AirspaceKind::Adiz.to_string(), "ADIZ");
        assert_eq!(AirspaceKind::Area.to_string(), "Area");
        assert_eq!(AirspaceKind::Other(42).to_string(), "Other (42)");
    }
}

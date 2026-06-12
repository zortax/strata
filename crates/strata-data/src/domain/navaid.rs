//! Radio navigation aids.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::airport::RadioFrequency;
use super::geo::LatLon;
use super::vertical::MetersAmsl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NavaidKind {
    Dme,
    Tacan,
    Ndb,
    Vor,
    VorDme,
    Vortac,
    Dvor,
    DvorDme,
    Dvortac,
}

impl fmt::Display for NavaidKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Dme => "DME",
            Self::Tacan => "TACAN",
            Self::Ndb => "NDB",
            Self::Vor => "VOR",
            Self::VorDme => "VOR-DME",
            Self::Vortac => "VORTAC",
            Self::Dvor => "DVOR",
            Self::DvorDme => "DVOR-DME",
            Self::Dvortac => "DVORTAC",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Navaid {
    /// Published 2–3 letter identifier, e.g. "ALG".
    pub ident: String,
    pub name: String,
    pub kind: NavaidKind,
    /// `None` for channel-only facilities (e.g. some TACANs).
    pub frequency: Option<RadioFrequency>,
    /// DME/TACAN channel, e.g. "84X".
    pub channel: Option<String>,
    pub position: LatLon,
    pub elevation: MetersAmsl,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_display() {
        assert_eq!(NavaidKind::VorDme.to_string(), "VOR-DME");
        assert_eq!(NavaidKind::Ndb.to_string(), "NDB");
    }
}

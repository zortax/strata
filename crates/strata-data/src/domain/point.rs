//! VFR reporting points.

use serde::{Deserialize, Serialize};

use super::airport::IcaoCode;
use super::geo::LatLon;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportingPoint {
    /// Published name, e.g. "ALPHA", "ECHO 1".
    pub name: String,
    /// Compulsory (mandatory) reporting point.
    pub mandatory: bool,
    pub position: LatLon,
    /// Airports this point belongs to. The openAIP normalizer resolves the
    /// source's internal airport ids to ICAO idents; unresolvable ones drop.
    pub airports: Vec<IcaoCode>,
}

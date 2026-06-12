//! Aeronautical obstacles (wind turbines, masts, towers, …).

use serde::{Deserialize, Serialize};

use super::geo::LatLon;
use super::vertical::{MetersAgl, MetersAmsl};

/// Obstacle kind. `Other` carries the raw source type code (openAIP
/// obstacle `type`) for kinds without a dedicated variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObstacleKind {
    WindTurbine,
    Antenna,
    Mast,
    Tower,
    Chimney,
    Building,
    PowerLine,
    Crane,
    Bridge,
    Other(u16),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obstacle {
    /// Published name where meaningful (openAIP OSM imports often carry
    /// synthetic names like "#784930").
    pub name: Option<String>,
    pub kind: ObstacleKind,
    pub position: LatLon,
    /// Height of the structure above ground.
    pub height: MetersAgl,
    /// Elevation of the obstacle top above mean sea level.
    pub elevation_top: MetersAmsl,
    pub lighted: bool,
}

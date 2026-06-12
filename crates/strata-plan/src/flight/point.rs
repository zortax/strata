//! Route points and waypoints.

use serde::{Deserialize, Serialize};
use strata_data::domain::{LatLon, MetersAmsl};

use crate::units::{DegreesTrue, Knots};

/// A planned altitude: an AMSL altitude or a flight level. Deliberately
/// narrower than `strata_data`'s `VerticalReference` — AGL/GND/UNL are not
/// plannable cruise values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedAltitude {
    /// Altitude above mean sea level (stored in meters, entered in feet).
    Amsl(MetersAmsl),
    /// Flight level (pressure altitude in hundreds of feet).
    FlightLevel(u16),
}

/// What kind of named feature a [`NamedPoint`] references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedPointKind {
    Airport,
    Navaid,
    ReportingPoint,
}

/// A reference to a named feature by stable identifier **plus a position
/// snapshot**: the document stays self-contained (offline, AIRAC swaps,
/// shared files) while the id lets the app re-resolve/re-snap the feature.
///
/// Stable ids per kind: airports — ICAO location indicator (`"EDDF"`);
/// navaids — published ident (`"FFM"`, disambiguated by `kind` + position);
/// reporting points — published name (e.g. `"ECHO 1"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedPoint {
    pub kind: NamedPointKind,
    pub id: String,
    /// Display name at reference time, e.g. `"Frankfurt/Main"`.
    pub name: String,
    /// Position at reference time (WGS84).
    pub position: LatLon,
}

/// A free user-placed point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreePoint {
    #[serde(default)]
    pub name: Option<String>,
    pub position: LatLon,
}

/// A point on the route — named feature or free coordinate. The enum is the
/// IFR seam (plan §3): airway/procedure points become new variants later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePoint {
    Named(NamedPoint),
    Free(FreePoint),
}

impl RoutePoint {
    /// Resolved WGS84 position (the stored snapshot for named points).
    pub fn position(&self) -> LatLon {
        match self {
            Self::Named(named) => named.position,
            Self::Free(free) => free.position,
        }
    }

    /// Stable identifier for named points (`None` for free points).
    pub fn ident(&self) -> Option<&str> {
        match self {
            Self::Named(named) => Some(&named.id),
            Self::Free(_) => None,
        }
    }

    /// Human-readable label: id for named points, name or coordinates for
    /// free points.
    pub fn label(&self) -> String {
        match self {
            Self::Named(named) => named.id.clone(),
            Self::Free(free) => free
                .name
                .clone()
                .unwrap_or_else(|| free.position.to_string()),
        }
    }
}

/// Manual per-leg wind override — the classic paper-planning workflow and
/// the offline fallback (design §4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ManualWind {
    /// True direction the wind blows *from*.
    pub direction: DegreesTrue,
    pub speed: Knots,
}

/// A route entry: the point plus the plan for the leg **from this waypoint
/// to its successor**. Leg fields on the final waypoint are meaningless and
/// cleared by [`crate::route::normalize`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteWaypoint {
    pub point: RoutePoint,
    /// Planned altitude of the outgoing leg; `None` = the flight's
    /// [`cruise_altitude`](super::FlightDoc::cruise_altitude).
    #[serde(default)]
    pub leg_altitude: Option<PlannedAltitude>,
    /// Manual wind override for the outgoing leg; `None` = sampled winds.
    #[serde(default)]
    pub leg_wind: Option<ManualWind>,
    /// Pilot notes shown on **this waypoint's nav-log row** (the PLOG
    /// "Notes" column, design §3.3) and persisted with the document.
    /// Unlike the leg fields above this annotates the *checkpoint* itself
    /// (the departure row included), so [`crate::route::normalize`] keeps
    /// it on the final waypoint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl RouteWaypoint {
    /// A waypoint with no leg overrides.
    pub fn new(point: RoutePoint) -> Self {
        Self {
            point,
            leg_altitude: None,
            leg_wind: None,
            notes: String::new(),
        }
    }

    /// Shortcut for `self.point.position()`.
    pub fn position(&self) -> LatLon {
        self.point.position()
    }
}

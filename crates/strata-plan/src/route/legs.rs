//! Leg iteration over a route.

use serde::{Deserialize, Serialize};
use strata_data::domain::{LatLon, Meters};

use crate::flight::RouteWaypoint;
use crate::units::DegreesTrue;

use super::geo;

/// Pure geometry of one leg.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LegGeometry {
    pub distance: Meters,
    pub initial_true_track: DegreesTrue,
    /// Great-circle midpoint (magnetic variation is evaluated here).
    pub midpoint: LatLon,
}

/// Geometry of the great-circle segment `from → to`.
pub fn leg_geometry(from: LatLon, to: LatLon) -> LegGeometry {
    LegGeometry {
        distance: geo::great_circle_distance(from, to),
        initial_true_track: geo::initial_true_track(from, to),
        midpoint: geo::midpoint(from, to),
    }
}

/// One leg of a route: waypoint `index` to waypoint `index + 1`. Leg-scoped
/// plan fields (altitude, wind override) live on [`Leg::from`].
#[derive(Debug, Clone, Copy)]
pub struct Leg<'a> {
    /// Leg index == index of [`Leg::from`] in the route.
    pub index: usize,
    pub from: &'a RouteWaypoint,
    pub to: &'a RouteWaypoint,
}

impl Leg<'_> {
    pub fn geometry(&self) -> LegGeometry {
        leg_geometry(self.from.position(), self.to.position())
    }
}

/// Iterates the legs of `route` in order (empty for fewer than two
/// waypoints).
pub fn legs(route: &[RouteWaypoint]) -> impl Iterator<Item = Leg<'_>> {
    route.windows(2).enumerate().map(|(index, pair)| Leg {
        index,
        from: &pair[0],
        to: &pair[1],
    })
}

/// Sum of all great-circle leg distances.
pub fn total_distance(route: &[RouteWaypoint]) -> Meters {
    Meters(
        legs(route)
            .map(|leg| leg.geometry().distance.0)
            .sum::<f64>(),
    )
}

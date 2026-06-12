//! Tallest obstacle per station.
//!
//! One bbox query over the whole padded corridor, then an exact spherical
//! lateral-distance check per obstacle: the obstacle's position is
//! projected onto every leg's great circle (plus the route vertices, which
//! cover the wedges outside both legs at a turn and the areas beyond the
//! route ends), the placement with the smallest cross-track distance wins,
//! and — if within the corridor half-width — the obstacle is assigned to
//! the station nearest to its along-track position. Per station only the
//! obstacle with the highest top elevation is kept ([`CorridorSample`]
//! semantics).
//!
//! [`CorridorSample`]: super::CorridorSample

use strata_data::domain::{BoundingBox, LatLon, Meters, Obstacle};

use crate::route::great_circle_distance;
use crate::sources::{ObstacleSource, SourceError};

use super::geometry::project_onto_leg;
use super::stations::{CumulativeLeg, TrackedStation};

/// Where an obstacle sits relative to the route.
struct Placement {
    along: f64,
    cross: f64,
}

/// The tallest in-corridor obstacle for each station (same order as
/// `stations`).
pub(super) fn tallest_per_station(
    legs: &[CumulativeLeg],
    total: f64,
    stations: &[TrackedStation],
    half_width: Meters,
    query_bbox: BoundingBox,
    source: &dyn ObstacleSource,
) -> Result<Vec<Option<Obstacle>>, SourceError> {
    let mut tallest: Vec<Option<Obstacle>> = vec![None; stations.len()];
    for obstacle in source.obstacles_in_bbox(query_bbox)? {
        let placement = place(legs, total, obstacle.position);
        if placement.cross > half_width.0 {
            continue;
        }
        let index = nearest_station(stations, placement.along);
        let replace = match &tallest[index] {
            None => true,
            Some(current) => obstacle.elevation_top.0 > current.elevation_top.0,
        };
        if replace {
            tallest[index] = Some(obstacle);
        }
    }
    Ok(tallest)
}

/// Best placement of a point against the route: minimum cross-track
/// distance over (a) perpendicular projections that land within a leg and
/// (b) the route vertices. Ties keep the earlier along-track placement.
fn place(legs: &[CumulativeLeg], total: f64, position: LatLon) -> Placement {
    let mut best = Placement {
        along: 0.0,
        cross: f64::INFINITY,
    };
    let mut consider = |candidate: Placement| {
        if candidate.cross < best.cross
            || (candidate.cross == best.cross && candidate.along < best.along)
        {
            best = candidate;
        }
    };
    for leg in legs {
        if leg.length > 0.0 {
            let projection = project_onto_leg(leg.from, leg.to, position);
            if projection.along.0 >= 0.0 && projection.along.0 <= leg.length {
                consider(Placement {
                    along: leg.start_along + projection.along.0,
                    cross: projection.cross.0,
                });
            }
        }
        consider(Placement {
            along: leg.start_along,
            cross: great_circle_distance(position, leg.from).0,
        });
    }
    if let Some(last) = legs.last() {
        consider(Placement {
            along: total,
            cross: great_circle_distance(position, last.to).0,
        });
    }
    best
}

/// Index of the station whose along-track distance is closest to `along`
/// (stations are in ascending along-track order; ties pick the earlier).
fn nearest_station(stations: &[TrackedStation], along: f64) -> usize {
    let upper = stations.partition_point(|s| s.station.along_track.0 < along);
    if upper == 0 {
        return 0;
    }
    if upper == stations.len() {
        return stations.len() - 1;
    }
    let before = along - stations[upper - 1].station.along_track.0;
    let after = stations[upper].station.along_track.0 - along;
    if before <= after { upper - 1 } else { upper }
}

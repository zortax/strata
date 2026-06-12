//! Corridor sampling engine (plan §3 `corridor/`): stations every ~500 m
//! along track, lateral offsets across the corridor half-width; per
//! station the worst-case terrain, tallest obstacle and airspace stack,
//! aggregated into corridor-level airspace crossing intervals.
//!
//! Sampling-based by design — no polygon boolean ops; the resolution
//! parameters are explicit ([`CorridorParams`]) and tested.
//!
//! # Design choices
//!
//! - **Stations** every [`CorridorParams::station_spacing`] from departure
//!   (inclusive), plus a final partial station at the exact destination.
//!   A station on a leg boundary belongs to the following leg.
//! - **Lateral samples** per station: the centerline plus `n =`
//!   [`CorridorParams::lateral_samples_per_side`] offsets each side,
//!   perpendicular to the local track at fractions `i / n` of the
//!   half-width (default `n = 4`: ¼, ½, ¾, 1) — the outermost sample lies
//!   exactly **on** the corridor edge, so the stated width is actually
//!   sampled. Lateral resolution is `half_width / n`; features narrower
//!   than that can slip between samples — the knob is explicit.
//! - **Terrain** per station is the min and max over the lateral samples'
//!   max-pooled cell values (max for ceilings/clearance, min for AGL
//!   floors); `None` only when every sample is outside coverage.
//! - **Obstacles**: one padded-bbox query, then an exact spherical
//!   cross-track check against the legs (and route vertices, covering turn
//!   wedges and the areas beyond the ends); each in-corridor obstacle
//!   attaches to the station nearest its along-track position, tallest top
//!   elevation per station wins.
//! - **Airspaces**: padded-bbox prefilter, exact point-in-polygon over the
//!   lateral samples, consecutive membership merged into entry/exit
//!   intervals at station resolution, with single-station-gap hysteresis
//!   (module docs in `airspaces.rs`): boundary flicker is bridged,
//!   single-station grazes are kept.

mod airspaces;
mod geometry;
mod obstacles;
mod stations;
mod terrain;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use strata_data::domain::{Airspace, BoundingBox, LatLon, Meters, MetersAmsl, Obstacle};
use thiserror::Error;

use crate::flight::RouteWaypoint;
use crate::sources::{AirspaceSource, ElevationSource, ObstacleSource, SourceError};

/// Sampling resolution. Defaults: ±5 NM half-width (the classic VFR MSA
/// corridor; the UI offers 2–5 NM), 500 m station spacing, 4 lateral
/// samples per side of the centerline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CorridorParams {
    /// Lateral half-width of the corridor around the track.
    pub half_width: Meters,
    /// Along-track distance between stations.
    pub station_spacing: Meters,
    /// Lateral sample count on *each side* of the centerline (total
    /// samples per station = `2 × n + 1`).
    pub lateral_samples_per_side: usize,
}

impl Default for CorridorParams {
    fn default() -> Self {
        Self {
            half_width: Meters(5.0 * crate::units::METERS_PER_NAUTICAL_MILE),
            station_spacing: Meters(500.0),
            lateral_samples_per_side: 4,
        }
    }
}

/// One along-track sampling station (on the centerline).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Station {
    /// Index into [`Corridor::samples`].
    pub index: usize,
    /// Route leg this station lies on.
    pub leg_index: usize,
    /// Distance from departure along the track.
    pub along_track: Meters,
    /// Centerline position.
    pub position: LatLon,
}

/// What the corridor saw at one station, worst-case across the lateral
/// samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorridorSample {
    pub station: Station,
    /// Highest max-pooled terrain across the corridor width; `None` only
    /// outside elevation coverage.
    pub max_terrain: Option<MetersAmsl>,
    /// Lowest max-pooled terrain across the corridor width; `None` exactly
    /// when [`Self::max_terrain`] is `None`. This is the conservative
    /// statistic for AGL *floors* (the lowest plausible terrain admits
    /// more penetrations); being min-of-max-pooled values it can still
    /// overestimate the true terrain within one pooling cell.
    pub min_terrain: Option<MetersAmsl>,
    /// Obstacle with the highest top elevation within the corridor slice,
    /// if any.
    pub tallest_obstacle: Option<Obstacle>,
}

/// A corridor-level airspace crossing: the along-track interval over which
/// any lateral sample lies inside the volume's horizontal geometry.
/// Vertical relevance (floor/ceiling vs planned altitude) is judged by the
/// conflict engine, which owns the datum conversions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirspaceCrossing {
    pub airspace: Airspace,
    pub entry_along_track: Meters,
    pub exit_along_track: Meters,
}

/// The sampled corridor for one route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Corridor {
    pub params: CorridorParams,
    /// Stations in along-track order.
    pub samples: Vec<CorridorSample>,
    /// Crossings ordered by entry distance.
    pub crossings: Vec<AirspaceCrossing>,
}

/// Errors from corridor sampling.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CorridorError {
    #[error("route needs at least two waypoints to sample a corridor")]
    RouteTooShort,
    #[error("invalid corridor params: {0}")]
    InvalidParams(&'static str),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Samples the corridor around `route`: stations every
/// [`CorridorParams::station_spacing`] from departure (inclusive) to
/// destination (a final partial station at the exact destination), worst
/// case across `2n + 1` lateral samples; airspace crossings merged per
/// airspace from consecutive stations whose lateral stack contains it.
///
/// Sources are queried with one bounding box covering the whole corridor,
/// padded by the half-width (plus the station spacing, covering
/// great-circle sag between stations, and a conservative degree
/// conversion).
pub fn sample_corridor(
    route: &[RouteWaypoint],
    params: &CorridorParams,
    elevation: &dyn ElevationSource,
    obstacles: &dyn ObstacleSource,
    airspaces: &dyn AirspaceSource,
) -> Result<Corridor, CorridorError> {
    validate(params)?;
    if route.len() < 2 {
        return Err(CorridorError::RouteTooShort);
    }
    let (legs, total) = stations::cumulative_legs(route);
    let tracked = stations::generate(&legs, total, params.station_spacing)?;
    let lateral: Vec<Vec<LatLon>> = tracked
        .iter()
        .map(|station| {
            stations::lateral_samples(station, params.half_width, params.lateral_samples_per_side)
        })
        .collect();
    let query_bbox = query_bbox(&tracked, params);

    let tallest = obstacles::tallest_per_station(
        &legs,
        total,
        &tracked,
        params.half_width,
        query_bbox,
        obstacles,
    )?;
    let crossings =
        airspaces::crossings(&tracked, &lateral, params.half_width, query_bbox, airspaces)?;

    let mut samples = Vec::with_capacity(tracked.len());
    for (station, (samples_at, tallest_obstacle)) in
        tracked.iter().zip(lateral.iter().zip(tallest))
    {
        let terrain = terrain::terrain_extrema(samples_at, elevation)?;
        samples.push(CorridorSample {
            station: station.station,
            max_terrain: terrain.map(|(_, max)| max),
            min_terrain: terrain.map(|(min, _)| min),
            tallest_obstacle,
        });
    }
    Ok(Corridor {
        params: *params,
        samples,
        crossings,
    })
}

fn validate(params: &CorridorParams) -> Result<(), CorridorError> {
    // NaN fails the `is_finite` arm in both checks.
    if params.station_spacing.0 <= 0.0 || !params.station_spacing.0.is_finite() {
        return Err(CorridorError::InvalidParams(
            "station_spacing must be positive and finite",
        ));
    }
    if params.half_width.0 < 0.0 || !params.half_width.0.is_finite() {
        return Err(CorridorError::InvalidParams(
            "half_width must be non-negative and finite",
        ));
    }
    Ok(())
}

/// One bbox covering every lateral sample of every station: the stations'
/// bounding box padded by half-width + spacing (the spacing term covers
/// great-circle sag between stations, which is at most `spacing² / 8R`).
fn query_bbox(tracked: &[stations::TrackedStation], params: &CorridorParams) -> BoundingBox {
    let centers = BoundingBox::from_points(tracked.iter().map(|t| t.station.position))
        .expect("a sampled route has at least one station");
    geometry::pad_bbox(
        centers,
        Meters(params.half_width.0 + params.station_spacing.0),
    )
}

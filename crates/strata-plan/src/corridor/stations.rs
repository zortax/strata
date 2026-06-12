//! Along-track station generation and lateral sample positions.

use strata_data::domain::{LatLon, Meters};

use crate::flight::RouteWaypoint;
use crate::route::{great_circle_distance, initial_true_track, intermediate_point};
use crate::units::DegreesTrue;

use super::geometry::destination_point;
use super::{CorridorError, Station};

/// Slop below which a final partial station would coincide with the last
/// regular station: 1 mm — far above f64 rounding at route scales (≤ µm),
/// far below planning relevance.
const FINAL_STATION_EPSILON_METERS: f64 = 1e-3;

/// Hard cap on station count so a pathological `station_spacing` cannot
/// allocate unbounded memory (the default 500 m spacing yields ~1500
/// stations on the longest plausible German route).
const MAX_STATIONS: usize = 1_000_000;

/// One route leg with its cumulative along-track start.
pub(super) struct CumulativeLeg {
    /// Along-track distance of the leg start from departure.
    pub start_along: f64,
    /// Great-circle length of the leg, meters.
    pub length: f64,
    pub from: LatLon,
    pub to: LatLon,
}

impl CumulativeLeg {
    fn end_along(&self) -> f64 {
        self.start_along + self.length
    }
}

/// A station plus the local true track (needed for lateral offsets and not
/// part of the frozen output types).
pub(super) struct TrackedStation {
    pub station: Station,
    pub track: DegreesTrue,
}

/// Legs with cumulative along-track starts, plus the total route length.
/// Requires `route.len() >= 2` (checked by the caller).
pub(super) fn cumulative_legs(route: &[RouteWaypoint]) -> (Vec<CumulativeLeg>, f64) {
    let mut legs = Vec::with_capacity(route.len().saturating_sub(1));
    let mut total = 0.0;
    for pair in route.windows(2) {
        let from = pair[0].position();
        let to = pair[1].position();
        let length = great_circle_distance(from, to).0;
        legs.push(CumulativeLeg {
            start_along: total,
            length,
            from,
            to,
        });
        total += length;
    }
    (legs, total)
}

/// Stations every `spacing` along the track from departure (inclusive),
/// plus a final partial station at the exact destination unless the last
/// regular station already coincides with it. A fully coincident route
/// (total length 0) yields the single departure station.
///
/// A station exactly on a leg boundary belongs to the *following* leg
/// (half-open `[start, end)` intervals); the destination station belongs to
/// the final leg.
pub(super) fn generate(
    legs: &[CumulativeLeg],
    total: f64,
    spacing: Meters,
) -> Result<Vec<TrackedStation>, CorridorError> {
    let regular = total / spacing.0;
    if !regular.is_finite() || regular >= MAX_STATIONS as f64 {
        return Err(CorridorError::InvalidParams(
            "station_spacing is too fine for the route length",
        ));
    }
    let regular = regular.floor() as usize;
    let mut stations = Vec::with_capacity(regular + 2);
    let mut cursor = 0usize;
    for k in 0..=regular {
        let along = (k as f64 * spacing.0).min(total);
        cursor = advance(legs, cursor, along);
        stations.push(station_at(stations.len(), cursor, along, &legs[cursor]));
    }
    let last_along = stations
        .last()
        .map(|s| s.station.along_track.0)
        .unwrap_or(0.0);
    if total - last_along > FINAL_STATION_EPSILON_METERS {
        let cursor = legs.len() - 1;
        stations.push(station_at(stations.len(), cursor, total, &legs[cursor]));
    }
    Ok(stations)
}

/// First leg whose half-open interval `[start, end)` contains `along`
/// (zero-length legs never own stations); the final leg catches
/// `along == total`.
fn advance(legs: &[CumulativeLeg], mut cursor: usize, along: f64) -> usize {
    while cursor + 1 < legs.len() && along >= legs[cursor].end_along() {
        cursor += 1;
    }
    cursor
}

fn station_at(index: usize, leg_index: usize, along: f64, leg: &CumulativeLeg) -> TrackedStation {
    let position = if leg.length > 0.0 {
        let fraction = ((along - leg.start_along) / leg.length).clamp(0.0, 1.0);
        intermediate_point(leg.from, leg.to, fraction)
    } else {
        leg.from
    };
    TrackedStation {
        station: Station {
            index,
            leg_index,
            along_track: Meters(along),
            position,
        },
        track: local_track(position, leg),
    }
}

/// Local true track at a station: the great-circle course towards the leg
/// end, or — for the destination station, where that is degenerate — the
/// arrival course (reciprocal of the back bearing). 0° by convention on a
/// zero-length leg.
fn local_track(position: LatLon, leg: &CumulativeLeg) -> DegreesTrue {
    if leg.length == 0.0 {
        return DegreesTrue::new(0.0);
    }
    if great_circle_distance(position, leg.to).0 > 1e-6 {
        initial_true_track(position, leg.to)
    } else {
        DegreesTrue::new(initial_true_track(leg.to, leg.from).0 + 180.0)
    }
}

/// Lateral sample positions for one station: the centerline point plus
/// `per_side` offsets on each side of the track, at fractions `i / n` of
/// the half-width (`i = 1..=n`) — for the default `n = 4`: ¼, ½, ¾ and the
/// **full** half-width. The outermost sample sits exactly on the corridor
/// edge, so the stated corridor width is actually sampled (an obstacle of
/// terrain ridge at the edge cannot fall just outside the sampled set).
pub(super) fn lateral_samples(
    station: &TrackedStation,
    half_width: Meters,
    per_side: usize,
) -> Vec<LatLon> {
    let center = station.station.position;
    let mut samples = Vec::with_capacity(2 * per_side + 1);
    samples.push(center);
    for i in 1..=per_side {
        let offset = Meters(half_width.0 * i as f64 / per_side as f64);
        let right = DegreesTrue::new(station.track.0 + 90.0);
        let left = DegreesTrue::new(station.track.0 - 90.0);
        samples.push(destination_point(center, right, offset));
        samples.push(destination_point(center, left, offset));
    }
    samples
}

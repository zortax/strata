//! Route editing operations. All of them preserve the invariant that
//! leg-scoped fields on a waypoint describe the leg *from* that waypoint,
//! and that the final waypoint carries no leg fields.

use thiserror::Error;

use crate::flight::{RoutePoint, RouteWaypoint};

/// Errors from route editing operations.
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum RouteError {
    #[error("waypoint index {index} out of range for route of length {len}")]
    IndexOutOfRange { index: usize, len: usize },
}

/// Reverses the route in place. Each geometric leg keeps its plan
/// (altitude/wind override): the leg fields move with the segment, not
/// with the waypoint they happened to be stored on.
pub fn reverse(route: &mut [RouteWaypoint]) {
    route.reverse();
    let len = route.len();
    if len == 0 {
        return;
    }
    // After reversal, the plan of new leg k (old leg n-2-k) sits on the
    // waypoint at index k+1 — shift every leg field up by one.
    for index in 0..len - 1 {
        let altitude = route[index + 1].leg_altitude.take();
        route[index].leg_altitude = altitude;
        let wind = route[index + 1].leg_wind.take();
        route[index].leg_wind = wind;
    }
    // The new final waypoint has no outgoing leg.
    route[len - 1].leg_altitude = None;
    route[len - 1].leg_wind = None;
}

/// Inserts `point` at `index` (0 = before the departure, `len` = append).
///
/// When the insertion splits an existing leg, the new waypoint inherits
/// that leg's plan so both halves keep the planned altitude/wind; at the
/// ends the new leg starts unplanned.
pub fn insert(
    route: &mut Vec<RouteWaypoint>,
    index: usize,
    point: RoutePoint,
) -> Result<(), RouteError> {
    let len = route.len();
    if index > len {
        return Err(RouteError::IndexOutOfRange { index, len });
    }
    let mut waypoint = RouteWaypoint::new(point);
    let splits_leg = index > 0 && index < len;
    if splits_leg {
        waypoint.leg_altitude = route[index - 1].leg_altitude;
        waypoint.leg_wind = route[index - 1].leg_wind;
    }
    route.insert(index, waypoint);
    Ok(())
}

/// Normalizes the route in place; returns whether anything changed.
///
/// - Removes waypoints whose position coincides with their predecessor's
///   (zero-length legs). The predecessor's leg plan (and nav-log notes)
///   wins; unset/empty fields inherit from the removed waypoint.
/// - Clears leg fields on the final waypoint (no outgoing leg). Notes are
///   *not* leg-scoped (they annotate the waypoint's nav-log row) and stay.
pub fn normalize(route: &mut Vec<RouteWaypoint>) -> bool {
    let mut changed = false;
    let mut index = 1;
    while index < route.len() {
        if route[index].position() == route[index - 1].position() {
            let removed = route.remove(index);
            let keeper = &mut route[index - 1];
            if keeper.leg_altitude.is_none() {
                keeper.leg_altitude = removed.leg_altitude;
            }
            if keeper.leg_wind.is_none() {
                keeper.leg_wind = removed.leg_wind;
            }
            if keeper.notes.is_empty() {
                keeper.notes = removed.notes;
            }
            changed = true;
        } else {
            index += 1;
        }
    }
    if let Some(last) = route.last_mut()
        && (last.leg_altitude.is_some() || last.leg_wind.is_some())
    {
        last.leg_altitude = None;
        last.leg_wind = None;
        changed = true;
    }
    changed
}

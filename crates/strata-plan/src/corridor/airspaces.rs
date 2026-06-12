//! Airspace crossing intervals from per-station membership.
//!
//! Membership: a station is inside an airspace when **any** of its lateral
//! samples lies inside the volume's horizontal polygon (exact even-odd
//! point-in-polygon from `strata-data`, after a cheap padded-bbox
//! prefilter). Consecutive inside stations form along-track intervals;
//! entry/exit are reported at the first/last inside station, i.e. with one
//! station-spacing of resolution — sampling-based by design.
//!
//! **Hysteresis:** point sampling along an airspace boundary can flicker
//! in/out between adjacent stations, shattering one real crossing into
//! micro-intervals. Gaps of at most [`HYSTERESIS_MAX_GAP_STATIONS`] outside
//! station(s) between two inside runs are therefore bridged (conservative:
//! over-reports the crossing extent by ≤ one spacing). Single-station
//! runs are *kept* — a genuine graze of a small volume must never be
//! dropped, even at one station of evidence (entry == exit then).

use strata_data::domain::{BoundingBox, LatLon, Meters, Polygon, PreparedPolygon};

use crate::sources::{AirspaceSource, SourceError};

use super::geometry::pad_bbox;
use super::stations::TrackedStation;
use super::AirspaceCrossing;

/// Maximum number of consecutive outside stations bridged between two
/// inside runs of the same airspace.
const HYSTERESIS_MAX_GAP_STATIONS: usize = 1;

/// Total vertex count above which an airspace polygon is preprocessed
/// into a latitude-banded [`PreparedPolygon`] before the station scan.
/// The scan is O(airspaces × stations) and a handful of high-vertex
/// polygons dominate its cost on long routes; the median candidate has
/// ~18 vertices, where the plain linear test beats the bucketing
/// overhead.
const PREPARE_VERTEX_THRESHOLD: usize = 64;

/// The containment test for one airspace's geometry: plain linear scan
/// for small rings, latitude-banded for big ones (identical semantics).
enum PolygonTest<'a> {
    Linear(&'a Polygon),
    Prepared(PreparedPolygon<'a>),
}

impl PolygonTest<'_> {
    fn new(polygon: &Polygon) -> PolygonTest<'_> {
        if polygon.vertex_count() > PREPARE_VERTEX_THRESHOLD {
            PolygonTest::Prepared(polygon.prepared())
        } else {
            PolygonTest::Linear(polygon)
        }
    }

    fn contains(&self, p: LatLon) -> bool {
        match self {
            Self::Linear(polygon) => polygon.contains(p),
            Self::Prepared(prepared) => prepared.contains(p),
        }
    }
}

/// Crossing intervals for every airspace whose horizontal geometry the
/// corridor samples, ordered by entry distance (then exit, then name, for
/// determinism).
pub(super) fn crossings(
    stations: &[TrackedStation],
    lateral: &[Vec<LatLon>],
    half_width: Meters,
    query_bbox: BoundingBox,
    source: &dyn AirspaceSource,
) -> Result<Vec<AirspaceCrossing>, SourceError> {
    let mut crossings = Vec::new();
    for airspace in source.airspaces_in_bbox(query_bbox)? {
        // A lateral sample is at most half_width from the station center,
        // so a station whose center is outside the polygon bbox padded by
        // half_width cannot have any sample inside the polygon.
        let reach = pad_bbox(airspace.geometry.bounding_box(), half_width);
        let geometry = PolygonTest::new(&airspace.geometry);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (i, station) in stations.iter().enumerate() {
            let inside = reach.contains(station.station.position)
                && lateral[i].iter().any(|&p| geometry.contains(p));
            if inside {
                match runs.last_mut() {
                    Some(run) if run.1 + 1 == i => run.1 = i,
                    _ => runs.push((i, i)),
                }
            }
        }
        for (first, last) in merge_runs(runs) {
            crossings.push(AirspaceCrossing {
                airspace: airspace.clone(),
                entry_along_track: stations[first].station.along_track,
                exit_along_track: stations[last].station.along_track,
            });
        }
    }
    crossings.sort_by(|a, b| {
        a.entry_along_track
            .0
            .total_cmp(&b.entry_along_track.0)
            .then(a.exit_along_track.0.total_cmp(&b.exit_along_track.0))
            .then_with(|| a.airspace.name.cmp(&b.airspace.name))
    });
    Ok(crossings)
}

/// Bridges gaps of at most [`HYSTERESIS_MAX_GAP_STATIONS`] stations between
/// consecutive runs (runs are disjoint and ordered, so `first > last_prev`).
fn merge_runs(runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(runs.len());
    for (first, last) in runs {
        match merged.last_mut() {
            Some(previous) if first - previous.1 - 1 <= HYSTERESIS_MAX_GAP_STATIONS => {
                previous.1 = last;
            }
            _ => merged.push((first, last)),
        }
    }
    merged
}

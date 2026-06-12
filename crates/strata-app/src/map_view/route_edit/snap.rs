//! Snap targets for waypoint drags: airports, navaids and reporting
//! points near the viewport, snapped to within a screen-px radius.
//!
//! Candidates are bulk-queried once per drag (blocking store reads on a
//! worker; the camera cannot move during a route drag, so the viewport
//! envelope stays valid) and the per-move selection is pure screen-space
//! math. Reporting points only become snappable at the zoom that also
//! feeds them to the renderer — snapping to invisible targets would look
//! like a glitch.

use strata_data::domain::{BoundingBox, LatLon as GeoLatLon};
use strata_data::store::Store;
use strata_plan::flight::{NamedPoint, NamedPointKind, RoutePoint};
use strata_render::glam::DVec2;

use super::super::{REPORTING_POINT_FEED_MIN_ZOOM, fetch};
use super::hit::Projection;

/// A named feature a dragged waypoint can snap onto. Carries everything a
/// [`NamedPoint`] needs so the committed route point stays re-resolvable.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapCandidate {
    pub kind: NamedPointKind,
    /// Stable identifier (ICAO ident / navaid ident / published name).
    pub id: String,
    pub name: String,
    pub position: GeoLatLon,
}

impl SnapCandidate {
    /// The route point a snap commits: a named reference with position
    /// snapshot.
    pub(crate) fn route_point(&self) -> RoutePoint {
        RoutePoint::Named(NamedPoint {
            kind: self.kind,
            id: self.id.clone(),
            name: self.name.clone(),
            position: self.position,
        })
    }
}

/// Whether reporting points join the snap set at this zoom (the same gate
/// as the feature feed — only what is visible snaps).
pub(crate) fn snap_reporting_points(zoom: f64) -> bool {
    zoom >= REPORTING_POINT_FEED_MIN_ZOOM
}

/// Blocking store queries — worker threads only. Airports without an ICAO
/// ident are skipped: a [`NamedPoint`] needs a stable id.
pub(crate) fn query_candidates(store: &Store, bbox: BoundingBox, zoom: f64) -> Vec<SnapCandidate> {
    let mut out = Vec::new();
    for airport in fetch("snap airports", store.airports_in_bbox(bbox)) {
        let Some(ident) = airport.ident else {
            continue;
        };
        out.push(SnapCandidate {
            kind: NamedPointKind::Airport,
            id: ident.as_str().to_owned(),
            name: airport.name,
            position: airport.position,
        });
    }
    for navaid in fetch("snap navaids", store.navaids_in_bbox(bbox)) {
        out.push(SnapCandidate {
            kind: NamedPointKind::Navaid,
            id: navaid.ident,
            name: navaid.name,
            position: navaid.position,
        });
    }
    if snap_reporting_points(zoom) {
        for point in fetch(
            "snap reporting points",
            store.reporting_points_in_bbox(bbox),
        ) {
            out.push(SnapCandidate {
                kind: NamedPointKind::ReportingPoint,
                id: point.name.clone(),
                name: point.name,
                position: point.position,
            });
        }
    }
    out
}

/// The candidate nearest to `cursor` within `radius_px` (screen space), if
/// any.
pub(crate) fn nearest_snap<'a>(
    candidates: &'a [SnapCandidate],
    proj: &Projection,
    cursor: DVec2,
    radius_px: f64,
) -> Option<&'a SnapCandidate> {
    candidates
        .iter()
        .map(|c| {
            let px = proj.px_of([c.position.lon(), c.position.lat()]);
            (c, px.distance(cursor))
        })
        .filter(|(_, distance)| *distance <= radius_px)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(candidate, _)| candidate)
}

#[cfg(test)]
mod tests {
    use strata_render::CameraSnapshot;

    use super::*;

    fn candidate(kind: NamedPointKind, id: &str, lat: f64, lon: f64) -> SnapCandidate {
        SnapCandidate {
            kind,
            id: id.to_owned(),
            name: format!("{id} name"),
            position: GeoLatLon::new(lat, lon).unwrap(),
        }
    }

    fn proj(lat: f64, lon: f64, zoom: f64) -> Projection {
        Projection::new(
            &CameraSnapshot {
                center: strata_render::LatLon::new(lat, lon),
                zoom,
                bounds: (
                    strata_render::LatLon::new(lat - 1.0, lon - 1.0),
                    strata_render::LatLon::new(lat + 1.0, lon + 1.0),
                ),
            },
            DVec2::new(1000.0, 800.0),
        )
    }

    /// Nearest within the radius wins; outside the radius nothing snaps.
    #[test]
    fn nearest_within_radius() {
        let proj = proj(50.0, 10.0, 10.0);
        // ~0.01° lon at zoom 10 ≈ 7.3 px — inside a 12 px radius;
        // 0.05° ≈ 36 px — outside.
        let near = candidate(NamedPointKind::Airport, "EDDF", 50.0, 10.01);
        let nearer = candidate(NamedPointKind::Navaid, "FFM", 50.0, 10.005);
        let far = candidate(NamedPointKind::Airport, "EDDM", 50.0, 10.05);
        let candidates = vec![near, nearer, far];

        let cursor = DVec2::new(500.0, 400.0); // the camera center
        let hit = nearest_snap(&candidates, &proj, cursor, 12.0).unwrap();
        assert_eq!(hit.id, "FFM", "the nearer of the two in-radius targets");

        let only_far = vec![candidate(NamedPointKind::Airport, "EDDM", 50.0, 10.05)];
        assert!(nearest_snap(&only_far, &proj, cursor, 12.0).is_none());
        assert!(nearest_snap(&[], &proj, cursor, 12.0).is_none());
    }

    /// Snap commits a named, re-resolvable route point with the position
    /// snapshot.
    #[test]
    fn snap_commits_a_named_point() {
        let c = candidate(NamedPointKind::ReportingPoint, "ECHO 1", 49.5, 11.0);
        match c.route_point() {
            RoutePoint::Named(named) => {
                assert_eq!(named.kind, NamedPointKind::ReportingPoint);
                assert_eq!(named.id, "ECHO 1");
                assert_eq!(named.name, "ECHO 1 name");
                assert_eq!(named.position, GeoLatLon::new(49.5, 11.0).unwrap());
            }
            RoutePoint::Free(_) => panic!("snap must produce a named point"),
        }
    }

    /// Reporting points share the feature feed's zoom gate.
    #[test]
    fn reporting_points_snap_only_when_visible() {
        assert!(!snap_reporting_points(7.9));
        assert!(snap_reporting_points(REPORTING_POINT_FEED_MIN_ZOOM));
        assert!(snap_reporting_points(12.0));
    }
}

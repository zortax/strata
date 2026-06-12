//! Pure screen-space math for route editing: lat/lon → logical-px
//! projection (mirroring `strata_render::Camera::project`) and the
//! handle/leg hit tests.
//!
//! The renderer does **no** picking — `RenderRoute` echoes app-side vertex
//! ids precisely so the app can hit-test in screen space with its own
//! camera snapshot (plan §4).

use strata_render::features::{RenderRoute, RouteVertex};
use strata_render::glam::DVec2;
use strata_render::{CameraSnapshot, camera::TILE_SIZE_PX, geo};

use super::{HANDLE_HIT_RADIUS_PX, LEG_HIT_RADIUS_PX};

/// World→screen projection frozen from a camera snapshot: the world→screen
/// scale is `256 · 2^zoom` logical px per world unit, the camera center
/// maps to the viewport center, y grows south/down.
pub(crate) struct Projection {
    center_world: DVec2,
    /// Logical px per world unit.
    scale: f64,
    half_viewport: DVec2,
}

impl Projection {
    pub(crate) fn new(snapshot: &CameraSnapshot, viewport_px: DVec2) -> Self {
        Self {
            center_world: geo::world_from_lat_lon(snapshot.center),
            scale: TILE_SIZE_PX * snapshot.zoom.exp2(),
            half_viewport: viewport_px * 0.5,
        }
    }

    /// `[lon, lat]` degrees (the route-vertex convention) → logical screen
    /// px.
    pub(crate) fn px_of(&self, pos: [f64; 2]) -> DVec2 {
        let world = geo::world_from_lat_lon(strata_render::LatLon::new(pos[1], pos[0]));
        (world - self.center_world) * self.scale + self.half_viewport
    }
}

/// What a press on the route hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteHit {
    /// A waypoint handle (vertex id from the [`RenderRoute`]).
    Handle(u64),
    /// A main-track leg line (leg index), outside every handle.
    Leg(usize),
}

/// Hit-test `cursor` against the route: the nearest handle within
/// [`HANDLE_HIT_RADIUS_PX`] wins; otherwise the nearest main-track leg
/// within [`LEG_HIT_RADIUS_PX`]. Alternate handles are draggable, the
/// dashed alternate links are not legs.
pub(crate) fn hit_test(route: &RenderRoute, proj: &Projection, cursor: DVec2) -> Option<RouteHit> {
    if let Some(id) = nearest_handle(route, proj, cursor, HANDLE_HIT_RADIUS_PX) {
        return Some(RouteHit::Handle(id));
    }
    let (leg, distance) = nearest_leg(route, proj, cursor)?;
    (distance <= LEG_HIT_RADIUS_PX).then_some(RouteHit::Leg(leg))
}

/// The id of the handle nearest to `cursor` within `radius_px`, if any.
pub(crate) fn nearest_handle(
    route: &RenderRoute,
    proj: &Projection,
    cursor: DVec2,
    radius_px: f64,
) -> Option<u64> {
    route
        .points
        .iter()
        .map(|v| (v.id, proj.px_of(v.pos).distance(cursor)))
        .filter(|(_, distance)| *distance <= radius_px)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// The main-track leg nearest to `cursor` and its distance in px. Legs
/// connect consecutive non-alternate points; `None` without at least two.
pub(crate) fn nearest_leg(
    route: &RenderRoute,
    proj: &Projection,
    cursor: DVec2,
) -> Option<(usize, f64)> {
    nearest_leg_projection(route, proj, cursor).map(|(leg, distance, _)| (leg, distance))
}

/// [`nearest_leg`] plus the cursor's clamped projection parameter along
/// that leg (0 = leg start, 1 = leg end) — the map-hover scrub's
/// along-track fraction.
pub(crate) fn nearest_leg_projection(
    route: &RenderRoute,
    proj: &Projection,
    cursor: DVec2,
) -> Option<(usize, f64, f64)> {
    let track: Vec<DVec2> = main_track(route).map(|v| proj.px_of(v.pos)).collect();
    track
        .windows(2)
        .enumerate()
        .map(|(leg, segment)| {
            let (distance, fraction) = segment_projection(cursor, segment[0], segment[1]);
            (leg, distance, fraction)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Route index a waypoint pulled out of leg `leg` is inserted at: between
/// the leg's endpoints (`leg` connects waypoints `leg` and `leg + 1`).
pub(crate) fn insert_index(leg: usize) -> usize {
    leg + 1
}

/// The flown track: every non-alternate point in `points` order.
pub(crate) fn main_track(route: &RenderRoute) -> impl Iterator<Item = &RouteVertex> {
    route.points.iter().filter(|v| !v.kind.is_alternate())
}

/// Distance from `p` to the segment `a`–`b` plus the clamped projection
/// parameter `t ∈ [0, 1]` of the closest point (degenerate segments
/// collapse to the point distance at `t = 0`).
fn segment_projection(p: DVec2, a: DVec2, b: DVec2) -> (f64, f64) {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 <= f64::EPSILON {
        return (p.distance(a), 0.0);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p.distance(a + ab * t), t)
}

#[cfg(test)]
mod tests {
    use strata_render::features::RoutePointKind;

    use super::*;

    fn snapshot(lat: f64, lon: f64, zoom: f64) -> CameraSnapshot {
        CameraSnapshot {
            center: strata_render::LatLon::new(lat, lon),
            zoom,
            bounds: (
                strata_render::LatLon::new(lat - 1.0, lon - 1.0),
                strata_render::LatLon::new(lat + 1.0, lon + 1.0),
            ),
        }
    }

    fn vertex(id: u64, pos: [f64; 2], kind: RoutePointKind) -> RouteVertex {
        RouteVertex { id, pos, kind }
    }

    fn route(points: Vec<RouteVertex>) -> RenderRoute {
        RenderRoute {
            points,
            ..RenderRoute::default()
        }
    }

    /// Three-point main track plus one alternate, all on the equator-ish
    /// horizontal so screen positions are easy to reason about.
    fn test_route() -> RenderRoute {
        route(vec![
            vertex(0, [10.0, 50.0], RoutePointKind::Departure),
            vertex(1, [10.2, 50.0], RoutePointKind::Waypoint),
            vertex(2, [10.4, 50.0], RoutePointKind::Destination),
            vertex(1 << 32, [10.4, 49.9], RoutePointKind::Alternate),
        ])
    }

    /// The camera center lands at the viewport center; x grows east by
    /// `Δlon/360 · 256·2^zoom`, y shrinks as latitude grows (y is down).
    #[test]
    fn projection_matches_camera_math() {
        let proj = Projection::new(&snapshot(50.0, 10.0, 8.0), DVec2::new(1000.0, 800.0));
        let center = proj.px_of([10.0, 50.0]);
        assert!((center - DVec2::new(500.0, 400.0)).length() < 1e-9);

        let east = proj.px_of([10.5, 50.0]);
        let want_dx = 0.5 / 360.0 * 256.0 * 8.0f64.exp2();
        assert!((east.x - 500.0 - want_dx).abs() < 1e-9);
        assert!((east.y - 400.0).abs() < 1e-9);

        let north = proj.px_of([10.0, 50.5]);
        assert!(north.y < 400.0, "north of center is up the screen");
    }

    #[test]
    fn handles_hit_within_radius_nearest_first() {
        let route = test_route();
        let proj = Projection::new(&snapshot(50.0, 10.2, 8.0), DVec2::new(1000.0, 800.0));

        // Exactly on waypoint 1 (the camera center).
        let on = proj.px_of([10.2, 50.0]);
        assert_eq!(hit_test(&route, &proj, on), Some(RouteHit::Handle(1)));

        // A few px off still hits.
        let near = on + DVec2::new(6.0, 5.0);
        assert_eq!(hit_test(&route, &proj, near), Some(RouteHit::Handle(1)));

        // Between two handles the nearer one wins.
        let h0 = proj.px_of([10.0, 50.0]);
        let h1 = proj.px_of([10.2, 50.0]);
        let closer_to_0 = h0 + (h1 - h0) * 0.02;
        assert_eq!(
            nearest_handle(&route, &proj, closer_to_0, 10.0),
            Some(0),
            "nearest handle wins"
        );

        // Alternates are hit-testable handles too.
        let alt = proj.px_of([10.4, 49.9]);
        assert_eq!(
            hit_test(&route, &proj, alt),
            Some(RouteHit::Handle(1 << 32))
        );
    }

    #[test]
    fn legs_hit_only_outside_handles_and_within_their_radius() {
        let route = test_route();
        let proj = Projection::new(&snapshot(50.0, 10.2, 8.0), DVec2::new(1000.0, 800.0));

        // Mid leg 0, 4 px off the line: a leg hit.
        let mid = (proj.px_of([10.0, 50.0]) + proj.px_of([10.2, 50.0])) * 0.5;
        let near_line = mid + DVec2::new(0.0, 4.0);
        assert_eq!(hit_test(&route, &proj, near_line), Some(RouteHit::Leg(0)));
        let mid_leg1 = (proj.px_of([10.2, 50.0]) + proj.px_of([10.4, 50.0])) * 0.5;
        assert_eq!(hit_test(&route, &proj, mid_leg1), Some(RouteHit::Leg(1)));

        // 20 px off the line: nothing.
        assert_eq!(hit_test(&route, &proj, mid + DVec2::new(0.0, 20.0)), None);

        // On a handle the handle takes precedence over both adjoining legs.
        let on_handle = proj.px_of([10.2, 50.0]) + DVec2::new(3.0, 0.0);
        assert_eq!(
            hit_test(&route, &proj, on_handle),
            Some(RouteHit::Handle(1))
        );
    }

    /// The dashed destination→alternate link is not a leg: a press on it
    /// (well away from the main track) hits nothing.
    #[test]
    fn alternate_links_are_not_legs() {
        let route = test_route();
        let proj = Projection::new(&snapshot(50.0, 10.4, 9.0), DVec2::new(1000.0, 800.0));
        let mid_link = (proj.px_of([10.4, 50.0]) + proj.px_of([10.4, 49.9])) * 0.5;
        assert_eq!(hit_test(&route, &proj, mid_link), None);
    }

    #[test]
    fn single_point_routes_have_no_legs() {
        let route = route(vec![vertex(0, [10.0, 50.0], RoutePointKind::Departure)]);
        let proj = Projection::new(&snapshot(50.0, 10.0, 8.0), DVec2::new(1000.0, 800.0));
        assert_eq!(nearest_leg(&route, &proj, DVec2::new(500.0, 400.0)), None);
    }

    /// The new waypoint lands *between* the leg's endpoints.
    #[test]
    fn insert_index_splits_the_leg() {
        assert_eq!(insert_index(0), 1);
        assert_eq!(insert_index(2), 3);
    }

    #[test]
    fn segment_projection_clamps_to_endpoints() {
        let a = DVec2::new(0.0, 0.0);
        let b = DVec2::new(10.0, 0.0);
        // Perpendicular foot inside the segment.
        let (distance, t) = segment_projection(DVec2::new(5.0, 3.0), a, b);
        assert!((distance - 3.0).abs() < 1e-12);
        assert!((t - 0.5).abs() < 1e-12);
        // Beyond an endpoint the endpoint distance counts, t clamps.
        let (distance, t) = segment_projection(DVec2::new(14.0, 3.0), a, b);
        assert!((distance - 5.0).abs() < 1e-12);
        assert!((t - 1.0).abs() < 1e-12);
        // Degenerate segment.
        let (distance, t) = segment_projection(DVec2::new(3.0, 4.0), a, a);
        assert!((distance - 5.0).abs() < 1e-12);
        assert_eq!(t, 0.0);
    }

    /// The map-hover scrub's along-leg fraction: the cursor's projection
    /// parameter on the nearest leg.
    #[test]
    fn leg_projection_reports_the_along_fraction() {
        let route = test_route();
        let proj = Projection::new(&snapshot(50.0, 10.2, 8.0), DVec2::new(1000.0, 800.0));
        let a = proj.px_of([10.0, 50.0]);
        let b = proj.px_of([10.2, 50.0]);
        let quarter = a + (b - a) * 0.25 + DVec2::new(0.0, 3.0);
        let (leg, distance, t) = nearest_leg_projection(&route, &proj, quarter).unwrap();
        assert_eq!(leg, 0);
        assert!((distance - 3.0).abs() < 1e-9);
        assert!((t - 0.25).abs() < 1e-9);
    }
}

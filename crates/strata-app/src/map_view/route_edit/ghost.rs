//! Ghost-route preview during drags (pure): the doc route with the dragged
//! handle moved, or the rubber-banded waypoint spliced into its leg. The
//! committed edit goes through `AppState` on release; the ghost only feeds
//! the renderer.

use strata_render::features::{RenderRoute, RoutePointKind, RouteVertex};

/// Vertex id of the rubber-band ghost point. Never collides with app ids
/// (route indices and `ALTERNATE_ID_BASE + i` stay far below) and is never
/// committed — the insert goes through the document.
pub(crate) const GHOST_VERTEX_ID: u64 = u64::MAX;

/// The route with vertex `id` moved to `pos` (`[lon, lat]`). An unknown id
/// (stale drag against a shrunken doc) leaves the route unchanged.
pub(crate) fn move_vertex(base: &RenderRoute, id: u64, pos: [f64; 2]) -> RenderRoute {
    let mut route = base.clone();
    if let Some(vertex) = route.points.iter_mut().find(|v| v.id == id) {
        vertex.pos = pos;
    }
    route
}

/// The route with a ghost waypoint spliced into main-track leg `leg` at
/// `pos`. The split leg's conflict flag is duplicated onto both halves so
/// the tint never flickers during the preview, while its **label** is
/// dropped from both halves — the leg's MH/GS/altitude describe the old,
/// unsplit geometry and would be wrong on either half (the landing
/// recompute labels the real legs). An out-of-range leg (stale drag)
/// leaves the route unchanged.
pub(crate) fn insert_into_leg(base: &RenderRoute, leg: usize, pos: [f64; 2]) -> RenderRoute {
    let mut route = base.clone();
    // Insert before the main-track vertex that ends the leg (alternates
    // trail the track in `points`, so this also keeps them last).
    let Some(at) = points_index_of_main(base, leg + 1) else {
        return route;
    };
    route.points.insert(
        at,
        RouteVertex {
            id: GHOST_VERTEX_ID,
            pos,
            kind: RoutePointKind::Waypoint,
        },
    );
    if leg < route.leg_conflict.len() {
        let flag = route.leg_conflict[leg];
        route.leg_conflict.insert(leg, flag);
    }
    if leg < route.leg_labels.len() {
        route.leg_labels[leg] = None;
        route.leg_labels.insert(leg, None);
    }
    route
}

/// Index in `points` of the `nth` main-track (non-alternate) vertex.
fn points_index_of_main(route: &RenderRoute, nth: usize) -> Option<usize> {
    route
        .points
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.kind.is_alternate())
        .nth(nth)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RenderRoute {
        RenderRoute {
            points: vec![
                RouteVertex {
                    id: 0,
                    pos: [10.0, 50.0],
                    kind: RoutePointKind::Departure,
                },
                RouteVertex {
                    id: 1,
                    pos: [10.2, 50.1],
                    kind: RoutePointKind::Waypoint,
                },
                RouteVertex {
                    id: 2,
                    pos: [10.4, 50.2],
                    kind: RoutePointKind::Destination,
                },
                RouteVertex {
                    id: 1 << 32,
                    pos: [10.5, 50.0],
                    kind: RoutePointKind::Alternate,
                },
            ],
            leg_conflict: vec![false, true],
            leg_labels: vec![
                Some("MH 053 · 135 kt · 4500".to_owned()),
                Some("MH 110 · 120 kt · 4500".to_owned()),
            ],
            ..RenderRoute::default()
        }
    }

    #[test]
    fn move_vertex_moves_exactly_the_matching_id() {
        let moved = move_vertex(&route(), 1, [11.0, 51.0]);
        assert_eq!(moved.points[1].pos, [11.0, 51.0]);
        assert_eq!(moved.points[0].pos, [10.0, 50.0]);
        assert_eq!(moved.points[2].pos, [10.4, 50.2]);
        assert_eq!(moved.leg_conflict, vec![false, true]);

        // Alternates move by their offset id.
        let alt = move_vertex(&route(), 1 << 32, [12.0, 49.0]);
        assert_eq!(alt.points[3].pos, [12.0, 49.0]);

        // Unknown id (stale drag): unchanged.
        assert_eq!(move_vertex(&route(), 99, [0.0, 0.0]), route());
    }

    #[test]
    fn insert_into_leg_splices_before_the_legs_end_vertex() {
        let ghost = insert_into_leg(&route(), 0, [10.1, 50.05]);
        assert_eq!(ghost.points.len(), 5);
        assert_eq!(ghost.points[1].id, GHOST_VERTEX_ID);
        assert_eq!(ghost.points[1].pos, [10.1, 50.05]);
        assert_eq!(ghost.points[1].kind, RoutePointKind::Waypoint);
        // The alternate stays last.
        assert_eq!(ghost.points[4].kind, RoutePointKind::Alternate);
        // Leg 0's flag (false) is duplicated; leg 1's tint is undisturbed.
        assert_eq!(ghost.leg_conflict, vec![false, false, true]);
        // The split leg's label is wrong for both halves — dropped; the
        // untouched leg keeps its numbers (now at index 2).
        assert_eq!(
            ghost.leg_labels,
            vec![None, None, Some("MH 110 · 120 kt · 4500".to_owned())]
        );

        // Splitting the tinted leg keeps both halves tinted.
        let tinted = insert_into_leg(&route(), 1, [10.3, 50.15]);
        assert_eq!(tinted.points[2].id, GHOST_VERTEX_ID);
        assert_eq!(tinted.leg_conflict, vec![false, true, true]);
        assert_eq!(
            tinted.leg_labels,
            vec![Some("MH 053 · 135 kt · 4500".to_owned()), None, None]
        );

        // Moving a handle keeps the (stale-but-benign) labels — same
        // contract as the conflict tints; the landing recompute refreshes.
        let moved = move_vertex(&route(), 1, [11.0, 51.0]);
        assert_eq!(moved.leg_labels, route().leg_labels);
    }

    #[test]
    fn insert_handles_missing_conflicts_and_stale_legs() {
        let mut base = route();
        base.leg_conflict = Vec::new();
        base.leg_labels = Vec::new();
        let ghost = insert_into_leg(&base, 0, [10.1, 50.05]);
        assert_eq!(ghost.points.len(), 5);
        assert!(ghost.leg_conflict.is_empty(), "short vec stays short");
        assert!(ghost.leg_labels.is_empty(), "short vec stays short");

        // Out-of-range leg: unchanged (a stale drag must not panic).
        assert_eq!(insert_into_leg(&route(), 9, [0.0, 0.0]), route());
    }
}

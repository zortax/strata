use strata_data::domain::{LatLon, MetersAmsl};

use super::*;
use crate::flight::{FreePoint, PlannedAltitude, RoutePoint, RouteWaypoint};

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

fn wp(lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: None,
        position: ll(lat, lon),
    }))
}

fn wp_alt(lat: f64, lon: f64, feet: f64) -> RouteWaypoint {
    let mut waypoint = wp(lat, lon);
    waypoint.leg_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(feet)));
    waypoint
}

fn alt_feet(waypoint: &RouteWaypoint) -> Option<f64> {
    match waypoint.leg_altitude {
        Some(PlannedAltitude::Amsl(m)) => Some(m.as_feet().round()),
        other => panic!("expected AMSL altitude, got {other:?}"),
    }
}

// Reference positions (all Germany — no antimeridian handling anywhere).
const EDDF: (f64, f64) = (50.0379, 8.5622);
const EDDM: (f64, f64) = (48.3538, 11.7861);
const EDDH: (f64, f64) = (53.6304, 9.9882);

// Expected values computed independently (Python haversine/SLERP with the
// same IUGG R1 radius).

#[test]
fn distance_along_a_meridian() {
    // One degree of latitude on the R1 sphere.
    let d = great_circle_distance(ll(50.0, 10.0), ll(51.0, 10.0));
    assert!((d.0 - 111_195.080).abs() < 0.01, "got {}", d.0);
}

#[test]
fn distance_along_a_parallel() {
    let d = great_circle_distance(ll(50.0, 10.0), ll(50.0, 11.0));
    assert!((d.0 - 71_474.287).abs() < 0.01, "got {}", d.0);
}

#[test]
fn distance_frankfurt_munich() {
    let d = great_circle_distance(ll(EDDF.0, EDDF.1), ll(EDDM.0, EDDM.1));
    assert!((d.0 - 299_861.344).abs() < 0.01, "got {}", d.0);
}

#[test]
fn distance_hamburg_frankfurt() {
    let d = great_circle_distance(ll(EDDH.0, EDDH.1), ll(EDDF.0, EDDF.1));
    assert!((d.0 - 411_286.890).abs() < 0.01, "got {}", d.0);
}

#[test]
fn distance_is_symmetric_and_zero_on_self() {
    let a = ll(EDDF.0, EDDF.1);
    let b = ll(EDDM.0, EDDM.1);
    assert_eq!(great_circle_distance(a, b), great_circle_distance(b, a));
    assert_eq!(great_circle_distance(a, a).0, 0.0);
}

#[test]
fn track_along_a_meridian() {
    assert_eq!(initial_true_track(ll(50.0, 10.0), ll(51.0, 10.0)).0, 0.0);
    assert_eq!(initial_true_track(ll(51.0, 10.0), ll(50.0, 10.0)).0, 180.0);
}

#[test]
fn track_along_a_parallel_shows_convergence() {
    // Eastbound at 50°N starts slightly north of 090° (great circle bows
    // poleward of the parallel).
    let t = initial_true_track(ll(50.0, 10.0), ll(50.0, 11.0));
    assert!((t.0 - 89.616_974).abs() < 1e-6, "got {}", t.0);
    let back = initial_true_track(ll(50.0, 11.0), ll(50.0, 10.0));
    assert!((back.0 - 270.383_026).abs() < 1e-6, "got {}", back.0);
}

#[test]
fn track_frankfurt_munich() {
    let t = initial_true_track(ll(EDDF.0, EDDF.1), ll(EDDM.0, EDDM.1));
    assert!((t.0 - 127.409_573).abs() < 1e-6, "got {}", t.0);
    let back = initial_true_track(ll(EDDM.0, EDDM.1), ll(EDDF.0, EDDF.1));
    assert!((back.0 - 309.850_435).abs() < 1e-6, "got {}", back.0);
}

#[test]
fn track_hamburg_frankfurt() {
    let t = initial_true_track(ll(EDDH.0, EDDH.1), ll(EDDF.0, EDDF.1));
    assert!((t.0 - 194.345_364).abs() < 1e-6, "got {}", t.0);
}

#[test]
fn coincident_points_track_is_zero_by_convention() {
    assert_eq!(initial_true_track(ll(50.0, 10.0), ll(50.0, 10.0)).0, 0.0);
}

#[test]
fn midpoint_frankfurt_munich() {
    let m = midpoint(ll(EDDF.0, EDDF.1), ll(EDDM.0, EDDM.1));
    assert!((m.lat() - 49.207_064).abs() < 1e-6, "got {}", m.lat());
    assert!((m.lon() - 10.201_600).abs() < 1e-6, "got {}", m.lon());
}

#[test]
fn intermediate_point_on_meridian() {
    let p = intermediate_point(ll(50.0, 10.0), ll(51.0, 10.0), 0.25);
    assert!((p.lat() - 50.25).abs() < 1e-9);
    assert!((p.lon() - 10.0).abs() < 1e-9);
}

#[test]
fn intermediate_point_quarter_frankfurt_munich() {
    let p = intermediate_point(ll(EDDF.0, EDDF.1), ll(EDDM.0, EDDM.1), 0.25);
    assert!((p.lat() - 49.625_376).abs() < 1e-6, "got {}", p.lat());
    assert!((p.lon() - 9.388_890).abs() < 1e-6, "got {}", p.lon());
}

#[test]
fn intermediate_point_clamps_fraction_and_handles_coincidence() {
    let a = ll(50.0, 10.0);
    let b = ll(51.0, 10.0);
    assert_eq!(intermediate_point(a, b, -0.5), a);
    assert_eq!(intermediate_point(a, b, 1.5), b);
    assert_eq!(intermediate_point(a, a, 0.5), a);
}

#[test]
fn leg_iteration() {
    let route = vec![wp(50.0, 10.0), wp(51.0, 10.0), wp(51.0, 11.0)];
    let collected: Vec<_> = legs(&route).collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].index, 0);
    assert_eq!(collected[1].index, 1);
    assert_eq!(collected[0].geometry().initial_true_track.0, 0.0);

    let total = total_distance(&route);
    let sum = great_circle_distance(ll(50.0, 10.0), ll(51.0, 10.0)).0
        + great_circle_distance(ll(51.0, 10.0), ll(51.0, 11.0)).0;
    assert!((total.0 - sum).abs() < 1e-9);

    assert_eq!(legs(&[]).count(), 0);
    assert_eq!(legs(&route[..1]).count(), 0);
}

#[test]
fn reverse_moves_leg_plans_with_their_segments() {
    // A --1000ft--> B --2000ft--> C
    let mut route = vec![
        wp_alt(50.0, 10.0, 1000.0),
        wp_alt(51.0, 10.0, 2000.0),
        wp(52.0, 10.0),
    ];
    reverse(&mut route);
    // C --2000ft--> B --1000ft--> A
    assert_eq!(route[0].position(), ll(52.0, 10.0));
    assert_eq!(alt_feet(&route[0]), Some(2000.0));
    assert_eq!(route[1].position(), ll(51.0, 10.0));
    assert_eq!(alt_feet(&route[1]), Some(1000.0));
    assert_eq!(route[2].position(), ll(50.0, 10.0));
    assert_eq!(route[2].leg_altitude, None);
}

#[test]
fn reverse_twice_is_identity() {
    let original = vec![
        wp_alt(50.0, 10.0, 1000.0),
        wp_alt(51.0, 10.0, 2000.0),
        wp(52.0, 10.0),
    ];
    let mut route = original.clone();
    reverse(&mut route);
    reverse(&mut route);
    assert_eq!(route, original);
}

#[test]
fn reverse_handles_trivial_routes() {
    let mut empty: Vec<RouteWaypoint> = vec![];
    reverse(&mut empty);
    assert!(empty.is_empty());

    let mut single = vec![wp_alt(50.0, 10.0, 1000.0)];
    reverse(&mut single);
    // A lone waypoint has no outgoing leg.
    assert_eq!(single[0].leg_altitude, None);
}

#[test]
fn insert_into_a_leg_inherits_its_plan() {
    let mut route = vec![wp_alt(50.0, 10.0, 3000.0), wp(52.0, 10.0)];
    insert(
        &mut route,
        1,
        RoutePoint::Free(FreePoint {
            name: None,
            position: ll(51.0, 10.0),
        }),
    )
    .unwrap();
    assert_eq!(route.len(), 3);
    // Both halves of the split leg keep the 3000 ft plan.
    assert_eq!(alt_feet(&route[0]), Some(3000.0));
    assert_eq!(alt_feet(&route[1]), Some(3000.0));
}

#[test]
fn insert_at_ends_starts_unplanned() {
    let mut route = vec![wp_alt(50.0, 10.0, 3000.0), wp(52.0, 10.0)];
    insert(
        &mut route,
        0,
        RoutePoint::Free(FreePoint {
            name: None,
            position: ll(49.0, 10.0),
        }),
    )
    .unwrap();
    assert_eq!(route[0].leg_altitude, None);
    let len = route.len();
    insert(
        &mut route,
        len,
        RoutePoint::Free(FreePoint {
            name: None,
            position: ll(53.0, 10.0),
        }),
    )
    .unwrap();
    assert_eq!(route.last().unwrap().leg_altitude, None);
}

#[test]
fn insert_out_of_range_errors() {
    let mut route = vec![wp(50.0, 10.0)];
    let result = insert(
        &mut route,
        5,
        RoutePoint::Free(FreePoint {
            name: None,
            position: ll(51.0, 10.0),
        }),
    );
    assert_eq!(
        result,
        Err(RouteError::IndexOutOfRange { index: 5, len: 1 })
    );
}

#[test]
fn normalize_drops_zero_length_legs_and_inherits_plans() {
    // B duplicated; the first B has no plan, the duplicate carries one.
    let mut route = vec![
        wp_alt(50.0, 10.0, 1000.0),
        wp(51.0, 10.0),
        wp_alt(51.0, 10.0, 2000.0),
        wp(52.0, 10.0),
    ];
    assert!(normalize(&mut route));
    assert_eq!(route.len(), 3);
    assert_eq!(alt_feet(&route[1]), Some(2000.0));
}

#[test]
fn normalize_keeps_predecessor_plan_when_set() {
    let mut route = vec![
        wp(50.0, 10.0),
        wp_alt(51.0, 10.0, 4000.0),
        wp_alt(51.0, 10.0, 2000.0),
        wp(52.0, 10.0),
    ];
    assert!(normalize(&mut route));
    assert_eq!(alt_feet(&route[1]), Some(4000.0));
}

#[test]
fn normalize_clears_final_leg_fields_and_is_idempotent() {
    let mut route = vec![wp(50.0, 10.0), wp_alt(51.0, 10.0, 2000.0)];
    assert!(normalize(&mut route));
    assert_eq!(route[1].leg_altitude, None);
    assert!(!normalize(&mut route));
}

/// Notes annotate the waypoint's nav-log row, not the outgoing leg: the
/// dedup merge inherits them like the leg plan, but the final waypoint
/// keeps its notes (the destination row is a real PLOG row).
#[test]
fn normalize_merges_notes_and_keeps_them_on_the_final_waypoint() {
    let mut route = vec![wp(50.0, 10.0), wp(50.0, 10.0), wp(51.0, 10.0)];
    route[0].notes = String::new();
    route[1].notes = "duplicate's note".to_owned();
    route[2].notes = "close flight plan".to_owned();
    assert!(normalize(&mut route));
    assert_eq!(route.len(), 2);
    assert_eq!(route[0].notes, "duplicate's note");
    assert_eq!(
        route[1].notes, "close flight plan",
        "destination notes stay"
    );
    assert!(!normalize(&mut route), "idempotent");

    // A set note on the keeper wins over the removed duplicate's.
    let mut route = vec![wp(50.0, 10.0), wp(50.0, 10.0), wp(51.0, 10.0)];
    route[0].notes = "keeper".to_owned();
    route[1].notes = "loser".to_owned();
    assert!(normalize(&mut route));
    assert_eq!(route[0].notes, "keeper");
}

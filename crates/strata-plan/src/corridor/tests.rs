//! Corridor engine tests. All expected numbers are worked examples
//! computed independently (Python, haversine/SLERP/destination-point/
//! cross-track on the same IUGG R₁ sphere, R = 6 371 008.8 m); the worked
//! values appear in comments next to each assertion.
//!
//! Shared reference route: the meridian leg (50°N, 10°E) → (51°N, 10°E),
//! total length 1° of arc = 111 195.080 m. On a meridian, along-track
//! distance maps linearly to latitude: `lat = 50 + along / total`.

use std::cell::Cell;

use strata_data::domain::{
    Airspace, AirspaceClass, AirspaceKind, BoundingBox, LatLon, Meters, MetersAgl, MetersAmsl,
    Obstacle, ObstacleKind, Polygon, VerticalLimit,
};

use super::geometry::{destination_point, project_onto_leg};
use super::*;
use crate::flight::{FreePoint, RoutePoint, RouteWaypoint};
use crate::units::DegreesTrue;

/// 1° of arc on the R₁ sphere (= length of the reference meridian route).
const MERIDIAN_TOTAL: f64 = 111_195.080_233_533;

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

fn wp(lat: f64, lon: f64) -> RouteWaypoint {
    RouteWaypoint::new(RoutePoint::Free(FreePoint {
        name: None,
        position: ll(lat, lon),
    }))
}

/// The reference meridian route (50, 10) → (51, 10).
fn meridian_route() -> Vec<RouteWaypoint> {
    vec![wp(50.0, 10.0), wp(51.0, 10.0)]
}

fn params(half_width: f64, spacing: f64, per_side: usize) -> CorridorParams {
    CorridorParams {
        half_width: Meters(half_width),
        station_spacing: Meters(spacing),
        lateral_samples_per_side: per_side,
    }
}

// --- synthetic sources -------------------------------------------------

/// Terrain from a closure over the sample position.
struct FnTerrain<F: Fn(LatLon) -> Option<f64>>(F);

impl<F: Fn(LatLon) -> Option<f64>> ElevationSource for FnTerrain<F> {
    fn max_elevation_at(&self, p: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        Ok((self.0)(p).map(MetersAmsl))
    }
}

/// Flat 100 m terrain everywhere (a quiet default).
fn flat_terrain() -> FnTerrain<impl Fn(LatLon) -> Option<f64>> {
    FnTerrain(|_| Some(100.0))
}

/// Counts elevation queries (lateral sample count check).
struct CountingTerrain {
    calls: Cell<usize>,
}

impl ElevationSource for CountingTerrain {
    fn max_elevation_at(&self, _: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        self.calls.set(self.calls.get() + 1);
        Ok(Some(MetersAmsl(0.0)))
    }
}

struct FailingTerrain;

impl ElevationSource for FailingTerrain {
    fn max_elevation_at(&self, _: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
        Err(SourceError::new("elevation store on fire"))
    }
}

/// Obstacles with **strict bbox semantics**: only obstacles whose position
/// is inside the query bbox are returned — so these tests also prove the
/// corridor pads its query bbox laterally (an off-track obstacle at
/// longitude 10.02 is invisible to an unpadded bbox of the meridian
/// route, whose stations all sit at longitude 10.0 exactly).
struct VecObstacles(Vec<Obstacle>);

impl ObstacleSource for VecObstacles {
    fn obstacles_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Obstacle>, SourceError> {
        Ok(self
            .0
            .iter()
            .filter(|o| bbox.contains(o.position))
            .cloned()
            .collect())
    }
}

fn no_obstacles() -> VecObstacles {
    VecObstacles(Vec::new())
}

struct FailingObstacles;

impl ObstacleSource for FailingObstacles {
    fn obstacles_in_bbox(&self, _: BoundingBox) -> Result<Vec<Obstacle>, SourceError> {
        Err(SourceError::new("obstacle store on fire"))
    }
}

/// Airspaces with R*Tree semantics: geometry bbox intersects query bbox.
struct VecAirspaces(Vec<Airspace>);

impl AirspaceSource for VecAirspaces {
    fn airspaces_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airspace>, SourceError> {
        Ok(self
            .0
            .iter()
            .filter(|a| bbox.intersects(&a.geometry.bounding_box()))
            .cloned()
            .collect())
    }
}

fn no_airspaces() -> VecAirspaces {
    VecAirspaces(Vec::new())
}

struct FailingAirspaces;

impl AirspaceSource for FailingAirspaces {
    fn airspaces_in_bbox(&self, _: BoundingBox) -> Result<Vec<Airspace>, SourceError> {
        Err(SourceError::new("airspace store on fire"))
    }
}

fn obstacle(name: &str, lat: f64, lon: f64, top_amsl: f64) -> Obstacle {
    Obstacle {
        name: Some(name.into()),
        kind: ObstacleKind::Mast,
        position: ll(lat, lon),
        height: MetersAgl(top_amsl / 2.0),
        elevation_top: MetersAmsl(top_amsl),
        lighted: false,
    }
}

fn airspace_from_rings(name: &str, exterior: Vec<LatLon>, holes: Vec<Vec<LatLon>>) -> Airspace {
    Airspace {
        name: name.into(),
        class: AirspaceClass::D,
        kind: AirspaceKind::Ctr,
        lower: VerticalLimit::amsl(MetersAmsl(0.0)),
        upper: VerticalLimit::fl(100),
        geometry: Polygon::new(exterior, holes).unwrap(),
        airac: None,
    }
}

fn box_ring(south: f64, west: f64, north: f64, east: f64) -> Vec<LatLon> {
    vec![
        ll(south, west),
        ll(south, east),
        ll(north, east),
        ll(north, west),
    ]
}

fn box_airspace(name: &str, south: f64, west: f64, north: f64, east: f64) -> Airspace {
    airspace_from_rings(name, box_ring(south, west, north, east), vec![])
}

fn sample(
    route: &[RouteWaypoint],
    p: &CorridorParams,
    elevation: &dyn ElevationSource,
    obstacles: &dyn ObstacleSource,
    airspaces: &dyn AirspaceSource,
) -> Corridor {
    sample_corridor(route, p, elevation, obstacles, airspaces).unwrap()
}

// --- params ------------------------------------------------------------

#[test]
fn default_params_are_the_documented_resolution() {
    let params = CorridorParams::default();
    assert_eq!(params.half_width, Meters(9260.0)); // 5 NM
    assert_eq!(params.station_spacing, Meters(500.0));
    assert_eq!(params.lateral_samples_per_side, 4);
}

// --- private geometry helpers -----------------------------------------

#[test]
fn destination_point_due_east_worked_example() {
    // Python (spherical direct formula, R = 6 371 008.8):
    // dest((50.5, 10.0), 90°, 2000 m) = (50.4999965752275, 10.02827703548337)
    // (a pure east offset loses a hair of latitude on the sphere).
    let p = destination_point(ll(50.5, 10.0), DegreesTrue::new(90.0), Meters(2000.0));
    assert!(
        (p.lat() - 50.499_996_575_227_5).abs() < 1e-9,
        "lat {}",
        p.lat()
    );
    assert!(
        (p.lon() - 10.028_277_035_483_37).abs() < 1e-9,
        "lon {}",
        p.lon()
    );
}

#[test]
fn destination_point_zero_distance_is_the_origin() {
    let origin = ll(50.5, 10.0);
    assert_eq!(
        destination_point(origin, DegreesTrue::new(123.0), Meters(0.0)),
        origin
    );
}

#[test]
fn project_onto_leg_worked_example() {
    // Python cross-track vs the meridian leg (50,10) → (51,10):
    // point (50.45, 10.02): along = 50037.977 m, cross = 1416.072 m.
    let proj = project_onto_leg(ll(50.0, 10.0), ll(51.0, 10.0), ll(50.45, 10.02));
    assert!(
        (proj.along.0 - 50_037.977).abs() < 1e-3,
        "along {}",
        proj.along.0
    );
    assert!(
        (proj.cross.0 - 1_416.072).abs() < 1e-3,
        "cross {}",
        proj.cross.0
    );
}

#[test]
fn project_onto_leg_behind_the_start_is_negative() {
    // Point south of the leg start projects behind it.
    let proj = project_onto_leg(ll(50.0, 10.0), ll(51.0, 10.0), ll(49.9, 10.0));
    // 0.1° of arc = 11 119.508 m.
    assert!(
        (proj.along.0 + 11_119.508).abs() < 1e-3,
        "along {}",
        proj.along.0
    );
    assert!(proj.cross.0.abs() < 1e-6, "cross {}", proj.cross.0);
}

// --- station math -------------------------------------------------------

#[test]
fn station_count_and_positions_on_the_meridian() {
    // total = 111 195.080 m, spacing 10 000 m → regular stations at
    // 0..=110 000 (12 of them) plus the final partial at the destination.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert_eq!(corridor.samples.len(), 13);
    for (i, s) in corridor.samples.iter().enumerate() {
        let st = s.station;
        assert_eq!(st.index, i);
        assert_eq!(st.leg_index, 0);
        let expected_along = if i == 12 {
            MERIDIAN_TOTAL
        } else {
            i as f64 * 10_000.0
        };
        assert!(
            (st.along_track.0 - expected_along).abs() < 1e-6,
            "station {i}"
        );
        // Meridian: lat = 50 + along / total, lon = 10 exactly.
        let expected_lat = 50.0 + expected_along / MERIDIAN_TOTAL;
        assert!(
            (st.position.lat() - expected_lat).abs() < 1e-9,
            "station {i}"
        );
        assert!((st.position.lon() - 10.0).abs() < 1e-9, "station {i}");
    }
    // Along-track strictly increasing.
    for pair in corridor.samples.windows(2) {
        assert!(pair[0].station.along_track.0 < pair[1].station.along_track.0);
    }
    // The final station is the exact destination.
    let last = corridor.samples.last().unwrap().station;
    assert!((last.position.lat() - 51.0).abs() < 1e-9);
}

#[test]
fn exact_multiple_spacing_emits_no_duplicate_final_station() {
    // spacing = total / 4: the 5th regular station lands exactly on the
    // destination — no extra partial station may be appended.
    let route = meridian_route();
    let total = crate::route::total_distance(&route).0;
    let corridor = sample(
        &route,
        &params(2000.0, total / 4.0, 1),
        &flat_terrain(),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert_eq!(corridor.samples.len(), 5);
    let last = corridor.samples.last().unwrap().station;
    assert!((last.along_track.0 - total).abs() < 1e-6);
    assert!((last.position.lat() - 51.0).abs() < 1e-9);
}

#[test]
fn station_on_a_leg_boundary_belongs_to_the_next_leg() {
    // Dogleg (50,10) → (50.2,10) → (50.2,10.3).
    // Python: leg0 = 22 239.016 m, leg1 = 21 353.100 m, total = 43 592.116 m.
    // spacing = leg0 / 2 → station 2 sits exactly on the corner. The
    // spacing is derived from the same distance call the engine uses so
    // that 2 × spacing equals the leg length to the bit (halving and
    // doubling are exact in binary floating point).
    let route = vec![wp(50.0, 10.0), wp(50.2, 10.0), wp(50.2, 10.3)];
    let leg0 = crate::route::great_circle_distance(ll(50.0, 10.0), ll(50.2, 10.0)).0;
    assert!((leg0 - 22_239.016_046_706).abs() < 1e-3); // worked value
    let corridor = sample(
        &route,
        &params(2000.0, leg0 / 2.0, 1),
        &flat_terrain(),
        &no_obstacles(),
        &no_airspaces(),
    );
    // Stations: 0, leg0/2, leg0, 3·leg0/2, final at total. (total/spacing
    // = 3.92 → k = 0..=3 regular + partial.)
    assert_eq!(corridor.samples.len(), 5);
    assert_eq!(corridor.samples[1].station.leg_index, 0);
    let corner = corridor.samples[2].station;
    assert!((corner.along_track.0 - leg0).abs() < 1e-3);
    assert_eq!(
        corner.leg_index, 1,
        "boundary station belongs to the next leg"
    );
    assert!((corner.position.lat() - 50.2).abs() < 1e-9);
    assert!((corner.position.lon() - 10.0).abs() < 1e-9);
    assert_eq!(corridor.samples[4].station.leg_index, 1);
    assert!((corridor.samples[4].station.along_track.0 - 43_592.116_466_835).abs() < 1e-3);
}

#[test]
fn coincident_route_yields_the_single_departure_station() {
    let route = vec![wp(50.0, 10.0), wp(50.0, 10.0)];
    let corridor = sample(
        &route,
        &params(2000.0, 500.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert_eq!(corridor.samples.len(), 1);
    let only = corridor.samples[0].station;
    assert_eq!(only.along_track, Meters(0.0));
    assert_eq!(only.position, ll(50.0, 10.0));
    assert!(corridor.crossings.is_empty());
}

#[test]
fn each_station_takes_two_n_plus_one_lateral_samples() {
    for per_side in [0usize, 1, 4] {
        let counting = CountingTerrain {
            calls: Cell::new(0),
        };
        let corridor = sample(
            &meridian_route(),
            &params(2000.0, 10_000.0, per_side),
            &counting,
            &no_obstacles(),
            &no_airspaces(),
        );
        assert_eq!(
            counting.calls.get(),
            corridor.samples.len() * (2 * per_side + 1),
            "per_side = {per_side}"
        );
    }
}

// --- terrain -------------------------------------------------------------

#[test]
fn inclined_plane_along_track_peaks_at_the_station_latitude() {
    // Terrain = lat × 100 m, rising along the northbound track. East/west
    // offsets only *lose* latitude on the sphere, so the centerline sample
    // is the worst case. Station 5 (along 50 000 m):
    // lat = 50 + 50 000 / 111 195.080 = 50.449 660 18 → 5 044.966 018 m.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 4),
        &FnTerrain(|p| Some(p.lat() * 100.0)),
        &no_obstacles(),
        &no_airspaces(),
    );
    let terrain = corridor.samples[5].max_terrain.unwrap();
    assert!(
        (terrain.0 - 5_044.966_018).abs() < 1e-3,
        "got {}",
        terrain.0
    );
    assert!(corridor.samples.iter().all(|s| s.max_terrain.is_some()));
}

#[test]
fn inclined_plane_across_track_peaks_at_the_outermost_east_sample() {
    // Terrain = lon × 100 m, rising due east. half_width 2000 m, n = 4 →
    // the worst case is the full-half-width east sample. At station 5
    // (lat 50.449 660 18) Python gives that sample as
    // (50.449 656 76, 10.028 246 94) → terrain = 1 002.824 694 0 m.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 4),
        &FnTerrain(|p| Some(p.lon() * 100.0)),
        &no_obstacles(),
        &no_airspaces(),
    );
    let terrain = corridor.samples[5].max_terrain.unwrap();
    assert!(
        (terrain.0 - 1_002.824_694).abs() < 1e-3,
        "got {}",
        terrain.0
    );
}

#[test]
fn off_track_ridge_appears_at_every_covering_corridor_width() {
    // The safety property. A step ridge east of the track: 2000 m terrain
    // for lon ≥ 10.02, else 100 m. The ridge edge is 1399.5–1429.5 m east
    // of the meridian track (0.02° of longitude at lat 51 / lat 50).
    //
    // Because the outermost lateral sample sits exactly on the corridor
    // edge, every half-width whose edge reaches the ridge must see it at
    // EVERY station. Python, outermost sample longitude at lat 50 (the
    // narrowest point): hw 1500 → 10.020 986 ≥ 10.02 (covers); hw 1000 →
    // 10.013 991 < 10.02 (does not cover).
    let ridge = |p: LatLon| Some(if p.lon() >= 10.02 { 2000.0 } else { 100.0 });
    for half_width in [1500.0, 2000.0, 3000.0, 5000.0, 9260.0] {
        for per_side in [1usize, 2, 4] {
            let corridor = sample(
                &meridian_route(),
                &params(half_width, 10_000.0, per_side),
                &FnTerrain(ridge),
                &no_obstacles(),
                &no_airspaces(),
            );
            for s in &corridor.samples {
                assert_eq!(
                    s.max_terrain,
                    Some(MetersAmsl(2000.0)),
                    "ridge missed at station {} (hw {half_width}, n {per_side})",
                    s.station.index
                );
            }
        }
    }
    // A corridor too narrow to cover the ridge correctly excludes it.
    let corridor = sample(
        &meridian_route(),
        &params(1000.0, 10_000.0, 4),
        &FnTerrain(ridge),
        &no_obstacles(),
        &no_airspaces(),
    );
    for s in &corridor.samples {
        assert_eq!(s.max_terrain, Some(MetersAmsl(100.0)));
    }
}

#[test]
fn terrain_coverage_none_partial_and_lateral_only() {
    // No coverage at all → None at every station.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &FnTerrain(|_| None),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert!(corridor.samples.iter().all(|s| s.max_terrain.is_none()));

    // Coverage only east of lon 10.001: the centerline (lon 10.0) and west
    // samples are outside coverage, the east samples carry the value.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &FnTerrain(|p| (p.lon() >= 10.001).then_some(321.0)),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert!(
        corridor
            .samples
            .iter()
            .all(|s| s.max_terrain == Some(MetersAmsl(321.0)))
    );
}

#[test]
fn terrain_extrema_split_min_and_max_across_the_corridor_width() {
    // The step ridge again: 2000 m east of lon 10.02, 100 m under the
    // track. max sees the ridge, min keeps the valley the track flies
    // over — the statistic AGL floors ride on.
    let ridge = |p: LatLon| Some(if p.lon() >= 10.02 { 2000.0 } else { 100.0 });
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 4),
        &FnTerrain(ridge),
        &no_obstacles(),
        &no_airspaces(),
    );
    for s in &corridor.samples {
        assert_eq!(s.max_terrain, Some(MetersAmsl(2000.0)));
        assert_eq!(s.min_terrain, Some(MetersAmsl(100.0)));
        assert!(s.min_terrain <= s.max_terrain);
    }

    // min and max are None together (outside coverage), and the sample
    // round-trips through serde with both statistics intact.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &FnTerrain(|_| None),
        &no_obstacles(),
        &no_airspaces(),
    );
    assert!(
        corridor
            .samples
            .iter()
            .all(|s| s.min_terrain.is_none() && s.max_terrain.is_none())
    );
    let json = serde_json::to_string(&corridor).expect("corridor serializes");
    let back: Corridor = serde_json::from_str(&json).expect("corridor deserializes");
    assert_eq!(back, corridor);
}

// --- obstacles -----------------------------------------------------------

#[test]
fn obstacle_included_by_lateral_distance_and_assigned_to_the_nearest_station() {
    // Python: obstacle (50.45, 10.02) vs the meridian leg —
    // along = 50 037.977 m, cross = 1 416.072 m ≤ hw 2000 → included;
    // nearest station (spacing 10 000) is along 50 000 → index 5.
    // NB: stations all sit at lon 10.0 exactly, so this also proves the
    // query bbox is padded laterally (the mock filters by bbox.contains).
    let tall = obstacle("mast", 50.45, 10.02, 300.0);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![tall.clone()]),
        &no_airspaces(),
    );
    for (i, s) in corridor.samples.iter().enumerate() {
        if i == 5 {
            assert_eq!(s.tallest_obstacle.as_ref(), Some(&tall));
        } else {
            assert!(
                s.tallest_obstacle.is_none(),
                "unexpected obstacle at station {i}"
            );
        }
    }
}

#[test]
fn obstacle_excluded_by_lateral_distance() {
    // Python: obstacle (50.45, 10.05) — cross = 3 540.181 m > hw 2000 →
    // excluded everywhere; widening the corridor to 4000 m includes it.
    let far = obstacle("far-mast", 50.45, 10.05, 300.0);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![far.clone()]),
        &no_airspaces(),
    );
    assert!(
        corridor
            .samples
            .iter()
            .all(|s| s.tallest_obstacle.is_none())
    );

    let corridor = sample(
        &meridian_route(),
        &params(4000.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![far.clone()]),
        &no_airspaces(),
    );
    assert_eq!(corridor.samples[5].tallest_obstacle.as_ref(), Some(&far));
}

#[test]
fn tallest_obstacle_wins_per_station() {
    // Both map to station 5 (Python: along 50 037.977 and 50 149.029,
    // cross 1 416.072 and 708.021); the higher top (500 m) must win.
    let lower = obstacle("lower", 50.45, 10.02, 300.0);
    let higher = obstacle("higher", 50.451, 10.01, 500.0);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![lower, higher.clone()]),
        &no_airspaces(),
    );
    assert_eq!(corridor.samples[5].tallest_obstacle.as_ref(), Some(&higher));
}

#[test]
fn obstacle_beyond_the_destination_attaches_to_the_final_station() {
    // Python: obstacle (51.0005, 10.0005) projects beyond the leg end
    // (along = 111 250.678 > total) but is 65.691 m from the destination
    // vertex → caught by the vertex check, assigned to the final station.
    // Obstacle (51.05, 10.0) is 5 559.754 m beyond the end → excluded.
    let near_end = obstacle("near-end", 51.0005, 10.0005, 400.0);
    let beyond = obstacle("beyond", 51.05, 10.0, 400.0);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![near_end.clone(), beyond]),
        &no_airspaces(),
    );
    let last = corridor.samples.last().unwrap();
    assert_eq!(last.tallest_obstacle.as_ref(), Some(&near_end));
    let others = &corridor.samples[..corridor.samples.len() - 1];
    assert!(others.iter().all(|s| s.tallest_obstacle.is_none()));
}

#[test]
fn obstacle_in_the_turn_wedge_is_caught_by_the_vertex_check() {
    // Dogleg (50,10) → (50.2,10) → (50.2,10.3); obstacle (50.201, 9.999)
    // sits in the outside wedge of the turn. Python: its perpendicular
    // projection misses both legs (leg 0: along 22 350.212 > 22 239.016;
    // leg 1: along −70.952 < 0) but it is 132.024 m from the corner
    // vertex → assigned to the corner station (index 2, see the
    // leg-boundary test for the station grid).
    let route = vec![wp(50.0, 10.0), wp(50.2, 10.0), wp(50.2, 10.3)];
    let corner_obstacle = obstacle("wedge", 50.201, 9.999, 250.0);
    let leg0 = crate::route::great_circle_distance(ll(50.0, 10.0), ll(50.2, 10.0)).0;
    let corridor = sample(
        &route,
        &params(2000.0, leg0 / 2.0, 1),
        &flat_terrain(),
        &VecObstacles(vec![corner_obstacle.clone()]),
        &no_airspaces(),
    );
    assert_eq!(
        corridor.samples[2].tallest_obstacle.as_ref(),
        Some(&corner_obstacle)
    );
    for (i, s) in corridor.samples.iter().enumerate() {
        if i != 2 {
            assert!(
                s.tallest_obstacle.is_none(),
                "unexpected obstacle at station {i}"
            );
        }
    }
}

// --- airspaces -----------------------------------------------------------

#[test]
fn airspace_box_crossing_at_known_distances() {
    // Box lat [50.40, 50.50], lon [9.9, 10.1]; spacing 1000 m. Station k
    // sits at lat 50 + k·1000/111195.080: k=44 → 50.395 698 (outside),
    // k=45 → 50.404 694 (inside) … k=55 → 50.494 626 (inside), k=56 →
    // 50.503 623 (outside). One crossing, entry 45 000, exit 55 000.
    let zone = box_airspace("CTR TEST", 50.40, 9.9, 50.50, 10.1);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 1000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![zone]),
    );
    assert_eq!(corridor.crossings.len(), 1);
    let crossing = &corridor.crossings[0];
    assert_eq!(crossing.airspace.name, "CTR TEST");
    assert!((crossing.entry_along_track.0 - 45_000.0).abs() < 1e-6);
    assert!((crossing.exit_along_track.0 - 55_000.0).abs() < 1e-6);
}

#[test]
fn airspace_cylinder_clipped_at_known_distances() {
    // A 72-gon "cylinder" of radius 5000 m centered on the track at
    // (50.45, 10.0). Python: the center is at along 50 037.786, so the
    // circle spans along [45 037.8, 55 037.8]. With 1000 m stations the
    // first inside station is 46 000 (4 037.8 m from the center, inside
    // even the polygon's inradius 5000·cos(π/72) = 4 995.2) and the last
    // is 55 000 (4 962.2 m); 45 000 and 56 000 are outside the circle.
    // Lateral samples are perpendicular to the track, so for a
    // track-centered cylinder they never extend the along-track interval.
    let center: (f64, f64) = (50.45, 10.0);
    let radius = 5000.0;
    let ring: Vec<LatLon> = (0..72)
        .map(|i| {
            let phi = f64::from(i) * 5.0_f64.to_radians();
            // Planar-degree circle: exact in latitude (meridian metric is
            // uniform), cos-scaled in longitude.
            let dlat = radius * phi.sin() / super::geometry::METERS_PER_DEGREE;
            let dlon = radius * phi.cos()
                / (super::geometry::METERS_PER_DEGREE * center.0.to_radians().cos());
            ll(center.0 + dlat, center.1 + dlon)
        })
        .collect();
    let cylinder = airspace_from_rings("ED-R CYL", ring, vec![]);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 1000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![cylinder]),
    );
    assert_eq!(corridor.crossings.len(), 1);
    let crossing = &corridor.crossings[0];
    assert!((crossing.entry_along_track.0 - 46_000.0).abs() < 1e-6);
    assert!((crossing.exit_along_track.0 - 55_000.0).abs() < 1e-6);
}

#[test]
fn airspace_reached_only_by_lateral_samples() {
    // Box entirely east of the track: lon [10.01, 10.05], lat [50.40,
    // 50.50]. Python, east lateral sample longitudes at lat 50.45:
    // hw 2000, n 2 → 10.014 124 and 10.028 247 (both inside the box) →
    // crossing detected; hw 500, n 2 → 10.003 531 and 10.007 062 (both
    // west of 10.01) → no crossing.
    let zone = box_airspace("EAST BOX", 50.40, 10.01, 50.50, 10.05);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 1000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![zone.clone()]),
    );
    assert_eq!(corridor.crossings.len(), 1);
    assert!((corridor.crossings[0].entry_along_track.0 - 45_000.0).abs() < 1e-6);
    assert!((corridor.crossings[0].exit_along_track.0 - 55_000.0).abs() < 1e-6);

    let corridor = sample(
        &meridian_route(),
        &params(500.0, 1000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![zone]),
    );
    assert!(corridor.crossings.is_empty());
}

#[test]
fn hysteresis_bridges_a_single_station_gap() {
    // Box lat [50.40, 50.50] with a full-width notch (hole) at lat
    // [50.4490, 50.4505]. Stations every 500 m sit at lat
    // 50 + k·0.004 496 60: k=99 → 50.445 164 (in the box), k=100 →
    // 50.449 660 (inside the notch → outside the airspace), k=101 →
    // 50.454 157 (in the box). Without hysteresis that one flickering
    // station would shatter the crossing into two micro-intervals; with
    // it there is ONE crossing over the full box: first inside station
    // k=89 (lat 50.400 198, along 44 500), last k=111 (lat 50.499 123,
    // along 55 500).
    let zone = airspace_from_rings(
        "NOTCHED",
        box_ring(50.40, 9.9, 50.50, 10.1),
        vec![box_ring(50.4490, 9.9, 50.4505, 10.1)],
    );
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 500.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![zone]),
    );
    assert_eq!(
        corridor.crossings.len(),
        1,
        "flicker must not split the crossing"
    );
    let crossing = &corridor.crossings[0];
    assert!((crossing.entry_along_track.0 - 44_500.0).abs() < 1e-6);
    assert!((crossing.exit_along_track.0 - 55_500.0).abs() < 1e-6);
}

#[test]
fn hysteresis_keeps_two_station_gaps_separate() {
    // Same box, wider notch [50.4490, 50.4546]: now stations k=100
    // (50.449 660) AND k=101 (50.454 157) fall in the notch (k=102 →
    // 50.458 653 is back in the box). A two-station gap is a real exit →
    // two crossings: [44 500, 49 500] (k=89..99) and [51 000, 55 500]
    // (k=102..111).
    let zone = airspace_from_rings(
        "WIDE NOTCH",
        box_ring(50.40, 9.9, 50.50, 10.1),
        vec![box_ring(50.4490, 9.9, 50.4546, 10.1)],
    );
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 500.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![zone]),
    );
    assert_eq!(corridor.crossings.len(), 2);
    assert!((corridor.crossings[0].entry_along_track.0 - 44_500.0).abs() < 1e-6);
    assert!((corridor.crossings[0].exit_along_track.0 - 49_500.0).abs() < 1e-6);
    assert!((corridor.crossings[1].entry_along_track.0 - 51_000.0).abs() < 1e-6);
    assert!((corridor.crossings[1].exit_along_track.0 - 55_500.0).abs() < 1e-6);
}

#[test]
fn single_station_graze_is_kept_not_dropped() {
    // A sliver of airspace containing exactly one station (k=100 at lat
    // 50.449 660, see above): one zero-length crossing with
    // entry == exit == 50 000 — a graze must never be silently dropped.
    let sliver = box_airspace("SLIVER", 50.4490, 9.9, 50.4505, 10.1);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 500.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![sliver]),
    );
    assert_eq!(corridor.crossings.len(), 1);
    let crossing = &corridor.crossings[0];
    assert!((crossing.entry_along_track.0 - 50_000.0).abs() < 1e-6);
    assert_eq!(crossing.entry_along_track, crossing.exit_along_track);
}

#[test]
fn crossings_are_ordered_by_entry_distance() {
    // Source order deliberately reversed: LATER first in the vec.
    let later = box_airspace("LATER", 50.60, 9.9, 50.70, 10.1);
    let earlier = box_airspace("EARLIER", 50.10, 9.9, 50.20, 10.1);
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 1000.0, 2),
        &flat_terrain(),
        &no_obstacles(),
        &VecAirspaces(vec![later, earlier]),
    );
    assert_eq!(corridor.crossings.len(), 2);
    assert_eq!(corridor.crossings[0].airspace.name, "EARLIER");
    assert_eq!(corridor.crossings[1].airspace.name, "LATER");
    assert!(corridor.crossings[0].entry_along_track.0 < corridor.crossings[1].entry_along_track.0);
}

// --- errors --------------------------------------------------------------

#[test]
fn route_too_short_is_rejected() {
    for route in [Vec::new(), vec![wp(50.0, 10.0)]] {
        let result = sample_corridor(
            &route,
            &CorridorParams::default(),
            &flat_terrain(),
            &no_obstacles(),
            &no_airspaces(),
        );
        assert!(matches!(result, Err(CorridorError::RouteTooShort)));
    }
}

#[test]
fn invalid_params_are_rejected() {
    let cases = [
        params(2000.0, 0.0, 2),          // zero spacing
        params(2000.0, -1.0, 2),         // negative spacing
        params(2000.0, f64::NAN, 2),     // NaN spacing
        params(-1.0, 500.0, 2),          // negative half-width
        params(f64::INFINITY, 500.0, 2), // infinite half-width
        params(2000.0, 1e-9, 2),         // would exceed the station cap
    ];
    for p in cases {
        let result = sample_corridor(
            &meridian_route(),
            &p,
            &flat_terrain(),
            &no_obstacles(),
            &no_airspaces(),
        );
        assert!(
            matches!(result, Err(CorridorError::InvalidParams(_))),
            "params {p:?} must be rejected"
        );
    }
}

#[test]
fn zero_half_width_samples_the_centerline_only() {
    // Degenerate but well-defined: half_width 0 → all lateral samples
    // coincide with the centerline; an off-track obstacle is excluded.
    let corridor = sample(
        &meridian_route(),
        &params(0.0, 10_000.0, 2),
        &flat_terrain(),
        &VecObstacles(vec![obstacle("off", 50.45, 10.02, 300.0)]),
        &no_airspaces(),
    );
    assert!(
        corridor
            .samples
            .iter()
            .all(|s| s.tallest_obstacle.is_none())
    );
    assert!(
        corridor
            .samples
            .iter()
            .all(|s| s.max_terrain == Some(MetersAmsl(100.0)))
    );
}

#[test]
fn source_errors_propagate() {
    let route = meridian_route();
    let p = params(2000.0, 10_000.0, 2);

    let result = sample_corridor(
        &route,
        &p,
        &FailingTerrain,
        &no_obstacles(),
        &no_airspaces(),
    );
    assert!(matches!(result, Err(CorridorError::Source(_))));

    let result = sample_corridor(
        &route,
        &p,
        &flat_terrain(),
        &FailingObstacles,
        &no_airspaces(),
    );
    assert!(matches!(result, Err(CorridorError::Source(_))));

    let result = sample_corridor(
        &route,
        &p,
        &flat_terrain(),
        &no_obstacles(),
        &FailingAirspaces,
    );
    assert!(matches!(result, Err(CorridorError::Source(_))));
}

// --- serde ---------------------------------------------------------------

#[test]
fn corridor_round_trips_through_json() {
    // ComputedFlight (and thus Corridor) is serialized for the briefing
    // PDF context — the full output must survive a JSON round trip.
    let corridor = sample(
        &meridian_route(),
        &params(2000.0, 10_000.0, 2),
        &FnTerrain(|p| Some(p.lat() * 100.0)),
        &VecObstacles(vec![obstacle("mast", 50.45, 10.02, 300.0)]),
        &VecAirspaces(vec![box_airspace("CTR TEST", 50.40, 9.9, 50.50, 10.1)]),
    );
    let json = serde_json::to_string(&corridor).unwrap();
    let back: Corridor = serde_json::from_str(&json).unwrap();
    assert_eq!(corridor, back);
}

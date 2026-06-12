//! Along-route interpolation: geodesic arc lengths over the main track and
//! marker positioning (scrub, TOC/TOD) by meters along it.
//!
//! Distances are great-circle meters between the `[lon, lat]` waypoints;
//! positions are interpolated on the *drawn* polyline (a world-space lerp
//! inside the containing leg by meter fraction), so a marker always sits
//! exactly on the rendered segment. At German leg lengths the Mercator
//! scale drift inside one leg is far below a pixel.

use crate::geo::{LatLon, world_from_lat_lon};

use glam::DVec2;

/// Mean Earth radius in meters (IUGG).
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Great-circle (haversine) distance in meters between two `[lon, lat]`
/// degree points.
pub fn haversine_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (lat1, lat2) = (a[1].to_radians(), b[1].to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b[0] - a[0]).to_radians();
    let s = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * s.sqrt().min(1.0).asin()
}

/// Cumulative geodesic meters along the track: one entry per point, first
/// entry `0.0`, last entry the total route length.
pub fn cumulative_m(track: &[[f64; 2]]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(track.len());
    let mut total = 0.0;
    for (i, p) in track.iter().enumerate() {
        if i > 0 {
            total += haversine_m(track[i - 1], *p);
        }
        cum.push(total);
    }
    cum
}

/// The world-space point `along_m` meters along the drawn polyline.
///
/// `track_world` and `cum_m` are parallel (as produced by projecting the
/// track and [`cumulative_m`]). `along_m` clamps to the track ends; `None`
/// only for an empty track. Zero-length legs are skipped.
pub fn point_at(track_world: &[DVec2], cum_m: &[f64], along_m: f64) -> Option<DVec2> {
    debug_assert_eq!(track_world.len(), cum_m.len());
    let first = *track_world.first()?;
    let total = *cum_m.last()?;
    if track_world.len() == 1 || along_m.is_nan() || along_m <= 0.0 {
        return Some(first);
    }
    if along_m >= total {
        return track_world.last().copied();
    }
    // First vertex strictly past `along_m`; bounded to 1..len by the clamps.
    let hi = cum_m.partition_point(|&c| c <= along_m);
    let lo = hi - 1;
    let span = cum_m[hi] - cum_m[lo];
    let t = if span <= f64::EPSILON {
        0.0
    } else {
        (along_m - cum_m[lo]) / span
    };
    Some(track_world[lo].lerp(track_world[hi], t))
}

/// Project a `[lon, lat]` degree pair into normalized world space.
pub fn world_from_pos(pos: [f64; 2]) -> DVec2 {
    world_from_lat_lon(LatLon::new(pos[1], pos[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(track: &[[f64; 2]]) -> Vec<DVec2> {
        track.iter().map(|&p| world_from_pos(p)).collect()
    }

    /// One degree of longitude on the equator is 1/360 of the great circle.
    #[test]
    fn haversine_matches_the_equatorial_arc() {
        let arc = haversine_m([0.0, 0.0], [1.0, 0.0]);
        let expected = std::f64::consts::TAU * EARTH_RADIUS_M / 360.0;
        assert!((arc - expected).abs() < 1e-6, "{arc} vs {expected}");
        // Symmetric and zero on identical points.
        assert_eq!(haversine_m([8.5, 50.0], [8.5, 50.0]), 0.0);
        let ab = haversine_m([8.5, 50.0], [11.0, 48.4]);
        let ba = haversine_m([11.0, 48.4], [8.5, 50.0]);
        assert!((ab - ba).abs() < 1e-9);
        // EDFE → EDQN ballpark (~200 km), sanity against gross unit errors.
        let frankfurt_to_north_bavaria = haversine_m([8.64, 49.96], [11.2, 49.9]);
        assert!((150_000.0..250_000.0).contains(&frankfurt_to_north_bavaria));
    }

    #[test]
    fn cumulative_distances_are_monotonic_and_start_at_zero() {
        let track = [[8.0, 50.0], [9.0, 50.0], [9.0, 51.0], [9.0, 51.0]];
        let cum = cumulative_m(&track);
        assert_eq!(cum.len(), 4);
        assert_eq!(cum[0], 0.0);
        assert!(cum.windows(2).all(|w| w[0] <= w[1]));
        // The duplicate point adds no length.
        assert_eq!(cum[2], cum[3]);
        assert!(cumulative_m(&[]).is_empty());
        assert_eq!(cumulative_m(&[[8.0, 50.0]]), vec![0.0]);
    }

    /// Endpoints, clamping and the degenerate track sizes.
    #[test]
    fn point_at_clamps_to_the_track_ends() {
        let track = [[8.0, 50.0], [9.0, 50.0]];
        let world = project(&track);
        let cum = cumulative_m(&track);
        assert_eq!(point_at(&world, &cum, 0.0), Some(world[0]));
        assert_eq!(point_at(&world, &cum, -5.0), Some(world[0]));
        assert_eq!(point_at(&world, &cum, cum[1]), Some(world[1]));
        assert_eq!(point_at(&world, &cum, cum[1] + 1e9), Some(world[1]));
        assert_eq!(point_at(&world, &cum, f64::INFINITY), Some(world[1]));
        assert_eq!(point_at(&world, &cum, f64::NEG_INFINITY), Some(world[0]));
        assert_eq!(point_at(&world, &cum, f64::NAN), Some(world[0]));
        assert_eq!(point_at(&[], &[], 100.0), None);
        let single = project(&[[8.0, 50.0]]);
        assert_eq!(point_at(&single, &[0.0], 100.0), Some(single[0]));
    }

    /// Halfway along one leg is the world-space midpoint of that segment —
    /// the marker sits exactly on the drawn line.
    #[test]
    fn point_at_lerps_within_a_leg() {
        let track = [[8.0, 50.0], [10.0, 50.0]];
        let world = project(&track);
        let cum = cumulative_m(&track);
        let mid = point_at(&world, &cum, cum[1] / 2.0).expect("on track");
        let expected = (world[0] + world[1]) / 2.0;
        assert!((mid - expected).length() < 1e-12);
        // Quarter point likewise.
        let quarter = point_at(&world, &cum, cum[1] / 4.0).expect("on track");
        let expected = world[0].lerp(world[1], 0.25);
        assert!((quarter - expected).length() < 1e-12);
    }

    /// A distance past the first leg walks into the second.
    #[test]
    fn point_at_walks_into_the_containing_leg() {
        let track = [[8.0, 50.0], [9.0, 50.0], [9.0, 51.0]];
        let world = project(&track);
        let cum = cumulative_m(&track);
        // Exactly at the shared vertex.
        let at_vertex = point_at(&world, &cum, cum[1]).expect("on track");
        assert!((at_vertex - world[1]).length() < 1e-12);
        // Halfway down the second leg.
        let along = cum[1] + (cum[2] - cum[1]) / 2.0;
        let p = point_at(&world, &cum, along).expect("on track");
        let expected = world[1].lerp(world[2], 0.5);
        assert!((p - expected).length() < 1e-12);
        assert!(p.x == world[1].x, "second leg is a meridian: x stays fixed");
    }

    /// Zero-length legs (duplicate waypoints) cannot produce NaN positions.
    #[test]
    fn point_at_skips_zero_length_legs() {
        let track = [[8.0, 50.0], [8.0, 50.0], [9.0, 50.0]];
        let world = project(&track);
        let cum = cumulative_m(&track);
        let p = point_at(&world, &cum, 0.0).expect("on track");
        assert!(p.x.is_finite() && p.y.is_finite());
        let mid = point_at(&world, &cum, cum[2] / 2.0).expect("on track");
        let expected = world[1].lerp(world[2], 0.5);
        assert!((mid - expected).length() < 1e-12);
    }
}

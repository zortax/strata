//! Great-circle geometry on the spherical Earth.
//!
//! **Model error:** all functions use a sphere of radius
//! [`EARTH_RADIUS_METERS`] instead of the WGS84 ellipsoid. Spherical
//! distances deviate from geodesic (Vincenty/Karney) distances by at most
//! ~0.5 % — at the longest plausible German VFR leg (~400 NM) that is
//! under 2 NM, far below planning resolution; initial-course error is a
//! few hundredths of a degree at these latitudes. Worth the simplicity;
//! revisit only if sub-0.1 % distances are ever required.

use strata_data::domain::{LatLon, Meters};

use crate::units::DegreesTrue;

/// IUGG mean Earth radius (R₁), meters.
pub const EARTH_RADIUS_METERS: f64 = 6_371_008.8;

/// Central angle between two points, radians (haversine — numerically
/// stable for the short distances planning deals in).
fn central_angle(a: LatLon, b: LatLon) -> f64 {
    let (lat_a, lon_a) = (a.lat().to_radians(), a.lon().to_radians());
    let (lat_b, lon_b) = (b.lat().to_radians(), b.lon().to_radians());
    let sin_dlat = ((lat_b - lat_a) / 2.0).sin();
    let sin_dlon = ((lon_b - lon_a) / 2.0).sin();
    let h = sin_dlat * sin_dlat + lat_a.cos() * lat_b.cos() * sin_dlon * sin_dlon;
    2.0 * h.sqrt().min(1.0).asin()
}

/// Great-circle distance between two points.
pub fn great_circle_distance(a: LatLon, b: LatLon) -> Meters {
    Meters(central_angle(a, b) * EARTH_RADIUS_METERS)
}

/// Initial true track from `a` towards `b` along the great circle, in
/// `[0, 360)`. For coincident points the bearing is undefined and `0°` is
/// returned by convention.
pub fn initial_true_track(a: LatLon, b: LatLon) -> DegreesTrue {
    let (lat_a, lon_a) = (a.lat().to_radians(), a.lon().to_radians());
    let (lat_b, lon_b) = (b.lat().to_radians(), b.lon().to_radians());
    let dlon = lon_b - lon_a;
    let y = dlon.sin() * lat_b.cos();
    let x = lat_a.cos() * lat_b.sin() - lat_a.sin() * lat_b.cos() * dlon.cos();
    DegreesTrue::new(y.atan2(x).to_degrees())
}

/// The point `fraction` of the way from `a` to `b` along the great circle
/// (spherical linear interpolation). `fraction` is clamped to `[0, 1]`;
/// coincident endpoints return `a`.
pub fn intermediate_point(a: LatLon, b: LatLon, fraction: f64) -> LatLon {
    let fraction = fraction.clamp(0.0, 1.0);
    let angle = central_angle(a, b);
    if angle < 1e-12 {
        return a;
    }
    let sin_angle = angle.sin();
    let weight_a = ((1.0 - fraction) * angle).sin() / sin_angle;
    let weight_b = (fraction * angle).sin() / sin_angle;
    let (lat_a, lon_a) = (a.lat().to_radians(), a.lon().to_radians());
    let (lat_b, lon_b) = (b.lat().to_radians(), b.lon().to_radians());
    let x = weight_a * lat_a.cos() * lon_a.cos() + weight_b * lat_b.cos() * lon_b.cos();
    let y = weight_a * lat_a.cos() * lon_a.sin() + weight_b * lat_b.cos() * lon_b.sin();
    let z = weight_a * lat_a.sin() + weight_b * lat_b.sin();
    let lat = z.atan2(x.hypot(y)).to_degrees();
    let lon = y.atan2(x).to_degrees();
    // atan2/asin ranges keep both coordinates inside their valid domains.
    LatLon::new(lat, lon).expect("spherical interpolation yields valid coordinates")
}

/// The great-circle midpoint of `a` and `b` (e.g. for per-leg magnetic
/// variation, design §4).
pub fn midpoint(a: LatLon, b: LatLon) -> LatLon {
    intermediate_point(a, b, 0.5)
}

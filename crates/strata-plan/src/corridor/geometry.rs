//! Spherical helpers private to the corridor engine: destination point,
//! leg projection (along/cross-track) and conservative bbox padding.
//!
//! Same spherical model as [`crate::route`] (IUGG R₁ sphere, documented
//! <0.5 % ellipsoid error) — corridor offsets are ≤ a few NM, where the
//! model error is centimeters.

use strata_data::domain::{BoundingBox, LatLon, Meters};

use crate::route::{EARTH_RADIUS_METERS, great_circle_distance, initial_true_track};
use crate::units::DegreesTrue;

/// Arc length of one degree on the R₁ sphere (≈ 111 195.08 m) — one degree
/// of latitude anywhere, one degree of longitude at the equator.
pub(super) const METERS_PER_DEGREE: f64 = EARTH_RADIUS_METERS * std::f64::consts::PI / 180.0;

/// The point `distance` from `origin` along the great circle leaving on
/// `bearing` (the standard spherical direct formula).
pub(super) fn destination_point(origin: LatLon, bearing: DegreesTrue, distance: Meters) -> LatLon {
    if distance.0 == 0.0 {
        return origin;
    }
    let delta = distance.0 / EARTH_RADIUS_METERS;
    let theta = bearing.0.to_radians();
    let lat1 = origin.lat().to_radians();
    let lon1 = origin.lon().to_radians();
    let sin_lat2 =
        (lat1.sin() * delta.cos() + lat1.cos() * delta.sin() * theta.cos()).clamp(-1.0, 1.0);
    let lat2 = sin_lat2.asin();
    let lon2 = lon1
        + (theta.sin() * delta.sin() * lat1.cos()).atan2(delta.cos() - lat1.sin() * sin_lat2);
    // asin keeps lat in [-90, 90]; lon1 + atan2 stays within (-540, 540),
    // so one wrap normalizes into [-180, 180).
    let lon = (lon2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0;
    LatLon::new(lat2.to_degrees(), lon).expect("spherical destination point yields valid coordinates")
}

/// Projection of a point onto the great circle through one leg.
#[derive(Debug, Clone, Copy)]
pub(super) struct LegProjection {
    /// Signed along-track distance from the leg start (negative = the foot
    /// of the perpendicular lies behind the start).
    pub along: Meters,
    /// Absolute cross-track distance to the great circle.
    pub cross: Meters,
}

/// Cross-track / along-track decomposition of `p` relative to the great
/// circle `from → to` (standard spherical cross-track formulas).
pub(super) fn project_onto_leg(from: LatLon, to: LatLon, p: LatLon) -> LegProjection {
    let delta13 = great_circle_distance(from, p).0 / EARTH_RADIUS_METERS;
    if delta13 == 0.0 {
        return LegProjection {
            along: Meters(0.0),
            cross: Meters(0.0),
        };
    }
    let theta13 = initial_true_track(from, p).0.to_radians();
    let theta12 = initial_true_track(from, to).0.to_radians();
    let dtheta = theta13 - theta12;
    let cross_angle = (delta13.sin() * dtheta.sin()).clamp(-1.0, 1.0).asin();
    let cos_cross = cross_angle.cos();
    // cos_cross ~ 0 only for points ~90° off the great circle — far outside
    // any corridor; the clamp keeps acos well-defined regardless.
    let along_angle = if cos_cross.abs() < 1e-12 {
        std::f64::consts::FRAC_PI_2
    } else {
        (delta13.cos() / cos_cross).clamp(-1.0, 1.0).acos()
    };
    let along = if dtheta.cos() >= 0.0 {
        along_angle
    } else {
        -along_angle
    };
    LegProjection {
        along: Meters(along * EARTH_RADIUS_METERS),
        cross: Meters(cross_angle.abs() * EARTH_RADIUS_METERS),
    }
}

/// Expands `bbox` outward by `pad` meters, converted to degrees
/// conservatively: 1 % slack on the spherical degree lengths and the
/// longitude width taken at the highest-|latitude| edge of the *padded*
/// box (smallest cos ⇒ widest padding). Clamped to valid coordinate
/// ranges; padding outward keeps the edges ordered.
pub(super) fn pad_bbox(bbox: BoundingBox, pad: Meters) -> BoundingBox {
    let pad_m = pad.0 * 1.01;
    let dlat = pad_m / METERS_PER_DEGREE;
    let south = (bbox.south() - dlat).max(-90.0);
    let north = (bbox.north() + dlat).min(90.0);
    // Cap at 89° so the cos never collapses; beyond that the clamp to
    // ±180° takes over anyway (irrelevant for the Germany region).
    let max_abs_lat = south.abs().max(north.abs()).min(89.0);
    let dlon = pad_m / (METERS_PER_DEGREE * max_abs_lat.to_radians().cos());
    let west = (bbox.west() - dlon).max(-180.0);
    let east = (bbox.east() + dlon).min(180.0);
    BoundingBox::new(west, south, east, north).expect("outward padding keeps bbox edges ordered")
}

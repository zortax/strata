//! Geographic value types and Web-Mercator world-space conversions.
//!
//! World space is the normalized Web-Mercator square `[0, 1]^2`:
//! `x = lon/360 + 0.5`, `y = 0.5 − asinh(tan φ)/(2π)`. `x` grows eastward,
//! `y` grows **southward** (matching screen-space y-down and XYZ tile rows).

use glam::DVec2;

use std::f64::consts::TAU;

/// Latitude limit of the Web-Mercator projection (where world `y` hits 0/1).
pub const MAX_MERCATOR_LAT_DEG: f64 = 85.051_128_779_806_59;

/// A WGS84 coordinate in degrees. Constructors clamp into the Web-Mercator
/// domain so every `LatLon` maps to a valid world-space point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    lat_deg: f64,
    lon_deg: f64,
}

impl LatLon {
    /// Clamps latitude to ±[`MAX_MERCATOR_LAT_DEG`] and longitude to ±180.
    /// Non-finite inputs collapse to 0.
    pub fn new(lat_deg: f64, lon_deg: f64) -> Self {
        let sanitize = |v: f64| if v.is_finite() { v } else { 0.0 };
        Self {
            lat_deg: sanitize(lat_deg).clamp(-MAX_MERCATOR_LAT_DEG, MAX_MERCATOR_LAT_DEG),
            lon_deg: sanitize(lon_deg).clamp(-180.0, 180.0),
        }
    }

    pub fn lat_deg(self) -> f64 {
        self.lat_deg
    }

    pub fn lon_deg(self) -> f64 {
        self.lon_deg
    }
}

/// WGS84 → normalized Web-Mercator world space `[0, 1]^2`.
pub fn world_from_lat_lon(p: LatLon) -> DVec2 {
    let x = p.lon_deg() / 360.0 + 0.5;
    let y = 0.5 - p.lat_deg().to_radians().tan().asinh() / TAU;
    DVec2::new(x, y)
}

/// Normalized Web-Mercator world space → WGS84. Input is clamped to `[0, 1]^2`.
pub fn lat_lon_from_world(world: DVec2) -> LatLon {
    let w = world.clamp(DVec2::ZERO, DVec2::ONE);
    let lon = (w.x - 0.5) * 360.0;
    let lat = ((0.5 - w.y) * TAU).sinh().atan().to_degrees();
    LatLon::new(lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equator_meridian_is_world_center() {
        let w = world_from_lat_lon(LatLon::new(0.0, 0.0));
        assert!((w.x - 0.5).abs() < 1e-15);
        assert!((w.y - 0.5).abs() < 1e-15);
    }

    #[test]
    fn world_corners() {
        let nw = world_from_lat_lon(LatLon::new(MAX_MERCATOR_LAT_DEG, -180.0));
        assert!(nw.x.abs() < 1e-12);
        assert!(nw.y.abs() < 1e-9);
        let se = world_from_lat_lon(LatLon::new(-MAX_MERCATOR_LAT_DEG, 180.0));
        assert!((se.x - 1.0).abs() < 1e-12);
        assert!((se.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn lat_lon_round_trip() {
        for &(lat, lon) in &[
            (51.1657, 10.4515), // Germany centroid
            (47.0, 5.5),
            (55.2, 15.5),
            (-33.9, 151.2),
            (0.0, 0.0),
            (84.9, -179.9),
        ] {
            let p = LatLon::new(lat, lon);
            let rt = lat_lon_from_world(world_from_lat_lon(p));
            assert!(
                (rt.lat_deg() - lat).abs() < 1e-12,
                "lat {lat} → {}",
                rt.lat_deg()
            );
            assert!(
                (rt.lon_deg() - lon).abs() < 1e-12,
                "lon {lon} → {}",
                rt.lon_deg()
            );
        }
    }

    #[test]
    fn constructor_clamps() {
        let p = LatLon::new(90.0, 200.0);
        assert_eq!(p.lat_deg(), MAX_MERCATOR_LAT_DEG);
        assert_eq!(p.lon_deg(), 180.0);
        let q = LatLon::new(f64::NAN, f64::INFINITY);
        assert_eq!(q.lat_deg(), 0.0);
        assert_eq!(q.lon_deg(), 0.0);
    }
}

//! DE1200 polar-stereographic projection — the grid of the DWD RV radar
//! composite, WGS84 ellipsoid variant.
//!
//! Parameters from the official RV format description ("Dateiformat des
//! RV-Produkts", DWD FE23, v1.01, 2022-05-02, Kap. 3): north-polar
//! stereographic with latitude of true scale 60° N and central meridian
//! 10° E on the WGS84 ellipsoid (format version `VS 5`; RV switched from
//! the spherical earth model on 2022-10-13). As a proj string:
//!
//! ```text
//! +proj=stere +lat_0=90 +lat_ts=60 +lon_0=10
//! +a=6378137 +b=6356752.3142451802
//! +x_0=543196.83521776402 +y_0=3622588.8619310018
//! ```
//!
//! The false easting/northing place the **center of the north-west pixel at
//! (0, 0)**; x grows eastward and y southward-negative, 1000 m per pixel.
//! [`forward`]/[`inverse`] implement the standard ellipsoidal polar
//! stereographic (Snyder, *Map Projections — A Working Manual*, eq. 21-33
//! ff. and the conformal-latitude series 3-5); tests pin them against the
//! four DE1200 corner coordinates documented with 10 significant digits
//! (they agree to well under a millimeter).

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use crate::domain::{GeoError, LatLon};

/// WGS84 semi-major axis, meters.
const SEMI_MAJOR_M: f64 = 6_378_137.0;
/// WGS84 semi-minor axis, meters. The RV proj string prints
/// `6356752.3142451802`; the trailing digits exceed f64 resolution and
/// parse to this same value.
const SEMI_MINOR_M: f64 = 6_356_752.314_245_18;
/// Latitude of true scale.
const LAT_TS_DEG: f64 = 60.0;
/// Central meridian.
pub(super) const LON_ORIGIN_DEG: f64 = 10.0;
/// False easting: pole → NW-pixel-center x offset (proj string
/// `543196.83521776402`, same f64).
pub(super) const FALSE_EASTING_M: f64 = 543_196.835_217_764;
/// False northing: pole → NW-pixel-center y offset (proj string
/// `3622588.8619310018`, same f64).
pub(super) const FALSE_NORTHING_M: f64 = 3_622_588.861_931_002;
/// DE1200 cell size in the projection plane.
pub(super) const PIXEL_SIZE_M: f64 = 1000.0;

/// First eccentricity squared, e² = 1 − b²/a².
fn eccentricity_sq() -> f64 {
    1.0 - (SEMI_MINOR_M * SEMI_MINOR_M) / (SEMI_MAJOR_M * SEMI_MAJOR_M)
}

/// Snyder's t(φ) (eq. 15-9): the conformal colatitude half-angle tangent.
fn half_colat_t(phi: f64) -> f64 {
    let e = eccentricity_sq().sqrt();
    let es = e * phi.sin();
    (FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - es) / (1.0 + es)).powf(e / 2.0)
}

/// ρ = k · t(φ) with k fixed by true scale at [`LAT_TS_DEG`]
/// (ρ = a·m(φ_ts)·t(φ)/t(φ_ts), Snyder eq. 21-34).
fn scale_k() -> f64 {
    let e2 = eccentricity_sq();
    let ts = LAT_TS_DEG.to_radians();
    let m_ts = ts.cos() / (1.0 - e2 * ts.sin() * ts.sin()).sqrt();
    SEMI_MAJOR_M * m_ts / half_colat_t(ts)
}

/// Distance from the pole in the projection plane for a given latitude.
/// Separable building block for [`forward`]: x and y only add the
/// longitude-dependent sin/cos factors, which lets the resampler hoist the
/// latitude term out of its inner loop.
pub(super) fn rho_m(lat_deg: f64) -> f64 {
    scale_k() * half_colat_t(lat_deg.to_radians())
}

/// Projects a WGS84 position into DE1200 grid coordinates, meters
/// (NW pixel center = (0, 0), x east, y south-negative).
///
/// Reference implementation pinned against the documented corners; the
/// resampler inlines the separable form ([`rho_m`] hoisted per latitude),
/// so production code reaches this only through tests.
#[cfg(test)]
pub(super) fn forward(p: LatLon) -> (f64, f64) {
    let rho = rho_m(p.lat());
    let (sin_d, cos_d) = (p.lon() - LON_ORIGIN_DEG).to_radians().sin_cos();
    (
        rho * sin_d + FALSE_EASTING_M,
        -rho * cos_d + FALSE_NORTHING_M,
    )
}

/// Inverse of [`forward`]: DE1200 grid coordinates (meters) back to WGS84.
/// Uses the conformal-latitude series (Snyder eq. 3-5), exact to far below
/// the 1 km pixel size.
pub(super) fn inverse(x_m: f64, y_m: f64) -> Result<LatLon, GeoError> {
    let x = x_m - FALSE_EASTING_M;
    let y = y_m - FALSE_NORTHING_M;
    let t = x.hypot(y) / scale_k();
    let chi = FRAC_PI_2 - 2.0 * t.atan();
    let e2 = eccentricity_sq();
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    let e8 = e4 * e4;
    let lat = chi
        + (e2 / 2.0 + 5.0 * e4 / 24.0 + e6 / 12.0 + 13.0 * e8 / 360.0) * (2.0 * chi).sin()
        + (7.0 * e4 / 48.0 + 29.0 * e6 / 240.0 + 811.0 * e8 / 11520.0) * (4.0 * chi).sin()
        + (7.0 * e6 / 120.0 + 81.0 * e8 / 1120.0) * (6.0 * chi).sin()
        + (4279.0 * e8 / 161_280.0) * (8.0 * chi).sin();
    // atan2(x, −y): due south of the pole (x = 0, y < 0) is the central
    // meridian. At the pole itself atan2(0, 0) = 0 → 10° E, fine.
    let lon = LON_ORIGIN_DEG + x.atan2(-y).to_degrees();
    LatLon::new(lat.to_degrees(), lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four DE1200 *outer* corner coordinates from the RV format
    /// description, Tabelle 3.2 (WGS84 earth model, 10 significant
    /// digits), with the grid coordinates of the composite's outer
    /// corners (NW pixel center = (0,0), 1100 × 1200 km area).
    const CORNERS: [(&str, f64, f64, f64, f64); 4] = [
        ("NW", 55.86208711, 1.463301510, -500.0, 500.0),
        ("NE", 55.84543856, 18.73161645, 1_099_500.0, 500.0),
        ("SE", 45.68460578, 16.58086935, 1_099_500.0, -1_199_500.0),
        ("SW", 45.69642538, 3.566994635, -500.0, -1_199_500.0),
    ];

    #[test]
    fn forward_reproduces_the_documented_corners() {
        // The task tolerance is ~100 m; the formulas actually agree with
        // the documented 10-significant-digit corners to sub-millimeter,
        // so 1 m catches any formula or constant slip.
        for (name, lat, lon, x, y) in CORNERS {
            let p = LatLon::new(lat, lon).unwrap();
            let (fx, fy) = forward(p);
            assert!(
                (fx - x).abs() < 1.0 && (fy - y).abs() < 1.0,
                "{name}: forward({lat}, {lon}) = ({fx:.3}, {fy:.3}), expected ({x}, {y})"
            );
        }
    }

    #[test]
    fn inverse_reproduces_the_documented_corners() {
        for (name, lat, lon, x, y) in CORNERS {
            let p = inverse(x, y).unwrap();
            assert!(
                (p.lat() - lat).abs() < 1e-8 && (p.lon() - lon).abs() < 1e-8,
                "{name}: inverse({x}, {y}) = {p}, expected ({lat}, {lon})"
            );
        }
    }

    #[test]
    fn round_trip_is_tight_across_the_domain() {
        for lat in [46.0, 47.5, 51.0, 54.9, 55.8] {
            for lon in [2.0, 6.3, 10.0, 13.4, 18.5] {
                let p = LatLon::new(lat, lon).unwrap();
                let (x, y) = forward(p);
                let q = inverse(x, y).unwrap();
                assert!(
                    (q.lat() - lat).abs() < 1e-9 && (q.lon() - lon).abs() < 1e-9,
                    "round trip drifted at ({lat}, {lon}): {q}"
                );
            }
        }
    }

    #[test]
    fn nw_pixel_center_is_the_grid_origin() {
        let p = inverse(0.0, 0.0).unwrap();
        let (x, y) = forward(p);
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6);
        // Half a pixel inside the documented NW outer corner.
        assert!(p.lat() < 55.86208711 && p.lon() > 1.463301510);
    }

    #[test]
    fn true_scale_holds_at_60_n() {
        // At the latitude of true scale, 1 m along a meridian in the
        // projection ≈ 1 m on the ellipsoid: dρ/dφ ≈ −M(φ) (the meridian
        // radius of curvature) at 60° N.
        let h = 1e-7_f64; // radians
        let rho1 = rho_m(60.0 - h.to_degrees());
        let rho2 = rho_m(60.0 + h.to_degrees());
        let d_rho_d_phi = (rho2 - rho1) / (2.0 * h);
        let e2 = eccentricity_sq();
        let sin60 = 60.0_f64.to_radians().sin();
        let meridian_radius =
            SEMI_MAJOR_M * (1.0 - e2) / (1.0 - e2 * sin60 * sin60).powf(1.5);
        let scale = -d_rho_d_phi / meridian_radius;
        assert!(
            (scale - 1.0).abs() < 1e-6,
            "meridian scale at 60N = {scale}"
        );
    }
}

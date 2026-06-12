//! Reprojection of DE1200 polar-stereographic frames onto a regular WGS84
//! lat-lon raster, so radar consumers see the exact same grid type as the
//! ICON forecast fields and never touch the source projection.
//!
//! The target grid is derived deterministically from the source dimensions
//! (boundary pixel centers → lat-lon bounding box, snapped to the target
//! spacing), so every frame of a given product resamples onto the
//! identical lattice — slider crossfades between frames need no regridding.
//! Each target node is forward-projected into the source plane and sampled
//! bilinearly from the four surrounding radar pixels; NaN pixels are
//! dropped with the remaining weights renormalized (matching
//! [`RegularLatLonGrid::sample`] semantics), and nodes outside the radar
//! pixel-center lattice are NaN.

use crate::domain::{GriddedError, LatLon, RegularLatLonGrid};

use super::DwdRadarError;
use super::projection;

/// Target spacing: ~1.1 km in latitude — same order as the 1 km source.
const TARGET_LAT_SPACING_DEG: f64 = 0.01;
/// Target spacing: ~1.05 km in longitude at 51° N.
const TARGET_LON_SPACING_DEG: f64 = 0.015;

/// Resamples a source raster in DE1200 pixel space (`values[j * nx + i]`,
/// i west→east, j south→north, NW pixel center at projection (0, 0)) onto
/// a [`RegularLatLonGrid`] covering all source pixel centers.
pub(super) fn resample_to_latlon(
    values: &[f32],
    nx: usize,
    ny: usize,
) -> Result<RegularLatLonGrid, DwdRadarError> {
    if values.len() != nx * ny {
        return Err(GriddedError::ValueCountMismatch { got: values.len(), ni: nx, nj: ny }.into());
    }
    let bounds = pixel_center_bounds(nx, ny)?;
    let origin = LatLon::new(
        snap_down(bounds.lat_min, TARGET_LAT_SPACING_DEG),
        snap_down(bounds.lon_min, TARGET_LON_SPACING_DEG),
    )?;
    let nj = node_count(origin.lat(), bounds.lat_max, TARGET_LAT_SPACING_DEG);
    let ni = node_count(origin.lon(), bounds.lon_max, TARGET_LON_SPACING_DEG);

    // The forward projection separates: x = ρ(lat)·sin Δλ, y = −ρ(lat)·cos Δλ,
    // so the latitude term is hoisted per row and the trig per column.
    let columns: Vec<(f64, f64)> = (0..ni)
        .map(|i| {
            let lon = origin.lon() + i as f64 * TARGET_LON_SPACING_DEG;
            (lon - projection::LON_ORIGIN_DEG).to_radians().sin_cos()
        })
        .collect();
    let mut out = Vec::with_capacity(ni * nj);
    for j in 0..nj {
        let lat = origin.lat() + j as f64 * TARGET_LAT_SPACING_DEG;
        let rho = projection::rho_m(lat);
        for &(sin_d, cos_d) in &columns {
            let x = rho * sin_d + projection::FALSE_EASTING_M;
            let y = -rho * cos_d + projection::FALSE_NORTHING_M;
            // Fractional source coordinates: u counts columns from the
            // west, v rows from the south (pixel centers at integers).
            let u = x / projection::PIXEL_SIZE_M;
            let v = y / projection::PIXEL_SIZE_M + (ny - 1) as f64;
            out.push(bilinear(values, nx, ny, u, v));
        }
    }
    Ok(RegularLatLonGrid::new(
        origin,
        TARGET_LAT_SPACING_DEG,
        TARGET_LON_SPACING_DEG,
        ni,
        nj,
        out,
    )?)
}

/// Lat-lon bounding box of all source pixel centers. Extremes occur on the
/// boundary (the projection is a continuous map of a convex region), so
/// scanning the four edges suffices; the northern edge bulges above the
/// corner latitudes near the central meridian.
struct Bounds {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

fn pixel_center_bounds(nx: usize, ny: usize) -> Result<Bounds, DwdRadarError> {
    let mut b = Bounds {
        lat_min: f64::INFINITY,
        lat_max: f64::NEG_INFINITY,
        lon_min: f64::INFINITY,
        lon_max: f64::NEG_INFINITY,
    };
    let bottom_top = (0..nx).flat_map(|i| [(i, 0), (i, ny - 1)]);
    let left_right = (0..ny).flat_map(|j| [(0, j), (nx - 1, j)]);
    for (i, j) in bottom_top.chain(left_right) {
        let x = i as f64 * projection::PIXEL_SIZE_M;
        let y = (j as f64 - (ny - 1) as f64) * projection::PIXEL_SIZE_M;
        let p = projection::inverse(x, y)?;
        b.lat_min = b.lat_min.min(p.lat());
        b.lat_max = b.lat_max.max(p.lat());
        b.lon_min = b.lon_min.min(p.lon());
        b.lon_max = b.lon_max.max(p.lon());
    }
    Ok(b)
}

/// Largest spacing multiple ≤ `v` (deterministic grid registration).
fn snap_down(v: f64, spacing: f64) -> f64 {
    (v / spacing).floor() * spacing
}

/// Nodes needed to span `origin..=max` inclusive at `spacing`.
fn node_count(origin: f64, max: f64, spacing: f64) -> usize {
    ((max - origin) / spacing).ceil() as usize + 1
}

/// Bilinear sample of the source raster at fractional pixel coordinates,
/// NaN corners dropped and weights renormalized; NaN outside the
/// pixel-center lattice or when no weighted corner holds data.
fn bilinear(values: &[f32], nx: usize, ny: usize, u: f64, v: f64) -> f32 {
    if !(0.0..=(nx - 1) as f64).contains(&u) || !(0.0..=(ny - 1) as f64).contains(&v) {
        return f32::NAN;
    }
    let i0 = (u.floor() as usize).min(nx - 2);
    let j0 = (v.floor() as usize).min(ny - 2);
    let fx = u - i0 as f64;
    let fy = v - j0 as f64;
    let corners = [
        (i0, j0, (1.0 - fx) * (1.0 - fy)),
        (i0 + 1, j0, fx * (1.0 - fy)),
        (i0, j0 + 1, (1.0 - fx) * fy),
        (i0 + 1, j0 + 1, fx * fy),
    ];
    let mut value_sum = 0.0_f64;
    let mut weight_sum = 0.0_f64;
    for (i, j, w) in corners {
        let value = values[j * nx + i];
        if w > 0.0 && !value.is_nan() {
            value_sum += f64::from(value) * w;
            weight_sum += w;
        }
    }
    if weight_sum > 0.0 {
        (value_sum / weight_sum) as f32
    } else {
        f32::NAN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small synthetic source near the DE1200 north-west corner (the
    /// resampler is dimension-generic; the NW pixel center is always the
    /// projection origin): 50 columns × 60 rows of 1 km pixels.
    const NX: usize = 50;
    const NY: usize = 60;

    /// Source whose value is the (west→east) column index — linear in the
    /// projected x coordinate, so bilinear resampling reproduces the
    /// fractional column exactly.
    fn column_ramp() -> Vec<f32> {
        (0..NX * NY).map(|k| (k % NX) as f32).collect()
    }

    #[test]
    fn target_grid_covers_all_pixel_centers() {
        let grid = resample_to_latlon(&column_ramp(), NX, NY).unwrap();
        let extent = grid.extent();
        for (i, j) in [(0, 0), (NX - 1, 0), (0, NY - 1), (NX - 1, NY - 1)] {
            let p = projection::inverse(
                i as f64 * 1000.0,
                (j as f64 - (NY - 1) as f64) * 1000.0,
            )
            .unwrap();
            assert!(extent.contains(p), "corner pixel center {p} outside {extent:?}");
        }
        // Snapped origin sits on the spacing lattice.
        assert!((grid.origin().lat() / TARGET_LAT_SPACING_DEG).fract().abs() < 1e-9);
        assert!((grid.origin().lon() / TARGET_LON_SPACING_DEG).fract().abs() < 1e-9);
    }

    #[test]
    fn ramp_resamples_to_the_projected_fractional_column() {
        let grid = resample_to_latlon(&column_ramp(), NX, NY).unwrap();
        let mut checked = 0;
        for j in 0..grid.nj() {
            for i in 0..grid.ni() {
                let value = grid.value_at(i, j).unwrap();
                if value.is_nan() {
                    continue;
                }
                let lat = grid.origin().lat() + j as f64 * TARGET_LAT_SPACING_DEG;
                let lon = grid.origin().lon() + i as f64 * TARGET_LON_SPACING_DEG;
                let (x, _y) = projection::forward(LatLon::new(lat, lon).unwrap());
                let u = x / 1000.0;
                assert!(
                    (f64::from(value) - u).abs() < 1e-3,
                    "node ({i}, {j}): value {value} vs projected column {u}"
                );
                checked += 1;
            }
        }
        // The vast majority of nodes must lie inside the source.
        assert!(checked > grid.values().len() / 2, "only {checked} nodes had data");
    }

    #[test]
    fn nodes_outside_the_source_are_nan() {
        let grid = resample_to_latlon(&column_ramp(), NX, NY).unwrap();
        // The lat-lon bounding box is strictly larger than the rotated
        // projected rectangle, so corner regions must be NaN.
        assert!(grid.values().iter().any(|v| v.is_nan()));
        // The grid's exact corner nodes lie outside the source lattice.
        let corner = grid.value_at(grid.ni() - 1, 0).unwrap();
        assert!(corner.is_nan());
    }

    #[test]
    fn nan_pixels_renormalize_and_propagate() {
        // A fully-NaN source stays fully NaN.
        let grid = resample_to_latlon(&vec![f32::NAN; NX * NY], NX, NY).unwrap();
        assert!(grid.values().iter().all(|v| v.is_nan()));

        // A half-NaN source (eastern half missing) still yields real data
        // on the western side.
        let half: Vec<f32> = (0..NX * NY)
            .map(|k| if k % NX >= NX / 2 { f32::NAN } else { 1.5 })
            .collect();
        let grid = resample_to_latlon(&half, NX, NY).unwrap();
        let real = grid.values().iter().filter(|v| !v.is_nan()).count();
        assert!(real > 0);
        assert!(grid.values().iter().filter(|v| !v.is_nan()).all(|&v| v == 1.5));
    }

    #[test]
    fn value_count_mismatch_is_rejected() {
        assert!(matches!(
            resample_to_latlon(&[0.0; 7], 2, 3),
            Err(DwdRadarError::Gridded(GriddedError::ValueCountMismatch { got: 7, ni: 2, nj: 3 }))
        ));
    }

    #[test]
    fn bilinear_matches_corner_weights() {
        // 2×2 source: values 1, 2 (south row), 3, 4 (north row).
        let values = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(bilinear(&values, 2, 2, 0.0, 0.0), 1.0);
        assert_eq!(bilinear(&values, 2, 2, 1.0, 1.0), 4.0);
        assert_eq!(bilinear(&values, 2, 2, 0.5, 0.5), 2.5);
        // NaN corner drops out, weights renormalize.
        let with_nan = [f32::NAN, 2.0, 3.0, 4.0];
        assert_eq!(bilinear(&with_nan, 2, 2, 0.5, 0.5), 3.0);
        assert!(bilinear(&with_nan, 2, 2, 0.0, 0.0).is_nan());
        // Outside the lattice.
        assert!(bilinear(&values, 2, 2, -0.1, 0.5).is_nan());
        assert!(bilinear(&values, 2, 2, 0.5, 1.1).is_nan());
    }
}

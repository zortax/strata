//! Grid geometry: frame validation and the CPU mirror of the
//! fragment-shader Mercator mapping in `shaders/weather_grid.wgsl`.

use crate::features::WeatherGridFrame;
use crate::geo::{self, LatLon, MAX_MERCATOR_LAT_DEG};

use glam::DVec2;

/// Why a [`WeatherGridFrame`] was rejected (logged, frame dropped).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum GridFrameError {
    #[error("values length {len} does not match ni*nj = {expected}")]
    ValueCountMismatch { len: usize, expected: usize },
    #[error("grid needs at least 2x2 points, got {ni}x{nj}")]
    TooFewPoints { ni: u32, nj: u32 },
    #[error("extent is not finite / ordered / inside the Mercator domain")]
    BadExtent,
}

/// Validated geographic window of one frame's grid (corner grid points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GridParams {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
    pub ni: u32,
    pub nj: u32,
}

impl GridParams {
    pub(crate) fn from_frame(frame: &WeatherGridFrame) -> Result<Self, GridFrameError> {
        let (lat_min, lon_min, lat_max, lon_max) = frame.extent;
        let (ni, nj) = (frame.ni, frame.nj);
        if ni < 2 || nj < 2 {
            return Err(GridFrameError::TooFewPoints { ni, nj });
        }
        let expected = ni as usize * nj as usize;
        if frame.values.len() != expected {
            return Err(GridFrameError::ValueCountMismatch {
                len: frame.values.len(),
                expected,
            });
        }
        let finite = [lat_min, lon_min, lat_max, lon_max]
            .iter()
            .all(|v| v.is_finite());
        if !finite
            || lat_min >= lat_max
            || lon_min >= lon_max
            || lat_min < -MAX_MERCATOR_LAT_DEG
            || lat_max > MAX_MERCATOR_LAT_DEG
            || lon_min < -180.0
            || lon_max > 180.0
        {
            return Err(GridFrameError::BadExtent);
        }
        Ok(Self {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
            ni,
            nj,
        })
    }

    /// World-space north-west corner (x grows east, y grows south).
    pub(crate) fn world_nw(&self) -> DVec2 {
        geo::world_from_lat_lon(LatLon::new(self.lat_max, self.lon_min))
    }

    /// World-space south-east corner.
    pub(crate) fn world_se(&self) -> DVec2 {
        geo::world_from_lat_lon(LatLon::new(self.lat_min, self.lon_max))
    }
}

/// World-space rectangle `(nw, size)` covering both grids (their union):
/// the blended frames may come from differently-extended sources.
pub(crate) fn union_world_rect(a: &GridParams, b: &GridParams) -> (DVec2, DVec2) {
    let nw = a.world_nw().min(b.world_nw());
    let se = a.world_se().max(b.world_se());
    (nw, se - nw)
}

/// CPU mirror of the fragment-shader latitude recovery (f32 math, see
/// `fs_main` in `shaders/weather_grid.wgsl`): inverse Mercator of the
/// absolute world-space `y`. Test-only — the real evaluation runs on the
/// GPU; this pins the formula's correctness.
#[cfg(test)]
pub(crate) fn shader_lat_from_world_y(y_abs: f32) -> f32 {
    const TAU: f32 = std::f32::consts::TAU;
    ((0.5 - y_abs) * TAU).sinh().atan().to_degrees()
}

/// CPU mirror of `grid_uv` in `shaders/weather_grid.wgsl`: half-texel UV
/// for a geographic position, `None` outside the grid window (with the
/// shader's f32-rounding tolerance at the edges). Test-only, like
/// [`shader_lat_from_world_y`].
#[cfg(test)]
pub(crate) fn shader_uv(lat: f32, lon: f32, grid: &GridParams) -> Option<(f32, f32)> {
    const WINDOW_EPS: f32 = 1e-4;
    let tx = (lon - grid.lon_min as f32) / (grid.lon_max - grid.lon_min) as f32;
    let ty = (lat - grid.lat_min as f32) / (grid.lat_max - grid.lat_min) as f32;
    if !(-WINDOW_EPS..=1.0 + WINDOW_EPS).contains(&tx)
        || !(-WINDOW_EPS..=1.0 + WINDOW_EPS).contains(&ty)
    {
        return None;
    }
    let (tx, ty) = (tx.clamp(0.0, 1.0), ty.clamp(0.0, 1.0));
    let u = (0.5 + tx * (grid.ni - 1) as f32) / grid.ni as f32;
    let v = (0.5 + ty * (grid.nj - 1) as f32) / grid.nj as f32;
    Some((u, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::GriddedField;

    /// The ICON-D2 window (regular lat-lon, 0.02°).
    fn icon_d2() -> GridParams {
        GridParams {
            lat_min: 43.18,
            lat_max: 58.08,
            lon_min: -3.94,
            lon_max: 20.34,
            ni: 1215,
            nj: 746,
        }
    }

    fn frame(ni: u32, nj: u32, extent: (f64, f64, f64, f64)) -> WeatherGridFrame {
        WeatherGridFrame {
            field: GriddedField::CloudCover,
            valid_time: 0,
            extent,
            ni,
            nj,
            values: vec![0.0; ni as usize * nj as usize],
        }
    }

    #[test]
    fn validation_accepts_a_sane_frame_and_rejects_broken_ones() {
        let extent = (43.18, -3.94, 58.08, 20.34);
        assert!(GridParams::from_frame(&frame(4, 3, extent)).is_ok());

        let mut wrong_len = frame(4, 3, extent);
        wrong_len.values.pop();
        assert_eq!(
            GridParams::from_frame(&wrong_len),
            Err(GridFrameError::ValueCountMismatch {
                len: 11,
                expected: 12
            })
        );
        assert_eq!(
            GridParams::from_frame(&frame(1, 3, extent)),
            Err(GridFrameError::TooFewPoints { ni: 1, nj: 3 })
        );
        for bad in [
            (58.08, -3.94, 43.18, 20.34), // inverted lat
            (43.18, 20.34, 58.08, -3.94), // inverted lon
            (43.18, -3.94, f64::NAN, 20.34),
            (-89.0, -3.94, 58.08, 20.34), // outside Mercator domain
        ] {
            assert_eq!(
                GridParams::from_frame(&frame(4, 3, bad)),
                Err(GridFrameError::BadExtent),
                "{bad:?}"
            );
        }
    }

    /// The fragment-shader mapping (f32 mirror) recovers the latitude from
    /// the interpolated world position at the grid's south edge, middle and
    /// north edge — Mercator's latitude nonlinearity is handled exactly,
    /// not approximated per row.
    #[test]
    fn mercator_mapping_recovers_latitudes_across_the_grid() {
        let grid = icon_d2();
        let nw = grid.world_nw();
        let se = grid.world_se();
        let size = se - nw;
        for lat in [43.18_f64, 50.5, 58.08] {
            let world_y = geo::world_from_lat_lon(LatLon::new(lat, 10.0)).y;
            // What the fragment computes: nw_abs.y + pos01.y * size.y in f32.
            let pos01_y = ((world_y - nw.y) / size.y) as f32;
            let y_abs = nw.y as f32 + pos01_y * size.y as f32;
            let recovered = shader_lat_from_world_y(y_abs);
            assert!(
                (f64::from(recovered) - lat).abs() < 1e-3,
                "lat {lat}: recovered {recovered}"
            );
        }
    }

    /// Rows of a regular lat-lon grid are equally spaced in latitude, so V
    /// must be linear in latitude and hit half-texel centers at the corner
    /// grid points (bilinear filtering then interpolates between grid
    /// points, never past them).
    #[test]
    fn uv_mapping_hits_half_texel_centers_at_the_grid_corners() {
        let grid = icon_d2();
        let (u, v) = shader_uv(43.18, -3.94, &grid).expect("south-west corner");
        assert!((u - 0.5 / 1215.0).abs() < 1e-6);
        assert!((v - 0.5 / 746.0).abs() < 1e-6);
        let (u, v) = shader_uv(58.08, 20.34, &grid).expect("north-east corner");
        assert!((u - (1.0 - 0.5 / 1215.0)).abs() < 1e-6);
        assert!((v - (1.0 - 0.5 / 746.0)).abs() < 1e-6);

        // Midpoint latitude lands midway between the first and last row.
        let (_, v) = shader_uv((43.18 + 58.08) / 2.0, 10.0, &grid).expect("middle");
        assert!((v - 0.5).abs() < 1e-3);

        // Outside the window there is no sample (coverage 0 in the shader).
        assert_eq!(shader_uv(42.0, 10.0, &grid), None);
        assert_eq!(shader_uv(50.0, 21.0, &grid), None);
    }

    /// The union rect covers both grids — mixed radar/model extents draw as
    /// one quad.
    #[test]
    fn union_world_rect_covers_both_grids() {
        let icon = icon_d2();
        let radar = GridParams {
            lat_min: 46.0,
            lat_max: 55.5,
            lon_min: 3.5,
            lon_max: 16.5,
            ni: 1100,
            nj: 1200,
        };
        let (nw, size) = union_world_rect(&icon, &radar);
        assert_eq!(nw, icon.world_nw(), "icon window contains the radar one");
        assert_eq!(nw + size, icon.world_se());
        let (nw2, size2) = union_world_rect(&radar, &icon);
        assert_eq!((nw2, size2), (nw, size), "union is symmetric");
    }
}

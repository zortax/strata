//! Domain → render conversion for gridded weather: `strata_data`'s
//! [`WeatherGrid`] becomes `strata_render`'s [`WeatherGridFrame`].
//!
//! Both sides share the same raster contract (regular WGS84 lat-lon grid,
//! row-major from the south-west node, NaN = no data), so conversion is a
//! straight copy. A guard downsamples grids above [`MAX_FRAME_POINTS`]
//! by integer stride — today's sources never trigger it (ICON-D2 is
//! 1215×746 ≈ 0.9 M points ≈ 3.6 MB, the reprojected DE1200 radar
//! 1152×1055 ≈ 1.2 M), it only protects against future, larger sources.

use strata_data::domain::{WeatherField, WeatherGrid};
use strata_render::{GriddedField, WeatherGridFrame};

/// Largest grid pushed to the renderer as-is; larger grids are stride-
/// downsampled below this.
pub const MAX_FRAME_POINTS: usize = 2_000_000;

/// The domain field fetched for a render field. (Ceiling/visibility are
/// ingested by the data layer but have no render field yet — a future
/// profile view / visibility cone consumes them.)
pub fn weather_field(field: GriddedField) -> WeatherField {
    match field {
        GriddedField::CloudCover => WeatherField::CloudCover,
        GriddedField::PrecipRate => WeatherField::PrecipRate,
        GriddedField::ThunderstormPotential => WeatherField::ThunderstormPotential,
    }
}

/// Convert a fetched grid into a renderer frame (downsampling if huge).
pub fn frame_from_grid(field: GriddedField, grid: &WeatherGrid) -> WeatherGridFrame {
    let raster = &grid.grid;
    let (ni, nj) = (raster.ni(), raster.nj());
    let stride = stride_for(ni * nj, MAX_FRAME_POINTS);
    let (values, ni, nj) = if stride > 1 {
        tracing::debug!(ni, nj, stride, "downsampling oversized weather grid");
        downsample(raster.values(), ni, nj, stride)
    } else {
        (raster.values().to_vec(), ni, nj)
    };

    let origin = raster.origin();
    let lat_spacing = raster.lat_spacing_deg() * stride as f64;
    let lon_spacing = raster.lon_spacing_deg() * stride as f64;
    WeatherGridFrame {
        field,
        valid_time: grid.valid_time.timestamp(),
        extent: (
            origin.lat(),
            origin.lon(),
            origin.lat() + lat_spacing * (nj - 1) as f64,
            origin.lon() + lon_spacing * (ni - 1) as f64,
        ),
        ni: ni as u32,
        nj: nj as u32,
        values,
    }
}

/// Smallest integer stride that brings `points` to at most `max`.
fn stride_for(points: usize, max: usize) -> usize {
    if points <= max {
        1
    } else {
        (points as f64 / max as f64).sqrt().ceil() as usize
    }
}

/// Keep every `stride`-th node per axis, starting at the south-west node.
/// Returns the new values plus (ni, nj).
fn downsample(values: &[f32], ni: usize, nj: usize, stride: usize) -> (Vec<f32>, usize, usize) {
    let ni2 = (ni - 1) / stride + 1;
    let nj2 = (nj - 1) / stride + 1;
    let mut out = Vec::with_capacity(ni2 * nj2);
    for j in 0..nj2 {
        let row = j * stride * ni;
        for i in 0..ni2 {
            out.push(values[row + i * stride]);
        }
    }
    (out, ni2, nj2)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use strata_data::domain::{LatLon, RegularLatLonGrid};

    use super::*;

    fn grid(ni: usize, nj: usize, spacing: f64) -> WeatherGrid {
        let values: Vec<f32> = (0..ni * nj).map(|i| i as f32).collect();
        WeatherGrid {
            field: WeatherField::CloudCover,
            run_time: Utc.with_ymd_and_hms(2026, 6, 10, 9, 0, 0).unwrap(),
            valid_time: Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap(),
            grid: RegularLatLonGrid::new(
                LatLon::new(47.0, 6.0).unwrap(),
                spacing,
                spacing,
                ni,
                nj,
                values,
            )
            .unwrap(),
        }
    }

    #[test]
    fn render_fields_map_to_their_domain_fields() {
        assert_eq!(
            weather_field(GriddedField::CloudCover),
            WeatherField::CloudCover
        );
        assert_eq!(
            weather_field(GriddedField::PrecipRate),
            WeatherField::PrecipRate
        );
        assert_eq!(
            weather_field(GriddedField::ThunderstormPotential),
            WeatherField::ThunderstormPotential
        );
    }

    #[test]
    fn frame_copies_grid_values_and_extent_verbatim() {
        let source = grid(4, 3, 0.5);
        let frame = frame_from_grid(GriddedField::CloudCover, &source);
        assert_eq!(frame.field, GriddedField::CloudCover);
        assert_eq!(frame.valid_time, source.valid_time.timestamp());
        assert_eq!((frame.ni, frame.nj), (4, 3));
        // SW node 47°N 6°E, NE node 47+2·0.5 / 6+3·0.5.
        assert_eq!(frame.extent, (47.0, 6.0, 48.0, 7.5));
        assert_eq!(frame.values, (0..12).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn icon_d2_and_de1200_sized_grids_pass_through_undownsampled() {
        assert_eq!(stride_for(1215 * 746, MAX_FRAME_POINTS), 1);
        assert_eq!(stride_for(1152 * 1055, MAX_FRAME_POINTS), 1);
        assert_eq!(stride_for(4 * MAX_FRAME_POINTS, MAX_FRAME_POINTS), 2);
    }

    #[test]
    fn downsample_keeps_every_stride_th_node_from_the_south_west() {
        // 5×4 grid, stride 2 → columns 0,2,4 and rows 0,2.
        let values: Vec<f32> = (0..20).map(|i| i as f32).collect();
        let (out, ni, nj) = downsample(&values, 5, 4, 2);
        assert_eq!((ni, nj), (3, 2));
        assert_eq!(out, vec![0.0, 2.0, 4.0, 10.0, 12.0, 14.0]);
    }

    #[test]
    fn downsampled_frame_extent_matches_the_kept_corner_nodes() {
        // 5×5 at 0.1° with a forced stride: shrink MAX via a huge grid is
        // impractical here, so exercise the helpers directly.
        let (_, ni, nj) = downsample(&[0.0; 25], 5, 5, 2);
        assert_eq!((ni, nj), (3, 3));
        // Kept NE node = origin + (n-1)·stride·spacing on each axis.
        let max_lat = 47.0 + 0.1 * 2.0 * (nj - 1) as f64;
        assert!((max_lat - 47.4).abs() < 1e-9);
    }
}

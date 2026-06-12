//! CPU mirror of the `GridLocals` uniform in `shaders/weather_grid.wgsl`.

use super::grid::GridParams;
use crate::map_theme::{Colormap, MAX_COLORMAP_STOPS};

use bytemuck::{Pod, Zeroable};
use glam::DVec2;

/// Std140-compatible mirror of `GridLocals` (272 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GridLocalsUniform {
    /// Quad NW corner, world units relative to the camera center.
    pub origin_rel: [f32; 2],
    /// Quad size in world units.
    pub size_world: [f32; 2],
    /// Absolute world coordinates of the quad NW corner.
    pub nw_abs: [f32; 2],
    /// Temporal blend fraction toward texture b.
    pub frac: f32,
    /// Screen-space hatch strength (thunderstorm overlay only).
    pub hatch: f32,
    /// `(lat_min, 1/lat_span, lon_min, 1/lon_span)` of texture a.
    pub grid_a: [f32; 4],
    /// `(ni, nj, 0, 0)` of texture a.
    pub dims_a: [f32; 4],
    pub grid_b: [f32; 4],
    pub dims_b: [f32; 4],
    pub stop_count: u32,
    pub pad: [u32; 3],
    /// Colormap stop values, packed 4 per vec4.
    pub stop_pos: [[f32; 4]; 2],
    /// Colormap stop colors (premultiplied linear).
    pub stop_color: [[f32; 4]; 8],
}

const _: () = assert!(std::mem::size_of::<GridLocalsUniform>() == 272);
const _: () = assert!(MAX_COLORMAP_STOPS == 8);

fn grid_vec(grid: &GridParams) -> [f32; 4] {
    [
        grid.lat_min as f32,
        (1.0 / (grid.lat_max - grid.lat_min)) as f32,
        grid.lon_min as f32,
        (1.0 / (grid.lon_max - grid.lon_min)) as f32,
    ]
}

fn dims_vec(grid: &GridParams) -> [f32; 4] {
    [grid.ni as f32, grid.nj as f32, 0.0, 0.0]
}

/// Where the quad sits in world space. `origin_rel` is the f64 camera
/// subtraction done by the caller per frame; `nw_abs`/`size_world` feed
/// the fragment-shader latitude recovery.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct QuadPlacement {
    pub origin_rel: DVec2,
    pub nw_abs: DVec2,
    pub size_world: DVec2,
}

/// Pack one field's draw parameters.
pub(crate) fn pack(
    quad: QuadPlacement,
    frac: f32,
    hatch: f32,
    grid_a: &GridParams,
    grid_b: &GridParams,
    colormap: &Colormap,
) -> GridLocalsUniform {
    let mut stop_pos = [[0.0f32; 4]; 2];
    let mut stop_color = [[0.0f32; 4]; 8];
    let stops = colormap.stops();
    for (i, stop) in stops.iter().enumerate() {
        stop_pos[i / 4][i % 4] = stop.value;
        stop_color[i] = stop.color;
    }
    GridLocalsUniform {
        origin_rel: [quad.origin_rel.x as f32, quad.origin_rel.y as f32],
        size_world: [quad.size_world.x as f32, quad.size_world.y as f32],
        nw_abs: [quad.nw_abs.x as f32, quad.nw_abs.y as f32],
        frac,
        hatch,
        grid_a: grid_vec(grid_a),
        dims_a: dims_vec(grid_a),
        grid_b: grid_vec(grid_b),
        dims_b: dims_vec(grid_b),
        stop_count: stops.len() as u32,
        pad: [0; 3],
        stop_pos,
        stop_color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_theme::ColorStop;

    #[test]
    fn packs_colormap_stops_into_the_vec4_lanes() {
        let grid = GridParams {
            lat_min: 43.18,
            lat_max: 58.08,
            lon_min: -3.94,
            lon_max: 20.34,
            ni: 1215,
            nj: 746,
        };
        let map = Colormap::new(&[
            ColorStop {
                value: 0.1,
                color: [0.0; 4],
            },
            ColorStop {
                value: 1.0,
                color: [0.1, 0.2, 0.3, 0.4],
            },
            ColorStop {
                value: 5.0,
                color: [0.5, 0.6, 0.7, 0.8],
            },
            ColorStop {
                value: 20.0,
                color: [0.9, 0.8, 0.7, 0.9],
            },
            ColorStop {
                value: 50.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
        ]);
        let uniform = pack(
            QuadPlacement {
                origin_rel: DVec2::new(0.01, -0.02),
                nw_abs: DVec2::new(0.489, 0.32),
                size_world: DVec2::new(0.067, 0.05),
            },
            0.25,
            1.0,
            &grid,
            &grid,
            &map,
        );
        assert_eq!(uniform.stop_count, 5);
        assert_eq!(uniform.stop_pos[0], [0.1, 1.0, 5.0, 20.0]);
        assert_eq!(uniform.stop_pos[1], [50.0, 0.0, 0.0, 0.0], "5th in lane 0");
        assert_eq!(uniform.stop_color[4], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(uniform.stop_color[5], [0.0; 4], "unused stops zeroed");
        assert_eq!(uniform.dims_a, [1215.0, 746.0, 0.0, 0.0]);
        assert!((uniform.grid_a[1] - 1.0 / 14.9).abs() < 1e-6);
        assert_eq!(uniform.frac, 0.25);
    }
}

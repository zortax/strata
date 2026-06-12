//! Mini elevation sparkline for the profile drawer's collapsed strip
//! (design §3.3: "collapsible to a slim summary strip (mini elevation
//! sparkline + conflict badges)"): the corridor terrain silhouette plus
//! the planned-altitude line in miniature, fed by the same
//! [`ProfileSeries`] the full chart consumes — the documented reuse seam
//! in [`series`](super::series).
//!
//! Geometry is pure (unit-space polylines, unit-tested); the canvas paint
//! closure only scales and replays it.

use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement as _, PathBuilder, Pixels, Styled as _, canvas, point, px,
};
use strata_render::MapTheme;

use super::scene::linear_to_rgba;
use super::series::ProfileSeries;

/// Vertical padding inside the sparkline box, px — keeps the planned line
/// from kissing the strip edges.
const PAD_Y_PX: f32 = 2.0;
/// Planned-line stroke width (the full chart's 2.5 px, miniaturized).
const PLANNED_WIDTH_PX: f32 = 1.5;

/// Sparkline polylines in unit space: `x, y ∈ [0, 1]`, y growing **down**
/// (pixel convention). Terrain comes as contiguous runs — the silhouette
/// gaps where elevation coverage ends, exactly like the full chart.
#[derive(Debug, Clone, PartialEq)]
pub struct SparklineGeometry {
    pub terrain_runs: Vec<Vec<(f32, f32)>>,
    pub planned: Vec<(f32, f32)>,
}

impl SparklineGeometry {
    /// Flattens `series` into unit-space polylines. The altitude range
    /// spans sea level (or the lowest sample below it) to the highest of
    /// terrain/planned with a little headroom, so the miniature keeps the
    /// full chart's sense of proportion.
    pub fn build(series: &ProfileSeries) -> Self {
        let total = series.total_m.max(1.0);
        let altitudes = series
            .terrain
            .iter()
            .filter_map(|&(_, elevation)| elevation)
            .chain(series.planned.iter().map(|&(_, alt)| alt));
        let (mut floor, mut ceil) = (0.0f64, f64::NEG_INFINITY);
        for altitude in altitudes {
            floor = floor.min(altitude);
            ceil = ceil.max(altitude);
        }
        if !ceil.is_finite() {
            return Self {
                terrain_runs: Vec::new(),
                planned: Vec::new(),
            };
        }
        let span = (ceil - floor).max(1.0);
        let ceil = ceil + span * 0.1; // headroom above the highest line
        let span = ceil - floor;

        let map = |along: f64, altitude: f64| -> (f32, f32) {
            (
                (along / total).clamp(0.0, 1.0) as f32,
                (1.0 - (altitude - floor) / span).clamp(0.0, 1.0) as f32,
            )
        };

        let mut terrain_runs: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut run: Vec<(f32, f32)> = Vec::new();
        for &(along, elevation) in &series.terrain {
            match elevation {
                Some(elevation) => run.push(map(along, elevation)),
                None => {
                    if run.len() >= 2 {
                        terrain_runs.push(std::mem::take(&mut run));
                    } else {
                        run.clear();
                    }
                }
            }
        }
        if run.len() >= 2 {
            terrain_runs.push(run);
        }

        let planned = series
            .planned
            .iter()
            .map(|&(along, altitude)| map(along, altitude))
            .collect();

        Self {
            terrain_runs,
            planned,
        }
    }
}

/// Sparkline colors from the active map theme — the same tints the full
/// chart resolves, so the miniature matches it: `(terrain fill, terrain
/// stroke, planned line)`.
pub fn sparkline_colors(map_theme: &MapTheme) -> (Hsla, Hsla, Hsla) {
    let terrain = Hsla::from(linear_to_rgba(map_theme.terrain.light_tint));
    let planned = Hsla::from(linear_to_rgba(map_theme.route.line));
    (
        terrain.opacity(0.25),
        terrain.opacity(0.6),
        planned.opacity(0.9),
    )
}

/// The sparkline element: a passive canvas filling its slot (no listeners
/// — the whole collapsed strip stays one click target).
pub fn sparkline(
    series: &Rc<ProfileSeries>,
    terrain_fill: Hsla,
    terrain_stroke: Hsla,
    planned: Hsla,
) -> AnyElement {
    let geometry = SparklineGeometry::build(series);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _cx| {
            paint(
                &geometry,
                bounds,
                terrain_fill,
                terrain_stroke,
                planned,
                window,
            )
        },
    )
    .size_full()
    .into_any_element()
}

/// Scales the unit-space geometry into `bounds` and paints it: silhouette
/// fills closed down to the strip bottom, terrain top edge stroked, the
/// planned line over everything (the full chart's layer order).
fn paint(
    geometry: &SparklineGeometry,
    bounds: Bounds<Pixels>,
    terrain_fill: Hsla,
    terrain_stroke: Hsla,
    planned: Hsla,
    window: &mut gpui::Window,
) {
    let origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let width = f32::from(bounds.size.width).max(1.0);
    let height = (f32::from(bounds.size.height) - 2.0 * PAD_Y_PX).max(1.0);
    let at = |&(x, y): &(f32, f32)| {
        point(
            px(origin.0 + x * width),
            px(origin.1 + PAD_Y_PX + y * height),
        )
    };
    let bottom = origin.1 + PAD_Y_PX + height;

    for run in &geometry.terrain_runs {
        let (Some(first), Some(last)) = (run.first(), run.last()) else {
            continue;
        };
        // Fill: run, closed down to the strip bottom.
        let mut fill = PathBuilder::fill();
        fill.move_to(at(first));
        for p in &run[1..] {
            fill.line_to(at(p));
        }
        fill.line_to(point(at(last).x, px(bottom)));
        fill.line_to(point(at(first).x, px(bottom)));
        fill.close();
        if let Ok(path) = fill.build() {
            window.paint_path(path, terrain_fill);
        }
        // The terrain's top edge, for definition at this size.
        let mut stroke = PathBuilder::stroke(px(1.0));
        stroke.move_to(at(first));
        for p in &run[1..] {
            stroke.line_to(at(p));
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, terrain_stroke);
        }
    }

    if geometry.planned.len() >= 2 {
        let mut line = PathBuilder::stroke(px(PLANNED_WIDTH_PX));
        line.move_to(at(&geometry.planned[0]));
        for p in &geometry.planned[1..] {
            line.line_to(at(p));
        }
        if let Ok(path) = line.build() {
            window.paint_path(path, planned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare series: terrain with a coverage gap, a climb-cruise planned
    /// line; everything else empty.
    fn series() -> ProfileSeries {
        ProfileSeries {
            total_m: 10_000.0,
            terrain: vec![
                (0.0, Some(100.0)),
                (2_500.0, Some(400.0)),
                (5_000.0, None), // coverage gap splits the silhouette
                (7_500.0, Some(200.0)),
                (10_000.0, Some(100.0)),
            ],
            obstacles: Vec::new(),
            planned: vec![(0.0, 300.0), (4_000.0, 900.0), (10_000.0, 900.0)],
            toc: None,
            tod: None,
            leg_ends_m: vec![10_000.0],
            waypoints: Vec::new(),
            msa_m: vec![None],
            freezing_m: vec![None],
            cloud_base_m: vec![None],
            bands: Vec::new(),
            emphasis: Vec::new(),
            eta: Vec::new(),
        }
    }

    #[test]
    fn geometry_maps_into_unit_space_with_y_down() {
        let geometry = SparklineGeometry::build(&series());

        // The coverage gap splits the terrain into two runs.
        assert_eq!(geometry.terrain_runs.len(), 2);
        assert_eq!(geometry.terrain_runs[0].len(), 2);
        assert_eq!(geometry.terrain_runs[1].len(), 2);

        // Everything lands in the unit square.
        let all = geometry
            .terrain_runs
            .iter()
            .flatten()
            .chain(&geometry.planned);
        for &(x, y) in all {
            assert!((0.0..=1.0).contains(&x), "x = {x}");
            assert!((0.0..=1.0).contains(&y), "y = {y}");
        }

        // X follows along-track distance.
        assert_eq!(geometry.planned[0].0, 0.0);
        assert!((geometry.planned[1].0 - 0.4).abs() < 1e-6);
        assert_eq!(geometry.planned[2].0, 1.0);

        // Y grows down: cruise (900 m) sits *above* (smaller y than) the
        // valley terrain (100 m), and the highest line keeps headroom.
        let cruise_y = geometry.planned[2].1;
        let valley_y = geometry.terrain_runs[0][0].1;
        assert!(cruise_y < valley_y, "cruise above terrain");
        assert!(cruise_y > 0.0, "headroom above the highest line");
    }

    #[test]
    fn short_runs_and_empty_series_yield_no_geometry() {
        let mut empty = series();
        empty.terrain = vec![(0.0, Some(100.0)), (1_000.0, None)]; // 1-point run
        empty.planned.clear();
        let geometry = SparklineGeometry::build(&empty);
        assert!(geometry.terrain_runs.is_empty(), "single points don't draw");

        empty.terrain.clear();
        let geometry = SparklineGeometry::build(&empty);
        assert_eq!(geometry.terrain_runs, Vec::<Vec<(f32, f32)>>::new());
        assert!(geometry.planned.is_empty());
    }
}

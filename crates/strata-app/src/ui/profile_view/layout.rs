//! Stage 2 of the profile paint pipeline: **per-frame pixel layout** —
//! maps the cached [`WorldScene`] through the [`ChartMapping`] of the
//! current bounds into plain, comparable pixel ops. This runs on every
//! frame whose bounds differ (drawer drag-resize, window resize), so it
//! must stay cheap: vertex remapping, tick generation and label-fit
//! decisions only. Text *shaping* never happens here — label widths enter
//! through the `measure` callback (backed by the shaped-text cache), and
//! the ops carry label *content* for stage 3 to resolve.
//!
//! Pure given `measure`, so the remap-equals-rebuild property is testable
//! without a window.

use gpui::{Bounds, Hsla, Pixels, SharedString};

use strata_plan::units::METERS_PER_NAUTICAL_MILE;

use super::mapping::{ChartMapping, FEET_PER_METER, meters_to_nm, nice_ticks};
use super::world::{NORM_SCALE, NormMesh, Palette, WorldScene};

/// Chart gutters around the plot rectangle, in px.
const GUTTER_LEFT: f32 = 54.;
const GUTTER_RIGHT: f32 = 10.;
const GUTTER_TOP: f32 = 24.;
const GUTTER_BOTTOM: f32 = 20.;

/// Planned-line stroke width (the design's "bold line over everything").
const PLANNED_WIDTH: f32 = 2.5;
/// TOC/TOD diamond half-diagonal.
const MARKER_RADIUS: f32 = 5.0;
/// Label line height (shared with stage 3's text paint).
pub(crate) const LABEL_LINE_HEIGHT: f32 = 14.0;

/// Resolves a label's shaped width in px (stage 3's shaped-text cache in
/// production; a fake in tests).
pub(crate) type TextMeasure<'a> = &'a mut dyn FnMut(&SharedString, Hsla) -> f32;

/// One pixel-space op, still declarative (no lyon, no shaping) so layouts
/// are comparable in tests. Stage 3 turns these into paths and shaped
/// lines. The two heavyweight cases avoid per-frame tessellation entirely:
/// `Mesh` carries ready triangles (affine-mapped cache / dash quads),
/// `Stroke` is reserved for the few solid outlines lyon strokes per frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LayoutOp {
    /// Ready triangles in px (flat vertex list, `len % 3 == 0`).
    Mesh { vertices: Vec<(f32, f32)>, color: Hsla },
    /// A filled closed polygon (small per-frame shapes: badges, diamonds).
    Fill { polygon: Vec<(f32, f32)>, color: Hsla },
    /// Solid stroked subpaths sharing one style.
    Stroke {
        subpaths: Vec<Vec<(f32, f32)>>,
        width: f32,
        color: Hsla,
    },
    /// One text line at a px origin (top-left).
    Text {
        text: SharedString,
        color: Hsla,
        origin: (f32, f32),
    },
}

/// Band hit-test data: the band polygon in px plus its mean vertical
/// thickness — stacked bands pick the thinnest (most specific).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BandHit {
    /// Index into [`super::series::ProfileSeries::bands`].
    pub band: usize,
    pub polygon: Vec<(f32, f32)>,
    pub thickness_px: f32,
}

/// The complete per-frame layout: draw ops in z-order plus the hit-test
/// geometry.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PxLayout {
    pub ops: Vec<LayoutOp>,
    /// Planned polyline in px (hit test + scrub marker).
    pub planned_px: Vec<(f32, f32)>,
    pub band_hits: Vec<BandHit>,
}

/// The chart mapping for `world` laid out inside `bounds`.
pub(crate) fn chart_mapping(world: &WorldScene, bounds: Bounds<Pixels>) -> ChartMapping {
    let origin = (
        f32::from(bounds.origin.x) + GUTTER_LEFT,
        f32::from(bounds.origin.y) + GUTTER_TOP,
    );
    let size = (
        (f32::from(bounds.size.width) - GUTTER_LEFT - GUTTER_RIGHT).max(1.0),
        (f32::from(bounds.size.height) - GUTTER_TOP - GUTTER_BOTTOM).max(1.0),
    );
    ChartMapping::new(origin, size, world.total_m, world.floor_m, world.ceil_m)
}

/// Lays out `world` under `mapping`. The whole per-resize-frame cost.
pub(crate) fn layout_world(
    world: &WorldScene,
    mapping: &ChartMapping,
    measure: TextMeasure,
) -> PxLayout {
    let palette = &world.params().palette;
    let mut layout = PxLayout::default();
    grid_and_axes(mapping, palette, &mut layout.ops, measure);
    terrain(world, mapping, palette, &mut layout.ops);
    obstacles(world, mapping, palette, &mut layout.ops);
    bands(world, mapping, &mut layout, measure);
    reference_line(&world.msa, mapping, 1.2, (6.0, 4.0), palette.msa, &mut layout.ops);
    reference_line(
        &world.freezing,
        mapping,
        1.5,
        (2.0, 3.0),
        palette.freezing,
        &mut layout.ops,
    );
    // A longer dash than the freezing line keeps the two weather overlays
    // tellable apart.
    reference_line(
        &world.cloud_base,
        mapping,
        1.5,
        (6.0, 3.0),
        palette.cloud_base,
        &mut layout.ops,
    );
    emphasis(world, mapping, palette, &mut layout.ops);
    layout.planned_px = planned_line(world, mapping, palette, &mut layout.ops);
    waypoint_ticks(world, mapping, palette, &mut layout.ops, measure);
    exaggeration_indicator(mapping, palette, &mut layout.ops);
    layout
}

// ── mesh helpers ────────────────────────────────────────────────────────

/// Maps a cached normalized fill mesh into px under `mapping` — the whole
/// per-frame cost of a fill (fills are affine-safe; strokes are not, their
/// widths would distort).
fn mesh_to_px(mesh: &NormMesh, mapping: &ChartMapping) -> Vec<(f32, f32)> {
    let (ox, oy) = mapping.origin();
    let (w, h) = mapping.size();
    mesh.iter()
        .map(|&(x, y)| (ox + x / NORM_SCALE * w, oy + (1.0 - y / NORM_SCALE) * h))
        .collect()
}

/// Expands a px polyline into dash quads (two triangles per piece) —
/// replaces lyon's measure/sample dash machinery on the per-frame path.
/// Pieces never cross vertices, so corners stay sharp; the pattern phase
/// runs continuously along the line.
fn dash_mesh(points: &[(f32, f32)], width: f32, (on, off): (f32, f32)) -> Vec<(f32, f32)> {
    let period = on + off;
    if points.len() < 2 || on <= 0.0 || off <= 0.0 {
        return Vec::new();
    }
    let half = width / 2.0;
    let mut tris = Vec::new();
    let mut phase = 0.0f32;
    for pair in points.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= f32::EPSILON {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy * half, ux * half);
        let mut s = 0.0f32;
        while s < len {
            let (drawing, remaining) = if phase < on {
                (true, on - phase)
            } else {
                (false, period - phase)
            };
            let step = remaining.min(len - s);
            if step <= 0.0 {
                break; // float underflow guard
            }
            if drawing {
                let (ax, ay) = (x0 + ux * s, y0 + uy * s);
                let (bx, by) = (x0 + ux * (s + step), y0 + uy * (s + step));
                tris.extend_from_slice(&[
                    (ax - nx, ay - ny),
                    (ax + nx, ay + ny),
                    (bx + nx, by + ny),
                    (ax - nx, ay - ny),
                    (bx + nx, by + ny),
                    (bx - nx, by - ny),
                ]);
            }
            s += step;
            phase += step;
            if phase >= period {
                phase -= period;
            }
        }
    }
    tris
}

// ── layers ───────────────────────────────────────────────────────────────

/// Horizontal altitude grid + y tick labels (ft) + x tick labels (NM).
fn grid_and_axes(
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
    measure: TextMeasure,
) {
    let (ox, oy) = mapping.origin();
    let (w, h) = mapping.size();

    // Altitude ticks (labelled in feet).
    let floor_ft = mapping.floor_m() * FEET_PER_METER;
    let ceil_ft = mapping.ceil_m() * FEET_PER_METER;
    let y_ticks = nice_ticks(floor_ft, ceil_ft, (h / 46.0).max(2.0) as usize);
    let grid: Vec<Vec<(f32, f32)>> = y_ticks
        .iter()
        .map(|&ft| {
            let y = mapping.y_at(ft / FEET_PER_METER);
            vec![(ox, y), (ox + w, y)]
        })
        .collect();
    if !grid.is_empty() {
        ops.push(LayoutOp::Stroke {
            subpaths: grid,
            width: 1.0,
            color: palette.grid,
        });
    }
    for (index, &ft) in y_ticks.iter().enumerate() {
        let unit = if index == y_ticks.len() - 1 { " ft" } else { "" };
        let text: SharedString = format!("{ft:.0}{unit}").into();
        let width = measure(&text, palette.axis_text);
        let y = mapping.y_at(ft / FEET_PER_METER);
        ops.push(LayoutOp::Text {
            origin: (ox - 6.0 - width, y - LABEL_LINE_HEIGHT / 2.0),
            text,
            color: palette.axis_text,
        });
    }

    // Distance ticks (NM): small marks on the bottom edge + labels.
    let total_nm = meters_to_nm(mapping.total_m());
    let x_ticks = nice_ticks(0.0, total_nm, (w / 90.0).max(2.0) as usize);
    let marks: Vec<Vec<(f32, f32)>> = x_ticks
        .iter()
        .map(|&nm| {
            let x = mapping.x_at(nm * METERS_PER_NAUTICAL_MILE);
            vec![(x, oy + h), (x, oy + h + 4.0)]
        })
        .collect();
    if !marks.is_empty() {
        ops.push(LayoutOp::Stroke {
            subpaths: marks,
            width: 1.0,
            color: palette.axis_text,
        });
    }
    for (index, &nm) in x_ticks.iter().enumerate() {
        let unit = if index == x_ticks.len() - 1 { " NM" } else { "" };
        let text: SharedString = format!("{nm:.0}{unit}").into();
        let width = measure(&text, palette.axis_text);
        let x = mapping.x_at(nm * METERS_PER_NAUTICAL_MILE) - width / 2.0;
        ops.push(LayoutOp::Text {
            origin: (x, oy + h + 5.0),
            text,
            color: palette.axis_text,
        });
    }
}

/// The corridor max-terrain silhouette: the cached fill mesh per
/// contiguous run (already closed to the chart floor) plus the top stroke.
fn terrain(
    world: &WorldScene,
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
) {
    for (run, mesh) in world.terrain_runs.iter().zip(&world.terrain_meshes) {
        ops.push(LayoutOp::Mesh {
            vertices: mesh_to_px(mesh, mapping),
            color: palette.terrain_fill,
        });
        let points: Vec<(f32, f32)> = run
            .iter()
            .map(|&(along, alt)| (mapping.x_at(along), mapping.y_at(alt)))
            .collect();
        ops.push(LayoutOp::Stroke {
            subpaths: vec![points],
            width: 1.0,
            color: palette.terrain_stroke,
        });
    }
}

/// Obstacle markers: a thin vertical line from the chart floor to the top
/// elevation, with a short tick across the top.
fn obstacles(
    world: &WorldScene,
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
) {
    if world.obstacles.is_empty() {
        return;
    }
    let bottom = mapping.y_at(world.floor_m);
    let mut subpaths = Vec::with_capacity(world.obstacles.len() * 2);
    for &(along, top) in &world.obstacles {
        let x = mapping.x_at(along);
        let y = mapping.y_at(top);
        subpaths.push(vec![(x, bottom), (x, y)]);
        subpaths.push(vec![(x - 3.0, y), (x + 3.0, y)]);
    }
    ops.push(LayoutOp::Stroke {
        subpaths,
        width: 1.0,
        color: palette.obstacle,
    });
}

/// Airspace bands: fill + border per crossing, label when wide enough,
/// conflict badge, hit polygon. Pixel gates (thinness, label fit) live
/// here so they follow the live size.
fn bands(
    world: &WorldScene,
    mapping: &ChartMapping,
    layout: &mut PxLayout,
    measure: TextMeasure,
) {
    let params = world.params();
    let px_per_m = mapping.px_per_alt_m();
    for band in &world.bands {
        let Some(style) = params.band_styles.get(band.series_index) else {
            continue;
        };
        let thickness_px = (band.thickness_m * px_per_m) as f32;
        if thickness_px < 0.5 {
            continue; // empty band (malformed limits clamp to nothing)
        }
        let polygon: Vec<(f32, f32)> = band
            .polygon
            .iter()
            .map(|&(along, alt)| (mapping.x_at(along), mapping.y_at(alt)))
            .collect();
        layout.ops.push(LayoutOp::Mesh {
            vertices: mesh_to_px(&band.mesh, mapping),
            color: style.fill,
        });

        // Border: the full outline, dashed per the map's stroke grammar;
        // conflicted bands switch to the severity tone, slightly heavier.
        let (border_color, border_width) = match band.conflict {
            Some(strata_plan::conflict::ConflictSeverity::Warning) => {
                (params.palette.danger, style.border_width + 0.8)
            }
            Some(_) => (params.palette.warning, style.border_width + 0.8),
            None => (style.border, style.border_width),
        };
        let mut closed = polygon.clone();
        closed.push(polygon[0]);
        match style.dash {
            Some(dash) => layout.ops.push(LayoutOp::Mesh {
                vertices: dash_mesh(&closed, border_width, dash),
                color: border_color,
            }),
            None => layout.ops.push(LayoutOp::Stroke {
                subpaths: vec![closed],
                width: border_width,
                color: border_color,
            }),
        }

        // Label, centered when the block is wide enough.
        let left = mapping.x_at(band.left_m);
        let right = mapping.x_at(band.right_m);
        let center_x = mapping.x_at(band.center_along_m);
        let center_y = mapping.y_at(band.center_alt_m);
        let label_w = measure(&band.label, style.label);
        if right - left > label_w + 16.0 && thickness_px > 15.0 {
            layout.ops.push(LayoutOp::Text {
                text: band.label.clone(),
                color: style.label,
                origin: (center_x - label_w / 2.0, center_y - LABEL_LINE_HEIGHT / 2.0),
            });
        }

        // Penetration badge: a small warning triangle at the band's top
        // center (design §3.3 "band emphasised + badge").
        if band.conflict.is_some() {
            let top_y = mapping.y_at(band.center_top_m);
            let cy = (top_y + 9.0).min(center_y);
            layout.ops.push(LayoutOp::Fill {
                polygon: vec![
                    (center_x, cy - 5.0),
                    (center_x + 5.0, cy + 4.0),
                    (center_x - 5.0, cy + 4.0),
                ],
                color: border_color,
            });
        }

        layout.band_hits.push(BandHit {
            band: band.series_index,
            polygon,
            thickness_px,
        });
    }
}

/// One dashed per-leg reference line (MSA / freezing / cloud base) — dash
/// quads generated arithmetically, one mesh op per line.
fn reference_line(
    segments: &[(f64, f64, f64)],
    mapping: &ChartMapping,
    width: f32,
    dash: (f32, f32),
    color: Hsla,
    ops: &mut Vec<LayoutOp>,
) {
    if segments.is_empty() {
        return;
    }
    let mut vertices = Vec::new();
    for &(start_m, end_m, alt) in segments {
        let y = mapping.y_at(alt);
        vertices.extend(dash_mesh(
            &[(mapping.x_at(start_m), y), (mapping.x_at(end_m), y)],
            width,
            dash,
        ));
    }
    if !vertices.is_empty() {
        ops.push(LayoutOp::Mesh { vertices, color });
    }
}

/// Red clearance emphasis between terrain and the planned line.
fn emphasis(
    world: &WorldScene,
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
) {
    for mesh in &world.emphasis {
        ops.push(LayoutOp::Mesh {
            vertices: mesh_to_px(mesh, mapping),
            color: palette.danger.opacity(0.3),
        });
    }
}

/// The planned-altitude polyline + TOC/TOD diamonds. Returns the px
/// polyline for hit testing.
fn planned_line(
    world: &WorldScene,
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
) -> Vec<(f32, f32)> {
    let points: Vec<(f32, f32)> = world
        .planned
        .iter()
        .map(|&(along, alt)| (mapping.x_at(along), mapping.y_at(alt)))
        .collect();
    if points.len() >= 2 {
        ops.push(LayoutOp::Stroke {
            subpaths: vec![points.clone()],
            width: PLANNED_WIDTH,
            color: palette.planned,
        });
    }
    for &(along, alt) in &world.markers {
        let x = mapping.x_at(along);
        let y = mapping.y_at(alt);
        ops.push(LayoutOp::Fill {
            polygon: vec![
                (x, y - MARKER_RADIUS),
                (x + MARKER_RADIUS, y),
                (x, y + MARKER_RADIUS),
                (x - MARKER_RADIUS, y),
            ],
            color: palette.marker_fill,
        });
    }
    points
}

/// Waypoint tick marks along the top edge with idents (departure /
/// destination / intermediate waypoints). Overlapping idents are skipped
/// left-to-right.
fn waypoint_ticks(
    world: &WorldScene,
    mapping: &ChartMapping,
    palette: &Palette,
    ops: &mut Vec<LayoutOp>,
    measure: TextMeasure,
) {
    let (ox, oy) = mapping.origin();
    let mut subpaths = Vec::with_capacity(world.waypoints.len());
    let mut last_label_end = f32::NEG_INFINITY;
    for (along, ident) in &world.waypoints {
        let x = mapping.x_at(*along);
        subpaths.push(vec![(x, oy - 4.0), (x, oy + 4.0)]);
        let width = measure(ident, palette.axis_text);
        let label_x = (x - width / 2.0).max(ox - GUTTER_LEFT + 2.0);
        if label_x > last_label_end + 6.0 {
            last_label_end = label_x + width;
            ops.push(LayoutOp::Text {
                text: ident.clone(),
                color: palette.axis_text,
                origin: (label_x, oy - 20.0),
            });
        }
    }
    if !subpaths.is_empty() {
        ops.push(LayoutOp::Stroke {
            subpaths,
            width: 1.0,
            color: palette.axis_text,
        });
    }
}

/// The "×12" vertical-exaggeration indicator (design §3.3: the picture
/// must not be misread as true slopes).
fn exaggeration_indicator(mapping: &ChartMapping, palette: &Palette, ops: &mut Vec<LayoutOp>) {
    let factor = mapping.exaggeration();
    let text: SharedString = if factor >= 3.0 {
        format!("×{factor:.0}")
    } else {
        format!("×{factor:.1}")
    }
    .into();
    let (ox, oy) = mapping.origin();
    ops.push(LayoutOp::Text {
        text,
        color: palette.axis_text,
        origin: (ox + 6.0, oy + 4.0),
    });
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::super::world::fixtures::{test_params, test_series};
    use super::super::world::build_world_scene;
    use super::*;

    fn bounds(w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(12.), px(34.)), size(px(w), px(h)))
    }

    /// The production-shaped fake: width proportional to content length
    /// (monospace-ish), independent of color.
    fn fake_measure() -> impl FnMut(&SharedString, Hsla) -> f32 {
        |text: &SharedString, _color: Hsla| text.len() as f32 * 6.0
    }

    /// Remap correctness: laying out a *cached* world scene at any size
    /// must equal a full rebuild (fresh world scene) at that size — no
    /// pixel state may leak into the stage-1 cache. Property-tested over
    /// pseudo-random sizes, including degenerate ones.
    #[test]
    fn remap_equals_full_rebuild_at_any_size() {
        let series = test_series();
        let cached = build_world_scene(&series, test_params(1, series.bands.len()));

        // Deterministic LCG; covers tiny through 4K-ish sizes.
        let mut state: u64 = 0x5DEECE66D;
        let mut next = |range: f32| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / u32::MAX as f32 * 2.0).fract() * range
        };
        for round in 0..200 {
            let b = if round < 4 {
                // Degenerate corners first: zero / sub-gutter sizes.
                bounds(round as f32 * 20.0, round as f32 * 12.0)
            } else {
                bounds(next(3800.0), next(2200.0))
            };
            let mapping = chart_mapping(&cached, b);
            let remapped = layout_world(&cached, &mapping, &mut fake_measure());

            let fresh = build_world_scene(&series, test_params(1, series.bands.len()));
            let rebuilt = layout_world(&fresh, &chart_mapping(&fresh, b), &mut fake_measure());

            assert_eq!(remapped, rebuilt, "diverged at bounds {b:?}");
        }
    }

    #[test]
    fn layout_tracks_the_bounds_exactly() {
        let series = test_series();
        let world = build_world_scene(&series, test_params(1, series.bands.len()));

        for (w, h) in [(900.0_f32, 420.0_f32), (333.0, 188.0), (2400.0, 1200.0)] {
            let b = bounds(w, h);
            let mapping = chart_mapping(&world, b);
            let layout = layout_world(&world, &mapping, &mut fake_measure());

            // The planned line spans the full plot rect: route start on
            // the left edge, route end on the right edge.
            let (ox, oy) = mapping.origin();
            let (pw, ph) = mapping.size();
            assert_eq!(ox, 12.0 + 54.0, "left gutter");
            assert_eq!(pw, w - 54.0 - 10.0, "plot width tracks bounds");
            let first = layout.planned_px.first().copied().expect("planned line");
            let last = layout.planned_px.last().copied().expect("planned line");
            assert_eq!(first.0, ox);
            assert_eq!(last.0, ox + pw);

            // Every op vertex stays inside the padded chart frame (labels
            // may extend into the gutters, geometry must not run away).
            let frame_x = ox - GUTTER_LEFT..=ox + pw + GUTTER_RIGHT;
            let frame_y = oy - GUTTER_TOP..=oy + ph + GUTTER_BOTTOM;
            for op in &layout.ops {
                let points: &[(f32, f32)] = match op {
                    LayoutOp::Mesh { vertices, .. } => vertices,
                    LayoutOp::Fill { polygon, .. } => polygon,
                    LayoutOp::Stroke { subpaths, .. } => {
                        for sub in subpaths {
                            for &(x, y) in sub {
                                assert!(frame_x.contains(&x), "x {x} outside {frame_x:?}");
                                assert!(frame_y.contains(&y), "y {y} outside {frame_y:?}");
                            }
                        }
                        continue;
                    }
                    LayoutOp::Text { .. } => continue,
                };
                for &(x, y) in points {
                    assert!(frame_x.contains(&x), "x {x} outside {frame_x:?}");
                    assert!(frame_y.contains(&y), "y {y} outside {frame_y:?}");
                }
            }

            // Band hits keep the series indices of the surviving bands.
            let hit_bands: Vec<usize> = layout.band_hits.iter().map(|h| h.band).collect();
            assert_eq!(hit_bands, vec![0, 1]);
        }
    }

    #[test]
    fn dash_mesh_covers_the_on_phases_and_respects_corners() {
        // Horizontal line, 10 on / 5 off, width 2: dashes at [0,10),
        // [15,25), [30,40) — quads of two triangles each.
        let tris = dash_mesh(&[(0.0, 100.0), (40.0, 100.0)], 2.0, (10.0, 5.0));
        assert_eq!(tris.len() % 3, 0);
        assert_eq!(tris.len(), 3 * 6, "three dashes, two triangles each");
        let xs: Vec<f32> = tris.iter().map(|p| p.0).collect();
        assert!(xs.iter().all(|&x| (0.0..=40.0).contains(&x)));
        // First dash spans x 0..10, the gap 10..15 stays empty.
        assert!(xs.contains(&10.0));
        assert!(!xs.iter().any(|&x| x > 10.0 && x < 15.0));
        // Width straddles the line symmetrically.
        let ys: Vec<f32> = tris.iter().map(|p| p.1).collect();
        assert!(ys.iter().all(|&y| y == 99.0 || y == 101.0));

        // A dash spanning a corner splits there: total on-coverage equals
        // the polyline length share, and no triangle crosses the corner.
        let corner = dash_mesh(&[(0.0, 0.0), (7.0, 0.0), (7.0, 7.0)], 2.0, (10.0, 4.0));
        assert!(!corner.is_empty());
        for tri in corner.chunks_exact(3) {
            let on_first_leg = tri.iter().all(|p| p.1.abs() <= 1.0);
            let on_second_leg = tri.iter().all(|p| (p.0 - 7.0).abs() <= 1.0);
            assert!(on_first_leg || on_second_leg, "{tri:?}");
        }

        // Degenerate inputs draw nothing (and never loop).
        assert!(dash_mesh(&[(0.0, 0.0)], 2.0, (10.0, 5.0)).is_empty());
        assert!(dash_mesh(&[(0.0, 0.0), (10.0, 0.0)], 2.0, (0.0, 5.0)).is_empty());
        assert!(dash_mesh(&[(5.0, 5.0), (5.0, 5.0)], 2.0, (10.0, 5.0)).is_empty());
    }

    #[test]
    fn label_gates_follow_the_measured_width_and_size() {
        let series = test_series();
        let world = build_world_scene(&series, test_params(1, series.bands.len()));
        let mapping = chart_mapping(&world, bounds(900.0, 420.0));

        let band_label_count = |layout: &PxLayout| {
            layout
                .ops
                .iter()
                .filter(|op| {
                    matches!(op, LayoutOp::Text { text, .. } if text.contains('·'))
                })
                .count()
        };

        // Comfortable widths: both band labels fit.
        let wide = layout_world(&world, &mapping, &mut fake_measure());
        assert_eq!(band_label_count(&wide), 2);

        // Labels wider than the blocks: suppressed, geometry unchanged.
        let mut huge = |_: &SharedString, _: Hsla| -> f32 { 10_000.0 };
        let narrow = layout_world(&world, &mapping, &mut huge);
        assert_eq!(band_label_count(&narrow), 0);
        assert_eq!(narrow.band_hits, wide.band_hits, "hits ignore labels");

        // A tiny chart suppresses them through the thickness gate too.
        let tiny_mapping = chart_mapping(&world, bounds(120.0, 60.0));
        let tiny = layout_world(&world, &tiny_mapping, &mut fake_measure());
        assert_eq!(band_label_count(&tiny), 0);

        // Waypoint idents skip overlapping neighbours left-to-right; the
        // first one always lands (ident labels sit at exactly oy − 20).
        let ident_y = mapping.origin().1 - 20.0;
        let idents: Vec<&str> = wide
            .ops
            .iter()
            .filter_map(|op| match op {
                LayoutOp::Text { text, origin, .. } if origin.1 == ident_y => {
                    Some(text.as_ref())
                }
                _ => None,
            })
            .collect();
        assert!(idents.contains(&"EDFE"), "{idents:?}");
    }
}

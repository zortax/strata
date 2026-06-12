//! Stage 3 of the profile paint pipeline: **realizing and replaying** the
//! per-frame [`PxLayout`] — ready meshes become paths directly (no
//! per-frame tessellation), the few solid outlines stroke through lyon,
//! and label content binds to shaped lines from the content-keyed
//! [`ShapedTextCache`]; the result replays each paint. Shaping is the
//! only expensive text step and happens at most once per distinct label;
//! a resize frame merely *repositions* the cached shaped runs
//! (`ShapedLine::paint` takes the origin per call — no re-shape).
//!
//! Pure geometry helpers (point-in-polygon, polyline distance, the band
//! pick) live as free functions so the hit-testing is unit-testable
//! without a window.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, Hsla, Path, PathBuilder, Pixels, Point, Rgba, SharedString, ShapedLine,
    TextAlign, TextRun, Window, point, px,
};
use strata_render::layers::style::label_color_from_border;

use super::layout::{BandHit, LABEL_LINE_HEIGHT, LayoutOp, PxLayout};
use super::mapping::{ChartMapping, point_segment_distance};

/// Hover slop around the planned line for the drag hit test.
pub(crate) const LINE_HIT_SLOP_PX: f32 = 6.0;
/// Axis / label font size.
const LABEL_FONT_PX: f32 = 10.0;

/// One deferred paint operation (replayed in order each frame). Shaped
/// lines are shared with the text cache (`Rc`): a `ShapedLine` inlines its
/// decoration runs and would dominate the enum size.
pub(crate) enum PaintOp {
    FillOrStroke(Path<Pixels>, Hsla),
    Text(Rc<ShapedLine>, Point<Pixels>),
}

/// What the cursor is over (hover/drag/click dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoverTarget {
    PlannedLine,
    Band(usize),
    Chart,
    Outside,
}

/// The realized pixel scene of one frame's bounds, replayed until the
/// bounds (→ remap) or the world inputs (→ rebuild) change.
pub(crate) struct Scene {
    bounds: Bounds<Pixels>,
    pub mapping: ChartMapping,
    pub ops: Vec<PaintOp>,
    /// Planned polyline in px (hit test + scrub marker).
    pub planned_px: Vec<(f32, f32)>,
    pub band_hits: Vec<BandHit>,
}

impl Scene {
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    /// The hover target under a window pixel.
    pub fn hover_target(&self, pos: (f32, f32)) -> HoverTarget {
        if !self.mapping.contains(pos.0, pos.1) {
            return HoverTarget::Outside;
        }
        if polyline_distance(&self.planned_px, pos) <= LINE_HIT_SLOP_PX {
            return HoverTarget::PlannedLine;
        }
        match pick_band(&self.band_hits, pos) {
            Some(band) => HoverTarget::Band(band),
            None => HoverTarget::Chart,
        }
    }

    /// Pixel y of the planned line at pixel x (the scrub marker rides it).
    pub fn planned_y_at(&self, x: f32) -> Option<f32> {
        sample_polyline_px(&self.planned_px, x)
    }
}

/// The canvas prepaint's per-frame choice (see [`rebuild_decision`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildDecision {
    /// The inputs changed (compute generation, theme, weather toggles):
    /// rebuild the world scene, then remap.
    BuildWorld,
    /// World scene still valid but the bounds moved (drawer drag-resize,
    /// window resize): remap the cached world geometry **this frame** —
    /// cheap by design, so the chart tracks the panel edge per frame.
    Remap,
    /// Cache hit, nothing to do.
    Valid,
}

/// Pure cache decision for the canvas prepaint: the world scene is keyed
/// by params only (never by size); the realized scene additionally by the
/// bounds it was laid out for.
pub(crate) fn rebuild_decision(
    world_params_match: bool,
    scene_bounds: Option<Bounds<Pixels>>,
    bounds: Bounds<Pixels>,
) -> RebuildDecision {
    if !world_params_match {
        return RebuildDecision::BuildWorld;
    }
    if scene_bounds == Some(bounds) {
        RebuildDecision::Valid
    } else {
        RebuildDecision::Remap
    }
}

// ── shaped-text cache ───────────────────────────────────────────────────

/// Shaped label lines keyed by content + color, shared across frames:
/// resize frames reuse the shaped runs and only reposition them. Cleared
/// whenever the world scene rebuilds (new generation / palette).
#[derive(Default)]
pub(crate) struct ShapedTextCache {
    entries: HashMap<(SharedString, Hsla), Rc<ShapedLine>>,
}

impl ShapedTextCache {
    /// Safety valve: labels churn slightly during a resize (tick sets, the
    /// exaggeration factor), so cap the cache instead of letting a long
    /// interactive resize grow it without bound.
    const MAX_ENTRIES: usize = 1024;

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The shaped line for `text` in `color`, shaping it on first sight.
    pub fn get_or_shape(
        &mut self,
        text: &SharedString,
        color: Hsla,
        window: &mut Window,
    ) -> Rc<ShapedLine> {
        let key = (text.clone(), color);
        if let Some(line) = self.entries.get(&key) {
            return Rc::clone(line);
        }
        if self.entries.len() >= Self::MAX_ENTRIES {
            self.entries.clear();
        }
        let line = Rc::new(shape(text.clone(), color, window));
        self.entries.insert(key, Rc::clone(&line));
        line
    }

    /// Shaped width in px (the layout stage's `measure` callback).
    pub fn width(&mut self, text: &SharedString, color: Hsla, window: &mut Window) -> f32 {
        f32::from(self.get_or_shape(text, color, window).width())
    }
}

// ── realize + paint ─────────────────────────────────────────────────────

/// Turns a per-frame layout into the replayable scene: tessellates the
/// fill/stroke ops, binds text ops to cached shaped lines.
pub(crate) fn realize_scene(
    layout: PxLayout,
    mapping: ChartMapping,
    bounds: Bounds<Pixels>,
    text_cache: &mut ShapedTextCache,
    window: &mut Window,
) -> Scene {
    let mut ops = Vec::with_capacity(layout.ops.len());
    for op in layout.ops {
        match op {
            LayoutOp::Mesh { vertices, color } => {
                if let Some(path) = mesh_path(&vertices) {
                    ops.push(PaintOp::FillOrStroke(path, color));
                }
            }
            LayoutOp::Fill { polygon, color } => {
                if let Some(path) = fill_polygon(&polygon) {
                    ops.push(PaintOp::FillOrStroke(path, color));
                }
            }
            LayoutOp::Stroke {
                subpaths,
                width,
                color,
            } => {
                if let Some(path) = stroke_subpaths(&subpaths, width) {
                    ops.push(PaintOp::FillOrStroke(path, color));
                }
            }
            LayoutOp::Text {
                text,
                color,
                origin,
            } => {
                let line = text_cache.get_or_shape(&text, color, window);
                ops.push(PaintOp::Text(line, point(px(origin.0), px(origin.1))));
            }
        }
    }
    Scene {
        bounds,
        mapping,
        ops,
        planned_px: layout.planned_px,
        band_hits: layout.band_hits,
    }
}

/// Replays the realized scene (the whole per-frame static cost).
pub(crate) fn paint_scene(scene: &Scene, window: &mut Window, cx: &mut App) {
    for op in &scene.ops {
        match op {
            PaintOp::FillOrStroke(path, color) => window.paint_path(path.clone(), *color),
            PaintOp::Text(line, origin) => {
                line.paint(
                    *origin,
                    px(LABEL_LINE_HEIGHT),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
        }
    }
}

/// Shapes one single-style text line with the window's UI font.
fn shape(text: SharedString, color: Hsla, window: &mut Window) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, px(LABEL_FONT_PX), &[run], None)
}

fn fill_polygon(points: &[(f32, f32)]) -> Option<Path<Pixels>> {
    if points.len() < 3 {
        return None;
    }
    let mut builder = PathBuilder::fill();
    builder.move_to(point(px(points[0].0), px(points[0].1)));
    for &(x, y) in &points[1..] {
        builder.line_to(point(px(x), px(y)));
    }
    builder.close();
    builder.build().ok()
}

/// A ready triangle list (the cached-fill / dash-quad fast path) as a
/// paintable path — built exactly like gpui's own `build_path`, so it
/// renders identically to a lyon-tessellated fill.
fn mesh_path(vertices: &[(f32, f32)]) -> Option<Path<Pixels>> {
    if vertices.len() < 3 {
        return None;
    }
    let mut path = Path::new(point(px(vertices[0].0), px(vertices[0].1)));
    let st = (point(0., 1.), point(0., 1.), point(0., 1.));
    for tri in vertices.chunks_exact(3) {
        path.push_triangle(
            (
                point(px(tri[0].0), px(tri[0].1)),
                point(px(tri[1].0), px(tri[1].1)),
                point(px(tri[2].0), px(tri[2].1)),
            ),
            st,
        );
    }
    Some(path)
}

fn stroke_subpaths(subpaths: &[Vec<(f32, f32)>], width: f32) -> Option<Path<Pixels>> {
    let mut builder = PathBuilder::stroke(px(width));
    let mut any = false;
    for subpath in subpaths {
        if subpath.len() < 2 {
            continue;
        }
        builder.move_to(point(px(subpath[0].0), px(subpath[0].1)));
        for &(x, y) in &subpath[1..] {
            builder.line_to(point(px(x), px(y)));
        }
        any = true;
    }
    if !any {
        return None;
    }
    builder.build().ok()
}

/// Shapes a one-off label for the dynamic overlays (drag preview).
pub(crate) fn shape_overlay_label(
    text: impl Into<SharedString>,
    color: Hsla,
    window: &mut Window,
) -> ShapedLine {
    shape(text.into(), color, window)
}

/// Paints a shaped overlay label.
pub(crate) fn paint_overlay_label(
    line: &ShapedLine,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    line.paint(origin, px(LABEL_LINE_HEIGHT), TextAlign::Left, None, window, cx)
        .ok();
}

// ── pure hit-test helpers ───────────────────────────────────────────────

/// Minimum distance from `pos` to the polyline.
pub(crate) fn polyline_distance(points: &[(f32, f32)], pos: (f32, f32)) -> f32 {
    if points.len() < 2 {
        return f32::INFINITY;
    }
    points
        .windows(2)
        .map(|pair| point_segment_distance(pos, pair[0], pair[1]))
        .fold(f32::INFINITY, f32::min)
}

/// Even-odd point-in-polygon test.
pub(crate) fn point_in_polygon(polygon: &[(f32, f32)], pos: (f32, f32)) -> bool {
    let mut inside = false;
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > pos.1) != (yj > pos.1) {
            let cross = (xj - xi) * (pos.1 - yi) / (yj - yi) + xi;
            if pos.0 < cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// The most specific (thinnest) band containing `pos`.
pub(crate) fn pick_band(bands: &[BandHit], pos: (f32, f32)) -> Option<usize> {
    bands
        .iter()
        .filter(|hit| point_in_polygon(&hit.polygon, pos))
        .min_by(|a, b| a.thickness_px.total_cmp(&b.thickness_px))
        .map(|hit| hit.band)
}

/// Linear y of an ascending-x px polyline at `x` (clamped to the ends).
fn sample_polyline_px(points: &[(f32, f32)], x: f32) -> Option<f32> {
    let first = points.first()?;
    if x <= first.0 {
        return Some(first.1);
    }
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if x <= x1 {
            if x1 - x0 <= f32::EPSILON {
                return Some(y1);
            }
            return Some(y0 + (y1 - y0) * (x - x0) / (x1 - x0));
        }
    }
    points.last().map(|&(_, y)| y)
}

/// Premultiplied **linear** RGBA (the map style colors) → a gpui sRGB
/// color with the straight alpha preserved — the band fills must keep
/// their translucency so stacked volumes read (unlike
/// [`crate::ui::chip_color`], which forces opacity for legend chips).
pub(crate) fn linear_to_rgba(premultiplied: [f32; 4]) -> Rgba {
    let [r, g, b, a] = premultiplied;
    if a <= f32::EPSILON {
        return Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.,
        };
    }
    let encode = |linear: f32| {
        let v = (linear / a).clamp(0.0, 1.0);
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    Rgba {
        r: encode(r),
        g: encode(g),
        b: encode(b),
        a,
    }
}

/// Band label color from the border (the map's label convention).
pub(crate) fn band_label_color(border: [f32; 4]) -> Rgba {
    linear_to_rgba(label_color_from_border(border))
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    #[test]
    fn world_cache_invalidates_only_on_param_changes() {
        use RebuildDecision::*;
        let bounds = |w: f32, h: f32| {
            Bounds::new(gpui::point(px(0.), px(0.)), size(px(w), px(h)))
        };
        let settled = bounds(800., 300.);
        let moving = bounds(800., 320.);

        // Params changed (new compute generation / theme / toggle): the
        // world scene rebuilds — regardless of bounds.
        assert_eq!(rebuild_decision(false, None, settled), BuildWorld);
        assert_eq!(rebuild_decision(false, Some(settled), settled), BuildWorld);

        // Cache hit.
        assert_eq!(rebuild_decision(true, Some(settled), settled), Valid);

        // A bounds-only change NEVER rebuilds the world scene — it remaps
        // the cached geometry the same frame (per-frame resize tracking).
        assert_eq!(rebuild_decision(true, Some(settled), moving), Remap);
        assert_eq!(rebuild_decision(true, None, settled), Remap);
    }

    #[test]
    fn point_in_polygon_handles_sloped_quads() {
        // A band polygon with a sloped floor (the AGL case).
        let polygon = [
            (0.0, 10.0),
            (100.0, 10.0), // flat ceiling
            (100.0, 60.0),
            (0.0, 90.0), // floor slopes up to the right
        ];
        assert!(point_in_polygon(&polygon, (50.0, 40.0)));
        assert!(point_in_polygon(&polygon, (10.0, 80.0)));
        // Below the sloped floor on the left, but inside on the right edge
        // height — the slope must matter.
        assert!(!point_in_polygon(&polygon, (10.0, 95.0)));
        assert!(point_in_polygon(&polygon, (90.0, 62.0)));
        assert!(!point_in_polygon(&polygon, (-5.0, 40.0)));
        assert!(!point_in_polygon(&polygon, (50.0, 5.0)));
        assert!(!point_in_polygon(&[(0.0, 0.0), (1.0, 1.0)], (0.5, 0.5)));
    }

    #[test]
    fn band_pick_prefers_the_thinnest_containing_band() {
        let outer = BandHit {
            band: 0,
            polygon: vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            thickness_px: 100.0,
        };
        let inner = BandHit {
            band: 1,
            polygon: vec![(20.0, 40.0), (80.0, 40.0), (80.0, 60.0), (20.0, 60.0)],
            thickness_px: 20.0,
        };
        let bands = [outer, inner];
        // Inside both → the thinner (more specific) volume wins.
        assert_eq!(pick_band(&bands, (50.0, 50.0)), Some(1));
        // Inside only the outer.
        assert_eq!(pick_band(&bands, (50.0, 10.0)), Some(0));
        assert_eq!(pick_band(&bands, (150.0, 50.0)), None);
    }

    #[test]
    fn polyline_distance_finds_the_nearest_segment() {
        let line = [(0.0, 100.0), (50.0, 100.0), (100.0, 50.0)];
        assert_eq!(polyline_distance(&line, (25.0, 104.0)), 4.0);
        assert!(polyline_distance(&line, (100.0, 50.0)) < 1e-6);
        assert_eq!(polyline_distance(&[(0.0, 0.0)], (5.0, 5.0)), f32::INFINITY);
    }

    #[test]
    fn planned_y_interpolates_in_pixel_space() {
        assert_eq!(sample_polyline_px(&[(0.0, 100.0), (10.0, 0.0)], 5.0), Some(50.0));
        assert_eq!(sample_polyline_px(&[(0.0, 100.0), (10.0, 0.0)], -5.0), Some(100.0));
        assert_eq!(sample_polyline_px(&[(0.0, 100.0), (10.0, 0.0)], 50.0), Some(0.0));
        assert_eq!(sample_polyline_px(&[], 5.0), None);
    }

    #[test]
    fn linear_color_round_trips_and_keeps_alpha() {
        // srgb(255, 0, 0, 0.5) premultiplied-linear is (0.5, 0, 0, 0.5).
        let c = linear_to_rgba([0.5, 0.0, 0.0, 0.5]);
        assert!((c.r - 1.0).abs() < 1e-3, "r = {}", c.r);
        assert!(c.g.abs() < 1e-3);
        assert_eq!(c.a, 0.5, "translucency preserved");
        let transparent = linear_to_rgba([0.0; 4]);
        assert_eq!(transparent.a, 0.0);
    }
}

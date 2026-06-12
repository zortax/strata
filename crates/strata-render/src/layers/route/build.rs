//! Worker-side geometry build for the route layer: stroke tessellation of
//! the corridor / legs / alternate links into [`RouteLineVertex`] buffers
//! (the style fields `route_line.wgsl` consumes) and the marker instances.
//!
//! Same precision discipline as [`crate::layers::tess`]: vertices are world
//! units relative to the dataset origin (f64 subtraction on the worker),
//! lyon runs on `TESS_SCALE`-scaled local coordinates.

use crate::features::{RenderRoute, RouteVertex};
use crate::geo::lat_lon_from_world;
use crate::layers::route::path;
use crate::layers::route::symbols::RouteSymbolKey;
use crate::layers::style::priority;
use crate::layers::symbols::SymbolInstance;
use crate::layers::tess::TESS_SCALE;
use crate::map_theme::RouteTheme;
use crate::text::{LabelAnchor, LabelPlacement, LabelRequest};

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, Vec2};
use lyon::math::point;
use lyon::path::{Path, Side};
use lyon::tessellation::{
    BuffersBuilder, StrokeOptions, StrokeTessellator, StrokeVertex as LyonStrokeVertex,
    VertexBuffers,
};

use std::ops::Range;

/// Route polyline width in logical px (screen-stable).
pub const ROUTE_LINE_WIDTH_PX: f32 = 3.5;
/// Alternate-link width in logical px.
pub const ALTERNATE_LINE_WIDTH_PX: f32 = 2.0;
/// Alternate-link dash pattern (on, off) in logical px.
pub const ALTERNATE_DASH_PX: (f32, f32) = (7.0, 5.0);

/// Leg labels (MH · GS · alt) are hidden below this camera zoom — at a
/// typical whole-route view they would only collide with each other; once
/// a leg spans a good part of the screen there is room for its numbers.
pub const LEG_LABEL_MIN_ZOOM: f32 = 8.0;
/// Leg label font size in logical px (between airport idents at 11 and the
/// small feature labels at 10 — but lower priority than both anchors).
const LEG_LABEL_SIZE_PX: f32 = 10.0;
/// Perpendicular screen offset of a leg label's center off the polyline
/// (logical px): clears half the stroke, the chevrons and half the shaped
/// text height with a small gap.
pub const LEG_LABEL_OFFSET_PX: f32 = 13.0;
/// Namespace bit for route-leg label ids (points/airspace/weather layers
/// use bits 61–63).
const LABEL_ID_NAMESPACE: u64 = 1 << 60;

/// Equatorial Earth circumference in meters — the Web-Mercator scale base
/// (1 world unit of the normalized square = this many ground meters at the
/// equator, shrinking with `cos(lat)`).
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;

/// Vertex of a route stroke (`route_line.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RouteLineVertex {
    /// Centerline position, world units relative to the dataset origin.
    pub pos: [f32; 2],
    /// Extrusion direction (lyon stroke normal; unit-ish, longer at miters).
    pub normal: [f32; 2],
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
    /// Screen-stable width component in logical px.
    pub width_px: f32,
    /// Ground-fixed width component in world units (the corridor).
    pub width_world: f32,
    /// Arc length along the stroke in world units.
    pub along_world: f32,
    /// Side of the centerline: −1 or +1 (interpolates to 0 at the center).
    pub side: f32,
    /// Dash pattern (on, off) in logical px; (0, 0) = solid.
    pub dash_px: [f32; 2],
    /// 1.0 = draw direction chevrons; 0.0 = plain stroke.
    pub ticks: f32,
}

impl RouteLineVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x4,
        3 => Float32,
        4 => Float32,
        5 => Float32,
        6 => Float32,
        7 => Float32x2,
        8 => Float32,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// CPU-side accumulated route stroke geometry.
#[derive(Debug, Clone, Default)]
pub struct RouteLineMesh {
    pub vertices: Vec<RouteLineVertex>,
    pub indices: Vec<u32>,
}

/// Style of one route stroke.
#[derive(Debug, Clone, Copy)]
struct StrokeSpec {
    color: [f32; 4],
    width_px: f32,
    /// Ground-fixed half-width in meters (the corridor); converted to world
    /// units per vertex from the local Mercator scale.
    halfwidth_m: Option<f64>,
    dash_px: Option<(f32, f32)>,
    ticks: bool,
}

/// One instanced draw of route markers sharing a mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteBatch {
    pub key: RouteSymbolKey,
    pub instances: Range<u32>,
}

/// One retained marker: mesh key, the app-side vertex id for handles
/// (`None` for TOC/TOD — they are positions, not route points), and the
/// resting instance data. The id is what the hover highlight correlates
/// against at assembly time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaseInstance {
    pub key: RouteSymbolKey,
    pub id: Option<u64>,
    pub instance: SymbolInstance,
}

/// Worker build output, retained CPU-side so a scrub move can reposition
/// its marker without re-tessellating anything.
pub struct Artifacts {
    pub origin_world: DVec2,
    pub lines: RouteLineMesh,
    /// Marker instances in batch order, excluding the scrub marker.
    pub base_instances: Vec<BaseInstance>,
    /// Per-leg labels for the text system (midpoint-anchored, offset off
    /// the line); the layer re-queues them every frame.
    pub labels: Vec<LabelRequest>,
    /// Projected main track, parallel to [`cum_m`](Self::cum_m).
    pub track_world: Vec<DVec2>,
    /// Cumulative geodesic meters along the main track.
    pub cum_m: Vec<f64>,
}

/// Pure worker job: project, tessellate strokes and build the marker
/// instances for one route.
pub fn build_artifacts(route: &RenderRoute, theme: &RouteTheme) -> Artifacts {
    let main: Vec<&RouteVertex> = route
        .points
        .iter()
        .filter(|p| !p.kind.is_alternate())
        .collect();
    let alternates: Vec<&RouteVertex> = route
        .points
        .iter()
        .filter(|p| p.kind.is_alternate())
        .collect();

    let track_pos: Vec<[f64; 2]> = main.iter().map(|p| p.pos).collect();
    let track_world: Vec<DVec2> = track_pos.iter().map(|&p| path::world_from_pos(p)).collect();
    let cum_m = path::cumulative_m(&track_pos);

    let all_world: Vec<DVec2> = route
        .points
        .iter()
        .map(|p| path::world_from_pos(p.pos))
        .collect();
    let origin_world = bbox_center(&all_world);

    if route.leg_conflict.len() > track_world.len().saturating_sub(1) {
        tracing::warn!(
            legs = track_world.len().saturating_sub(1),
            conflicts = route.leg_conflict.len(),
            "leg_conflict longer than the main track; extra entries ignored"
        );
    }

    // Strokes, bottom to top: corridor → legs (conflict tint per leg) →
    // alternate links. Draw order inside one mesh is append order.
    let mut lines = RouteLineMesh::default();
    if let Some(halfwidth_m) = route.corridor_halfwidth_m
        && halfwidth_m > 0.0
        && track_world.len() >= 2
    {
        tessellate_stroke(
            &track_world,
            origin_world,
            StrokeSpec {
                color: theme.corridor,
                width_px: 0.0,
                halfwidth_m: Some(halfwidth_m),
                dash_px: None,
                ticks: false,
            },
            &mut lines,
        );
    }
    for (i, leg) in track_world.windows(2).enumerate() {
        let conflict = route.leg_conflict.get(i).copied().unwrap_or(false);
        tessellate_stroke(
            leg,
            origin_world,
            StrokeSpec {
                color: if conflict {
                    theme.line_conflict
                } else {
                    theme.line
                },
                width_px: ROUTE_LINE_WIDTH_PX,
                halfwidth_m: None,
                dash_px: None,
                ticks: true,
            },
            &mut lines,
        );
    }
    if let Some(&destination) = track_world.last() {
        for alternate in &alternates {
            let link = [destination, path::world_from_pos(alternate.pos)];
            tessellate_stroke(
                &link,
                origin_world,
                StrokeSpec {
                    color: theme.line,
                    width_px: ALTERNATE_LINE_WIDTH_PX,
                    halfwidth_m: None,
                    dash_px: Some(ALTERNATE_DASH_PX),
                    ticks: false,
                },
                &mut lines,
            );
        }
    }

    // Marker instances, grouped by mesh key so batches stay contiguous:
    // handles in point order per kind, then TOC, then TOD (the scrub marker
    // is appended at assembly time).
    let mut base_instances = Vec::new();
    for key in [
        RouteSymbolKey::Departure,
        RouteSymbolKey::Waypoint,
        RouteSymbolKey::Destination,
        RouteSymbolKey::Alternate,
    ] {
        for point in &route.points {
            if handle_key(point) == key {
                let world = path::world_from_pos(point.pos);
                base_instances.push(BaseInstance {
                    key,
                    id: Some(point.id),
                    instance: instance(key, world, origin_world),
                });
            }
        }
    }
    for (key, marker) in [
        (RouteSymbolKey::Toc, route.toc),
        (RouteSymbolKey::Tod, route.tod),
    ] {
        if let Some((along_m, pos)) = marker {
            // Interpolate onto the drawn polyline; the explicit position is
            // the fallback for degenerate tracks.
            let world = path::point_at(&track_world, &cum_m, along_m)
                .unwrap_or_else(|| path::world_from_pos(pos));
            base_instances.push(BaseInstance {
                key,
                id: None,
                instance: instance(key, world, origin_world),
            });
        }
    }

    let labels = leg_labels(&track_world, &route.leg_labels, theme);

    Artifacts {
        origin_world,
        lines,
        base_instances,
        labels,
        track_world,
        cum_m,
    }
}

/// One text-system request per labelled leg: anchored at the drawn leg's
/// world midpoint, pushed perpendicularly off the line in screen px (the
/// world direction *is* the screen direction — y grows south/down), colored
/// with the theme's route accent. Blank/missing entries and zero-length
/// legs produce nothing; entries beyond the track's legs are ignored (the
/// stale-compute window — same contract as `leg_conflict`).
fn leg_labels(
    track_world: &[DVec2],
    texts: &[Option<String>],
    theme: &RouteTheme,
) -> Vec<LabelRequest> {
    let mut labels = Vec::new();
    for (index, leg) in track_world.windows(2).enumerate() {
        let Some(Some(text)) = texts.get(index) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let Some(offset_px) = leg_label_offset(leg[0], leg[1]) else {
            continue; // zero-length leg: no direction to hang off
        };
        labels.push(LabelRequest {
            text: text.into(),
            anchor: LabelAnchor::World((leg[0] + leg[1]) * 0.5),
            offset_px,
            placement: LabelPlacement::Center,
            size_px: LEG_LABEL_SIZE_PX,
            color: theme.line,
            priority: priority::ROUTE_LEG,
            min_zoom: LEG_LABEL_MIN_ZOOM,
            id: LABEL_ID_NAMESPACE | index as u64,
        });
    }
    labels
}

/// Screen offset (logical px) perpendicular to the leg `a → b`, on the
/// upper screen side so the label reads "above" the line; a perfectly
/// vertical leg labels to the right. `None` for zero-length legs.
fn leg_label_offset(a: DVec2, b: DVec2) -> Option<Vec2> {
    let direction = b - a;
    let length = direction.length();
    if length <= f64::EPSILON || !length.is_finite() {
        return None;
    }
    let mut perp = DVec2::new(direction.y, -direction.x) / length;
    if perp.y > 0.0 || (perp.y == 0.0 && perp.x < 0.0) {
        perp = -perp;
    }
    Some((perp * f64::from(LEG_LABEL_OFFSET_PX)).as_vec2())
}

/// Final instance buffer + batches for one frame: the retained base
/// instances — the hover-highlighted handle enlarged in place — plus the
/// scrub marker interpolated onto the track. Cheap — this is what a scrub
/// or highlight change re-runs instead of a worker rebuild.
pub fn assemble_instances(
    artifacts: &Artifacts,
    scrub_along_m: Option<f64>,
    highlight: Option<u64>,
) -> (Vec<SymbolInstance>, Vec<RouteBatch>) {
    let mut instances: Vec<SymbolInstance> = Vec::with_capacity(artifacts.base_instances.len() + 1);
    let mut batches: Vec<RouteBatch> = Vec::new();
    let mut push =
        |key: RouteSymbolKey, instance: SymbolInstance, batches: &mut Vec<RouteBatch>| {
            let index = instances.len() as u32;
            instances.push(instance);
            match batches.last_mut() {
                Some(batch) if batch.key == key => batch.instances.end = index + 1,
                _ => batches.push(RouteBatch {
                    key,
                    instances: index..index + 1,
                }),
            }
        };
    for base in &artifacts.base_instances {
        let mut inst = base.instance;
        if base.id.is_some() && base.id == highlight {
            // The emphasis is a per-instance size — the mesh stays shared.
            inst.size_px *= super::symbols::HIGHLIGHT_SCALE;
        }
        push(base.key, inst, &mut batches);
    }
    if let Some(along_m) = scrub_along_m
        && let Some(world) = path::point_at(&artifacts.track_world, &artifacts.cum_m, along_m)
    {
        push(
            RouteSymbolKey::Scrub,
            instance(RouteSymbolKey::Scrub, world, artifacts.origin_world),
            &mut batches,
        );
    }
    (instances, batches)
}

/// The handle mesh for one route point (also the layer's hook for sizing
/// the hover-highlight glow ring to the point's handle).
pub(super) fn handle_key(point: &RouteVertex) -> RouteSymbolKey {
    use crate::features::RoutePointKind;
    match point.kind {
        RoutePointKind::Departure => RouteSymbolKey::Departure,
        RoutePointKind::Waypoint => RouteSymbolKey::Waypoint,
        RoutePointKind::Destination => RouteSymbolKey::Destination,
        RoutePointKind::Alternate => RouteSymbolKey::Alternate,
    }
}

fn instance(key: RouteSymbolKey, world: DVec2, origin: DVec2) -> SymbolInstance {
    let local = world - origin; // f64, keeps deep zoom crisp
    SymbolInstance {
        anchor_local: [local.x as f32, local.y as f32],
        offset_px: [0.0, 0.0],
        size_px: key.size_px(),
        rotation_rad: 0.0,
        // Mesh colors are already themed; no per-instance tint.
        color_mul: [1.0, 1.0, 1.0, 1.0],
    }
}

fn bbox_center(points: &[DVec2]) -> DVec2 {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    if points.is_empty() {
        DVec2::ZERO
    } else {
        (min + max) / 2.0
    }
}

/// Stroke an open polyline (world coordinates) into `out`, relative to
/// `origin`. The ground-fixed width component is derived per vertex from
/// the local Mercator scale at the vertex's latitude.
fn tessellate_stroke(track: &[DVec2], origin: DVec2, spec: StrokeSpec, out: &mut RouteLineMesh) {
    if track.len() < 2 {
        return;
    }
    let mut builder = Path::builder();
    let mut points = track.iter().map(|&w| to_local(w, origin));
    let Some(first) = points.next() else {
        return;
    };
    builder.begin(first);
    for p in points {
        builder.line_to(p);
    }
    builder.end(false);
    let built = builder.build();

    let dash_px = spec.dash_px.map_or([0.0, 0.0], |(on, off)| [on, off]);
    let ticks = if spec.ticks { 1.0 } else { 0.0 };
    let mut buffers: VertexBuffers<RouteLineVertex, u32> = VertexBuffers::new();
    // Width 1.0 is a placeholder: the shader extrudes `normal` by the real
    // width (logical px + world units), so the tessellated width never shows.
    let options = StrokeOptions::default().with_line_width(1.0);
    let result = StrokeTessellator::new().tessellate_path(
        &built,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |v: LyonStrokeVertex| {
            let pos = from_local(v.position_on_path());
            let width_world = spec.halfwidth_m.map_or(0.0, |h| {
                corridor_width_world(h, origin.y + f64::from(pos[1]))
            });
            RouteLineVertex {
                pos,
                normal: [v.normal().x, v.normal().y],
                color: spec.color,
                width_px: spec.width_px,
                width_world,
                along_world: v.advancement() / TESS_SCALE,
                side: match v.side() {
                    Side::Positive => 1.0,
                    Side::Negative => -1.0,
                },
                dash_px,
                ticks,
            }
        }),
    );
    match result {
        Ok(_) => {
            let base = out.vertices.len() as u32;
            out.vertices.extend(buffers.vertices);
            out.indices.extend(buffers.indices.iter().map(|i| i + base));
        }
        Err(e) => tracing::warn!(error = %e, "route stroke tessellation failed; stroke skipped"),
    }
}

/// Full corridor width in world units for a half-width in ground meters at
/// the given world-space y (latitude): Web-Mercator is conformal, so the
/// local scale is `cos(lat)` of the equatorial circumference both ways.
fn corridor_width_world(halfwidth_m: f64, world_y: f64) -> f32 {
    let lat = lat_lon_from_world(DVec2::new(0.5, world_y)).lat_deg();
    let meters_per_world = EARTH_CIRCUMFERENCE_M * lat.to_radians().cos().max(1e-6);
    (2.0 * halfwidth_m / meters_per_world) as f32
}

fn to_local(world: DVec2, origin: DVec2) -> lyon::math::Point {
    let local = world - origin; // f64 subtraction keeps deep-zoom precision
    point(local.x as f32 * TESS_SCALE, local.y as f32 * TESS_SCALE)
}

fn from_local(p: lyon::math::Point) -> [f32; 2] {
    [p.x / TESS_SCALE, p.y / TESS_SCALE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::RoutePointKind;
    use crate::map_theme::MapTheme;

    fn vertex(id: u64, lon: f64, lat: f64, kind: RoutePointKind) -> RouteVertex {
        RouteVertex {
            id,
            pos: [lon, lat],
            kind,
        }
    }

    /// Departure → waypoint → destination plus one alternate.
    fn route() -> RenderRoute {
        RenderRoute {
            points: vec![
                vertex(1, 8.6, 50.0, RoutePointKind::Departure),
                vertex(2, 9.5, 50.2, RoutePointKind::Waypoint),
                vertex(3, 11.2, 49.9, RoutePointKind::Destination),
                vertex(4, 11.8, 49.5, RoutePointKind::Alternate),
            ],
            leg_conflict: vec![false, true],
            ..RenderRoute::default()
        }
    }

    fn theme() -> crate::map_theme::RouteTheme {
        MapTheme::oldworld().route
    }

    fn build(route: &RenderRoute) -> Artifacts {
        build_artifacts(route, &theme())
    }

    #[test]
    fn main_track_excludes_alternates() {
        let artifacts = build(&route());
        assert_eq!(artifacts.track_world.len(), 3, "alternate is not flown");
        assert_eq!(artifacts.cum_m.len(), 3);
        assert!(artifacts.cum_m[2] > 100_000.0, "EDFE→EDQN-ish length");
    }

    /// Legs carry the line color, conflict legs the conflict tint, and the
    /// alternate link is a dashed thin stroke without chevrons.
    #[test]
    fn strokes_carry_their_styles() {
        let theme = theme();
        let artifacts = build(&route());
        let v = &artifacts.lines.vertices;
        assert!(!v.is_empty());
        assert!(
            v.iter()
                .filter(|v| v.ticks > 0.5)
                .all(|v| v.color == theme.line || v.color == theme.line_conflict),
            "chevron strokes are the legs"
        );
        assert!(
            v.iter().any(|v| v.color == theme.line_conflict),
            "the conflicted leg is tinted"
        );
        let alternates: Vec<_> = v.iter().filter(|v| v.dash_px != [0.0, 0.0]).collect();
        assert!(!alternates.is_empty(), "alternate link present");
        for a in &alternates {
            assert_eq!(a.width_px, ALTERNATE_LINE_WIDTH_PX);
            assert_eq!(a.ticks, 0.0, "no chevrons on alternate links");
            assert_eq!(a.dash_px, [ALTERNATE_DASH_PX.0, ALTERNATE_DASH_PX.1]);
        }
        // No corridor was requested.
        assert!(v.iter().all(|v| v.width_world == 0.0));
        // Sides are edge markers.
        assert!(v.iter().all(|v| v.side == 1.0 || v.side == -1.0));
    }

    /// The corridor renders only when set: a ground-fixed translucent
    /// stroke drawn first (under the legs), its world width following the
    /// Mercator scale at the vertex latitude.
    #[test]
    fn corridor_is_ground_fixed_and_under_the_legs() {
        let theme = theme();
        let mut with = route();
        with.corridor_halfwidth_m = Some(1000.0);
        let artifacts = build(&with);
        let v = &artifacts.lines.vertices;
        let corridor: Vec<_> = v.iter().filter(|v| v.width_world > 0.0).collect();
        assert!(!corridor.is_empty());
        assert_eq!(
            v[0].color, theme.corridor,
            "corridor tessellates first (bottom of the route stack)"
        );
        for c in &corridor {
            assert_eq!(c.color, theme.corridor);
            assert_eq!(c.width_px, 0.0);
            assert_eq!(c.ticks, 0.0);
            // 2 km full width at ~50° N: 2000 / (40075km · cos 50°).
            let expected = 2000.0 / (EARTH_CIRCUMFERENCE_M * 50.0_f64.to_radians().cos());
            let ratio = f64::from(c.width_world) / expected;
            assert!(
                (0.95..1.05).contains(&ratio),
                "width_world {} vs expected {expected}",
                c.width_world
            );
        }
        // A zero/absent half-width keeps the corridor off.
        let mut zero = route();
        zero.corridor_halfwidth_m = Some(0.0);
        assert!(
            build(&zero)
                .lines
                .vertices
                .iter()
                .all(|v| v.width_world == 0.0),
            "zero half-width draws no corridor"
        );
    }

    /// TOC/TOD markers land on the polyline at their along-track distance
    /// (matching `path::point_at`), not merely at the provided position.
    #[test]
    fn toc_tod_markers_interpolate_along_the_track() {
        let mut r = route();
        let half = {
            let artifacts = build(&r);
            artifacts.cum_m[2] / 2.0
        };
        r.toc = Some((half, [0.0, 0.0])); // deliberately wrong position
        r.tod = Some((f64::INFINITY, [0.0, 0.0])); // clamps to the destination
        let artifacts = build(&r);
        let toc = artifacts
            .base_instances
            .iter()
            .find(|b| b.key == RouteSymbolKey::Toc)
            .map(|b| b.instance)
            .expect("toc instance");
        let expected = path::point_at(&artifacts.track_world, &artifacts.cum_m, half)
            .expect("on track")
            - artifacts.origin_world;
        assert!((f64::from(toc.anchor_local[0]) - expected.x).abs() < 1e-7);
        assert!((f64::from(toc.anchor_local[1]) - expected.y).abs() < 1e-7);
        let tod = artifacts
            .base_instances
            .iter()
            .find(|b| b.key == RouteSymbolKey::Tod)
            .map(|b| b.instance)
            .expect("tod instance");
        let destination = *artifacts.track_world.last().expect("track") - artifacts.origin_world;
        assert!((f64::from(tod.anchor_local[0]) - destination.x).abs() < 1e-7);
        assert_eq!(toc.size_px, RouteSymbolKey::Toc.size_px());
    }

    /// Handle instances: one per point with the right mesh key (carrying
    /// its vertex id; TOC/TOD carry none), grouped so batches stay
    /// contiguous; the scrub marker lands interpolated on the track and is
    /// assembled without a worker rebuild.
    #[test]
    fn instances_batch_by_key_with_scrub_on_top() {
        let artifacts = build(&route());
        let keys: Vec<RouteSymbolKey> = artifacts.base_instances.iter().map(|b| b.key).collect();
        assert_eq!(
            keys,
            vec![
                RouteSymbolKey::Departure,
                RouteSymbolKey::Waypoint,
                RouteSymbolKey::Destination,
                RouteSymbolKey::Alternate,
            ]
        );
        let ids: Vec<Option<u64>> = artifacts.base_instances.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![Some(1), Some(2), Some(3), Some(4)]);

        let half = artifacts.cum_m[2] / 2.0;
        let (instances, batches) = assemble_instances(&artifacts, Some(half), None);
        assert_eq!(instances.len(), 5);
        assert_eq!(batches.last().map(|b| b.key), Some(RouteSymbolKey::Scrub));
        // Batch ranges tile the instance buffer in order.
        let mut covered = 0;
        for batch in &batches {
            assert_eq!(batch.instances.start, covered);
            covered = batch.instances.end;
        }
        assert_eq!(covered as usize, instances.len());
        let scrub = instances.last().expect("scrub instance");
        let expected = path::point_at(&artifacts.track_world, &artifacts.cum_m, half)
            .expect("on track")
            - artifacts.origin_world;
        assert!((f64::from(scrub.anchor_local[0]) - expected.x).abs() < 1e-7);
        assert!((f64::from(scrub.anchor_local[1]) - expected.y).abs() < 1e-7);

        // No scrub → no marker; empty track → never panics.
        let (instances, batches) = assemble_instances(&artifacts, None, None);
        assert_eq!(instances.len(), 4);
        assert!(batches.iter().all(|b| b.key != RouteSymbolKey::Scrub));
        let empty = build(&RenderRoute::default());
        let (instances, _) = assemble_instances(&empty, Some(1000.0), None);
        assert!(instances.is_empty());
    }

    /// The hover highlight enlarges exactly its handle's instance — same
    /// anchor, scaled size — and never touches the others or the TOC/TOD
    /// markers (which carry no vertex id); an id the route does not carry
    /// changes nothing. All from the retained artifacts: no rebuild.
    #[test]
    fn highlight_enlarges_only_its_handle_instance() {
        let mut r = route();
        r.toc = Some((10_000.0, [9.0, 50.1]));
        let artifacts = build(&r);
        let (plain, plain_batches) = assemble_instances(&artifacts, None, None);
        let (lit, lit_batches) = assemble_instances(&artifacts, None, Some(2));
        assert_eq!(plain_batches, lit_batches, "batching is unaffected");
        assert_eq!(plain.len(), lit.len());
        for (index, (p, l)) in plain.iter().zip(&lit).enumerate() {
            let base = &artifacts.base_instances[index];
            if base.id == Some(2) {
                assert_eq!(
                    l.size_px,
                    p.size_px * super::super::symbols::HIGHLIGHT_SCALE,
                    "the hovered waypoint handle is enlarged"
                );
                assert_eq!(l.anchor_local, p.anchor_local, "anchor stays put");
            } else {
                assert_eq!(l, p, "{index}: only the hovered handle changes");
            }
        }

        // Unknown id (stale hover against a shrunken route): identical.
        let (ghost, _) = assemble_instances(&artifacts, None, Some(99));
        assert_eq!(ghost, plain);
    }

    /// A missing/short `leg_conflict` treats legs as conflict-free; a
    /// single-point route draws its handle and nothing else.
    #[test]
    fn degenerate_inputs_stay_safe() {
        let theme = theme();
        let mut r = route();
        r.leg_conflict = vec![]; // shorter than the two legs
        let artifacts = build(&r);
        assert!(
            artifacts
                .lines
                .vertices
                .iter()
                .filter(|v| v.ticks > 0.5)
                .all(|v| v.color == theme.line),
            "missing conflict flags default to the line color"
        );

        let single = RenderRoute {
            points: vec![vertex(1, 8.6, 50.0, RoutePointKind::Departure)],
            corridor_halfwidth_m: Some(2000.0),
            scrub_along_m: Some(0.0),
            ..RenderRoute::default()
        };
        let artifacts = build(&single);
        assert!(artifacts.lines.vertices.is_empty(), "no legs, no corridor");
        assert_eq!(artifacts.base_instances.len(), 1);
        let (instances, _) = assemble_instances(&artifacts, single.scrub_along_m, None);
        assert_eq!(instances.len(), 2, "handle + scrub pinned to the point");
    }

    /// Leg labels anchor at the drawn leg's world midpoint, carry the
    /// route accent color, the leg-label priority/zoom gate and stable
    /// namespaced ids; blank and missing entries draw nothing.
    #[test]
    fn leg_labels_anchor_at_the_midpoint_with_route_styling() {
        let theme = theme();
        let mut r = route();
        r.leg_labels = vec![
            Some("MH 053 · 135 kt · 4500".to_owned()),
            Some("   ".to_owned()), // blank → dropped
        ];
        let artifacts = build(&r);
        assert_eq!(artifacts.labels.len(), 1, "blank labels are dropped");
        let label = &artifacts.labels[0];
        assert_eq!(&*label.text, "MH 053 · 135 kt · 4500");
        let expected = (artifacts.track_world[0] + artifacts.track_world[1]) * 0.5;
        assert_eq!(label.anchor, LabelAnchor::World(expected));
        assert_eq!(label.placement, LabelPlacement::Center);
        assert_eq!(label.color, theme.line, "route-theme colored");
        assert_eq!(label.priority, priority::ROUTE_LEG);
        assert!(
            label.priority < priority::AIRPORT,
            "yields to airport idents"
        );
        assert_eq!(label.min_zoom, LEG_LABEL_MIN_ZOOM);
        assert_eq!(label.id, LABEL_ID_NAMESPACE);

        // No labels fed (the editing burst / NotComputable state): none built.
        assert!(build(&route()).labels.is_empty());
        // Entries beyond the track's legs are ignored, like leg_conflict.
        let mut over = route();
        over.leg_labels = vec![None, None, Some("ghost".to_owned())];
        assert!(build(&over).labels.is_empty());
    }

    /// Synthetic n-point route with the worst realistic feature load per
    /// point: a label on every leg, corridor, TOC/TOD and a scrub marker.
    fn synthetic_route(n: usize) -> RenderRoute {
        let points = (0..n)
            .map(|i| {
                let t = i as f64 / n.max(2) as f64;
                vertex(
                    i as u64,
                    8.0 + 4.0 * t,
                    49.0 + (t * 20.0).sin() * 1.5,
                    match i {
                        0 => RoutePointKind::Departure,
                        i if i == n - 1 => RoutePointKind::Destination,
                        _ => RoutePointKind::Waypoint,
                    },
                )
            })
            .collect::<Vec<_>>();
        let legs = n.saturating_sub(1);
        RenderRoute {
            points,
            leg_conflict: vec![false; legs],
            leg_labels: (0..legs)
                .map(|i| Some(format!("MH {i:03} · 110 kt · 4500")))
                .collect(),
            toc: Some((20_000.0, [8.2, 49.1])),
            tod: Some((150_000.0, [11.5, 49.9])),
            corridor_halfwidth_m: Some(4_000.0),
            scrub_along_m: Some(60_000.0),
            snap_ring: None,
            highlight: None,
        }
    }

    /// Bench-style budget assertion for the synchronous in-`prepare` build
    /// (see [`crate::layers::route::SYNC_BUILD_MAX_POINTS`]): a route at
    /// the sync threshold — far above any realistic route — must build
    /// well within a render-thread frame. Measured medians: ~0.12 ms
    /// optimized, ~0.7 ms unoptimized at 256 points (~0.07 / ~0.3 ms at a
    /// generous realistic 100); the asserted bounds leave slack for slow
    /// CI machines while still catching an accidental complexity
    /// regression (e.g. something quadratic in the leg count).
    #[test]
    fn synchronous_build_fits_the_prepare_budget() {
        let theme = theme();
        let route = synthetic_route(crate::layers::route::SYNC_BUILD_MAX_POINTS);
        for _ in 0..3 {
            std::hint::black_box(build_artifacts(&route, &theme)); // warmup
        }
        let mut samples: Vec<f64> = (0..32)
            .map(|_| {
                let started = std::time::Instant::now();
                std::hint::black_box(build_artifacts(&route, &theme));
                started.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        let median_ms = samples[samples.len() / 2];
        let budget_ms = if cfg!(debug_assertions) { 8.0 } else { 0.5 };
        assert!(
            median_ms < budget_ms,
            "threshold-size route build took {median_ms:.3} ms (budget {budget_ms} ms): \
             too slow for the synchronous in-prepare path"
        );
    }

    /// The synchronous in-`prepare` path and the worker fallback run the
    /// same [`build_artifacts`] — their outputs must be bit-identical for
    /// the same input, so the path taken can never change what is drawn.
    #[test]
    fn sync_and_worker_builds_are_identical() {
        use crate::workers::{JobQueue, WorkerPool};
        use std::time::{Duration, Instant};

        let route = synthetic_route(40);
        let direct = build_artifacts(&route, &theme());

        let pool = WorkerPool::new(1);
        let mut queue: JobQueue<Artifacts> = JobQueue::new();
        let job_route = route.clone();
        queue.submit(&pool, move || build_artifacts(&job_route, &theme()));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.is_empty() && Instant::now() < deadline {
            results.extend(queue.drain());
            std::thread::sleep(Duration::from_millis(1));
        }
        let worker = results.pop().expect("worker build result");

        assert_eq!(worker.origin_world, direct.origin_world);
        assert_eq!(worker.lines.vertices, direct.lines.vertices);
        assert_eq!(worker.lines.indices, direct.lines.indices);
        assert_eq!(worker.base_instances, direct.base_instances);
        assert_eq!(worker.labels, direct.labels);
        assert_eq!(worker.track_world, direct.track_world);
        assert_eq!(worker.cum_m, direct.cum_m);
    }

    /// The label offset is perpendicular to its leg, on the upper screen
    /// side, at the authored distance — and alternates get no labels (only
    /// main-track legs are labelled).
    #[test]
    fn leg_label_offsets_hang_off_the_line() {
        let mut r = route();
        r.leg_labels = vec![Some("L0".to_owned()), Some("L1".to_owned())];
        let artifacts = build(&r);
        assert_eq!(artifacts.labels.len(), 2, "one per labelled main-track leg");
        for (index, label) in artifacts.labels.iter().enumerate() {
            let along = (artifacts.track_world[index + 1] - artifacts.track_world[index])
                .normalize()
                .as_vec2();
            let offset = label.offset_px;
            assert!(
                (offset.length() - LEG_LABEL_OFFSET_PX).abs() < 1e-3,
                "{offset:?}"
            );
            assert!(
                offset.dot(along).abs() < 1e-3,
                "offset must be perpendicular"
            );
            assert!(
                offset.y < 0.0,
                "label sits above the line (screen y is down)"
            );
        }

        // A vertical (meridian) leg labels to the right, never on the line.
        let vertical = RenderRoute {
            points: vec![
                vertex(1, 10.0, 50.0, RoutePointKind::Departure),
                vertex(2, 10.0, 49.0, RoutePointKind::Destination),
            ],
            leg_labels: vec![Some("V".to_owned())],
            ..RenderRoute::default()
        };
        let label = &build(&vertical).labels[0];
        assert_eq!(label.offset_px.y, 0.0);
        assert!(label.offset_px.x > 0.0, "vertical legs label to the right");

        // A zero-length leg has no direction: no label, no NaN.
        let degenerate = RenderRoute {
            points: vec![
                vertex(1, 10.0, 50.0, RoutePointKind::Departure),
                vertex(2, 10.0, 50.0, RoutePointKind::Destination),
            ],
            leg_labels: vec![Some("Z".to_owned())],
            ..RenderRoute::default()
        };
        assert!(build(&degenerate).labels.is_empty());
    }
}

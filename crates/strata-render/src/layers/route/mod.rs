//! The flight-route layer: the planned route polyline with direction
//! chevrons (conflict legs tinted), waypoint handles / TOC / TOD markers as
//! symbol instances, per-leg text labels, the optional ground-fixed terrain
//! corridor, the profile-scrub marker and the pulsing snap-indication ring.
//!
//! Draw order (within the layer, bottom to top): corridor → legs →
//! alternate links → markers (scrub last) → snap ring. Leg labels are
//! handed to the shared text system via [`RouteLayer::labels`] (the
//! renderer forwards them each frame, like the other layers). Geometry
//! rebuilds only when the route geometry actually changes; a scrub move or
//! a hover-highlight change re-assembles the marker instances from the
//! retained artifacts without re-tessellating, a snap-ring move only
//! re-writes a uniform (the highlight's static glow ring rides one too) and
//! an identical [`RenderRoute`] is ignored entirely. The renderer itself draws
//! nothing — and reports no redraw demand — while no route is set, so the
//! explorer stays untouched.
//!
//! Routes up to [`SYNC_BUILD_MAX_POINTS`] tessellate **synchronously** in
//! `prepare()` on the render thread (a realistic route is well under 100
//! points and builds in tens of microseconds — see the budget test in
//! [`build`]): a waypoint drag's `set_route` per mouse move is then
//! guaranteed to draw in the very next frame. The worker path remains only
//! as a fallback for absurd sizes — it cannot keep up with per-frame
//! geometry changes, because each `prepare()` invalidates the in-flight
//! job's generation, so under continuous movement no job ever survives to
//! `drain()` (geometry would freeze until the mouse rests).

mod build;
mod path;
mod ring;
mod symbols;

pub use build::{LEG_LABEL_MIN_ZOOM, LEG_LABEL_OFFSET_PX};

/// Largest route (total points, alternates included) that tessellates
/// synchronously in `prepare()`. At this size one full build (corridor +
/// per-leg strokes + labels + markers) measures ~0.12 ms optimized
/// (~0.7 ms unoptimized) — well under a 0.5 ms render-thread budget,
/// asserted by `build::tests::synchronous_build_fits_the_prepare_budget`.
/// Realistic routes are far smaller (<100 points). Anything larger falls
/// back to the worker pool — accepting frozen-while-dragging geometry over
/// render-thread stalls.
pub const SYNC_BUILD_MAX_POINTS: usize = 256;

use crate::features::RenderRoute;
use crate::layer::{DrawCtx, MapLayer, PrepareCtx};
use crate::layers::pipelines::{
    self, GpuMesh, OriginBinding, ROUTE_LINE_SHADER, ROUTE_RING_SHADER,
};
use crate::layers::symbols::{SymbolInstance, SymbolVertex};
use crate::map_theme::MapTheme;
use crate::text::LabelRequest;
use crate::workers::JobQueue;

use self::build::{Artifacts, RouteBatch, RouteLineVertex};
use self::ring::{RingAnimation, RingBinding, RingUniform};
use self::symbols::RouteSymbolAtlas;

use glam::DVec2;
use wgpu::util::DeviceExt as _;

use std::sync::Arc;

/// The planned flight route (polyline, handles, leg labels, corridor,
/// scrub marker, snap ring).
pub struct RouteLayer {
    theme: Arc<MapTheme>,
    route: Option<RenderRoute>,
    /// The marker atlas (mesh colors) must be rebuilt for the new theme.
    atlas_dirty: bool,
    /// Full geometry rebuild on the worker pool.
    data_dirty: bool,
    /// Only the scrub marker or the hover highlight moved: re-assemble the
    /// marker instances from the retained artifacts (no tessellation).
    instances_dirty: bool,
    pending: bool,
    jobs: JobQueue<Artifacts>,
    gpu: GpuState,
    /// Retained worker output (track + cumulative distances) so scrub moves
    /// skip the worker round-trip.
    artifacts: Option<Artifacts>,
    lines: Option<GpuMesh>,
    instances: Option<InstanceData>,
    /// Per-leg labels from the retained artifacts, exposed via
    /// [`labels`](Self::labels) for the renderer's per-frame forwarding.
    labels: Vec<LabelRequest>,
    /// Snap-ring pulse state; active while the route carries a snap target.
    ring: RingAnimation,
    /// The hover highlight resolved against the current route in
    /// `prepare()` (id found → glow ring uniform written): `draw()` reads
    /// this to know whether the highlight ring pass runs.
    highlight_visible: bool,
    origin_world: DVec2,
}

impl Default for RouteLayer {
    fn default() -> Self {
        Self::new(Arc::new(MapTheme::oldworld()))
    }
}

impl RouteLayer {
    pub fn new(theme: Arc<MapTheme>) -> Self {
        Self {
            theme,
            route: None,
            atlas_dirty: false,
            data_dirty: false,
            instances_dirty: false,
            pending: false,
            jobs: JobQueue::new(),
            gpu: GpuState::default(),
            artifacts: None,
            lines: None,
            instances: None,
            labels: Vec::new(),
            ring: RingAnimation::default(),
            highlight_visible: false,
            origin_world: DVec2::ZERO,
        }
    }

    /// Switch the color theme: rebuilds the marker atlas and re-tessellates
    /// the strokes (their colors are baked into the vertices).
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
        self.atlas_dirty = true;
        self.data_dirty = true;
    }

    /// Replace (or clear) the route. Identical data is ignored; a change in
    /// `scrub_along_m` or `highlight` alone re-assembles the marker
    /// instances from the retained artifacts instead of re-tessellating
    /// (the highlight ring additionally rides a per-frame uniform), and a
    /// `snap_ring` change alone rebuilds nothing at all (that ring rides a
    /// per-frame uniform). Returns whether anything changed.
    pub fn set_route(&mut self, route: Option<RenderRoute>) -> bool {
        if self.route == route {
            return false;
        }
        let (geometry_changed, instances_changed) = match (&self.route, &route) {
            (Some(old), Some(new)) => (
                old.points != new.points
                    || old.leg_conflict != new.leg_conflict
                    || old.leg_labels != new.leg_labels
                    || old.toc != new.toc
                    || old.tod != new.tod
                    || old.corridor_halfwidth_m != new.corridor_halfwidth_m,
                old.scrub_along_m != new.scrub_along_m || old.highlight != new.highlight,
            ),
            _ => (true, true),
        };
        self.route = route;
        if geometry_changed {
            self.data_dirty = true;
        } else if instances_changed {
            self.instances_dirty = true;
        }
        true
    }

    pub fn route(&self) -> Option<&RenderRoute> {
        self.route.as_ref()
    }

    /// Per-leg labels of the retained route geometry; the renderer forwards
    /// them to the [`crate::text::TextSystem`] each frame (zoom-gated by
    /// `min_zoom`, decluttered by priority there).
    pub fn labels(&self) -> &[LabelRequest] {
        &self.labels
    }

    /// Whether this route is small enough for the synchronous in-`prepare`
    /// tessellation (the frame-fresh path a drag relies on).
    fn builds_synchronously(route: &RenderRoute) -> bool {
        route.points.len() <= SYNC_BUILD_MAX_POINTS
    }

    /// Install one build output: upload the stroke mesh, take over the
    /// labels, retain the artifacts and assemble the marker instances.
    /// Shared verbatim by the synchronous path and the worker drain, so the
    /// two paths cannot drift apart.
    fn apply_artifacts(&mut self, device: &wgpu::Device, mut artifacts: Artifacts) {
        self.origin_world = artifacts.origin_world;
        self.lines = pipelines::upload_mesh(
            device,
            "strata route lines",
            &artifacts.lines.vertices,
            &artifacts.lines.indices,
        );
        self.labels = std::mem::take(&mut artifacts.labels);
        self.artifacts = Some(artifacts);
        self.upload_instances(device);
        self.instances_dirty = false;
        self.pending = false;
    }

    fn upload_instances(&mut self, device: &wgpu::Device) {
        let Some(artifacts) = &self.artifacts else {
            self.instances = None;
            return;
        };
        let scrub = self.route.as_ref().and_then(|r| r.scrub_along_m);
        let highlight = self.route.as_ref().and_then(|r| r.highlight);
        let (instances, batches) = build::assemble_instances(artifacts, scrub, highlight);
        self.instances = (!instances.is_empty()).then(|| InstanceData {
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("strata route marker instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            batches,
        });
    }
}

impl MapLayer for RouteLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.gpu.ensure(ctx, &self.theme.route);
        if self.atlas_dirty {
            // A fresh `ensure` above already used the new theme; rebuilding
            // an existing atlas swaps the marker colors in place.
            if let GpuState::Ready(gpu) = &mut self.gpu {
                gpu.rebuild_atlas(ctx.device, &self.theme.route);
            }
            self.atlas_dirty = false;
        }

        if self.data_dirty {
            self.data_dirty = false;
            self.instances_dirty = false;
            // Drop any in-flight worker build: its input is superseded.
            self.jobs.invalidate();
            match &self.route {
                None => {
                    self.artifacts = None;
                    self.lines = None;
                    self.instances = None;
                    self.labels = Vec::new();
                    self.pending = false;
                }
                // The frame-fresh path: small routes build right here, so
                // a drag's mouse-move → `set_route` → this prepare → draw
                // shows the moved geometry in the very next frame. The
                // worker would be invalidated again before completing on
                // every per-frame change — geometry would freeze mid-drag.
                Some(route) if Self::builds_synchronously(route) => {
                    let started = std::time::Instant::now();
                    let artifacts = build::build_artifacts(route, &self.theme.route);
                    let points = route.points.len();
                    let ms = started.elapsed().as_secs_f64() * 1e3;
                    self.apply_artifacts(ctx.device, artifacts);
                    tracing::trace!(points, ms, "route geometry built synchronously");
                }
                Some(route) => {
                    let route = route.clone();
                    let theme = self.theme.route;
                    self.pending = true;
                    self.jobs.submit(ctx.workers, move || {
                        let started = std::time::Instant::now();
                        let artifacts = build::build_artifacts(&route, &theme);
                        tracing::debug!(
                            points = route.points.len(),
                            ms = started.elapsed().as_secs_f64() * 1e3,
                            "route geometry built on a worker"
                        );
                        artifacts
                    });
                }
            }
        }

        if let Some(artifacts) = self.jobs.drain().into_iter().next_back() {
            self.apply_artifacts(ctx.device, artifacts);
        } else if self.instances_dirty {
            self.upload_instances(ctx.device);
            self.instances_dirty = false;
        }

        if let GpuState::Ready(gpu) = &self.gpu
            && (self.lines.is_some() || self.instances.is_some())
        {
            gpu.origin
                .update(ctx.queue, self.origin_world - ctx.camera.center());
        }

        // The snap ring rides its own uniform: advance the pulse and
        // re-write center (camera-relative, f64 subtraction) + pulse values
        // every frame while a target is snapped. The animation goes idle —
        // and stops demanding redraws — the moment the target clears.
        let target = self.route.as_ref().and_then(|r| r.snap_ring);
        let pulse = self.ring.advance(target, ctx.frame.dt);
        if let (GpuState::Ready(gpu), Some(pulse), Some(pos)) = (&self.gpu, pulse, target) {
            let center_rel = path::world_from_pos(pos) - ctx.camera.center();
            gpu.ring.update(
                ctx.queue,
                &RingUniform::new(center_rel, pulse, self.theme.route.line),
            );
        }

        // The hover highlight's glow ring is the same per-frame-uniform
        // recipe, minus the animation (static, so it demands no redraws):
        // resolve the highlighted id against the current route and re-write
        // the camera-relative center while it is shown. An id the route
        // does not carry (a stale hover) simply draws nothing.
        let highlighted = self.route.as_ref().and_then(|r| {
            let id = r.highlight?;
            r.points.iter().find(|p| p.id == id)
        });
        self.highlight_visible = false;
        if let (GpuState::Ready(gpu), Some(point)) = (&self.gpu, highlighted) {
            let center_rel = path::world_from_pos(point.pos) - ctx.camera.center();
            let handle_px = build::handle_key(point).size_px() * symbols::HIGHLIGHT_SCALE;
            gpu.highlight.update(
                ctx.queue,
                &RingUniform::highlight(center_rel, handle_px, self.theme.route.line),
            );
            self.highlight_visible = true;
        }
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, _ctx: &DrawCtx<'_>) {
        let GpuState::Ready(gpu) = &self.gpu else {
            return;
        };
        if self.lines.is_none()
            && self.instances.is_none()
            && !self.ring.active()
            && !self.highlight_visible
        {
            return;
        }
        pass.set_bind_group(1, &gpu.origin.bind_group, &[]);
        if let Some(lines) = &self.lines {
            pass.set_pipeline(&gpu.line_pipeline);
            pass.set_vertex_buffer(0, lines.vertices.slice(..));
            pass.set_index_buffer(lines.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..lines.index_count, 0, 0..1);
        }
        if let Some(instances) = &self.instances {
            pass.set_pipeline(&gpu.marker_pipeline);
            pass.set_vertex_buffer(0, gpu.mesh_vertices.slice(..));
            pass.set_vertex_buffer(1, instances.buffer.slice(..));
            pass.set_index_buffer(gpu.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
            for batch in &instances.batches {
                let range = gpu.atlas.range(batch.key);
                pass.draw_indexed(
                    range.indices.clone(),
                    range.base_vertex,
                    batch.instances.clone(),
                );
            }
        }
        if self.highlight_visible {
            // The hover glow: the ring pipeline over its own static uniform
            // (drawn under the snap ring, which is the louder signal).
            pass.set_pipeline(&gpu.ring_pipeline);
            pass.set_bind_group(1, &gpu.highlight.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        if self.ring.active() {
            // One vertex-bufferless quad; group 1 is the ring's own uniform.
            pass.set_pipeline(&gpu.ring_pipeline);
            pass.set_bind_group(1, &gpu.ring.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    fn wants_redraw(&self) -> bool {
        // The ring pulses for as long as a snap target is shown.
        self.pending || self.ring.active()
    }
}

#[derive(Default)]
enum GpuState {
    #[default]
    Uninitialized,
    Ready(Box<Gpu>),
    Failed,
}

impl GpuState {
    fn ensure(&mut self, ctx: &PrepareCtx<'_>, theme: &crate::map_theme::RouteTheme) {
        if !matches!(self, Self::Uninitialized) {
            return;
        }
        *self = match Gpu::new(ctx, theme) {
            Ok(gpu) => Self::Ready(Box::new(gpu)),
            Err(e) => {
                tracing::error!(error = %e, "route pipelines failed; layer disabled");
                Self::Failed
            }
        };
    }
}

struct Gpu {
    origin: OriginBinding,
    line_pipeline: wgpu::RenderPipeline,
    marker_pipeline: wgpu::RenderPipeline,
    ring: RingBinding,
    /// The hover highlight's glow ring: the same shader/pipeline as the
    /// snap ring over its own (static) uniform, so both can draw in one
    /// frame.
    highlight: RingBinding,
    ring_pipeline: wgpu::RenderPipeline,
    atlas: RouteSymbolAtlas,
    mesh_vertices: wgpu::Buffer,
    mesh_indices: wgpu::Buffer,
}

impl Gpu {
    fn new(
        ctx: &PrepareCtx<'_>,
        theme: &crate::map_theme::RouteTheme,
    ) -> Result<Self, crate::error::RenderError> {
        let origin = OriginBinding::new(ctx.device, "strata route origin");
        let line_module = pipelines::create_layer_module(ctx, ROUTE_LINE_SHADER)?;
        let line_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata route lines",
            &line_module,
            &origin.layout,
            &[RouteLineVertex::layout()],
        );
        // Markers ride the existing instancing path (`symbol.wgsl`).
        let marker_module = ctx.shaders.create_module(ctx.device, "symbol.wgsl")?;
        let marker_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata route markers",
            &marker_module,
            &origin.layout,
            &[SymbolVertex::layout(), SymbolInstance::layout()],
        );
        // The snap ring: a vertex-bufferless quad over its own uniform.
        // The hover highlight shares the pipeline with its own binding.
        let ring = RingBinding::new(ctx.device, "strata route snap ring");
        let highlight = RingBinding::new(ctx.device, "strata route hover highlight");
        let ring_module = pipelines::create_layer_module(ctx, ROUTE_RING_SHADER)?;
        let ring_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata route snap ring",
            &ring_module,
            &ring.layout,
            &[],
        );
        let atlas = RouteSymbolAtlas::build(theme);
        let (mesh_vertices, mesh_indices) = upload_atlas(ctx.device, &atlas);
        Ok(Self {
            origin,
            line_pipeline,
            marker_pipeline,
            ring,
            highlight,
            ring_pipeline,
            atlas,
            mesh_vertices,
            mesh_indices,
        })
    }

    /// Rebuild the marker meshes with new theme colors (pipelines, origin
    /// and instance buffers stay).
    fn rebuild_atlas(&mut self, device: &wgpu::Device, theme: &crate::map_theme::RouteTheme) {
        self.atlas = RouteSymbolAtlas::build(theme);
        let (vertices, indices) = upload_atlas(device, &self.atlas);
        self.mesh_vertices = vertices;
        self.mesh_indices = indices;
    }
}

fn upload_atlas(device: &wgpu::Device, atlas: &RouteSymbolAtlas) -> (wgpu::Buffer, wgpu::Buffer) {
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("strata route marker mesh vertices"),
        contents: bytemuck::cast_slice(&atlas.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("strata route marker mesh indices"),
        contents: bytemuck::cast_slice(&atlas.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vertices, indices)
}

struct InstanceData {
    buffer: wgpu::Buffer,
    batches: Vec<RouteBatch>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{RoutePointKind, RouteVertex};

    fn route(scrub: Option<f64>) -> RenderRoute {
        RenderRoute {
            points: vec![
                RouteVertex {
                    id: 1,
                    pos: [8.6, 50.0],
                    kind: RoutePointKind::Departure,
                },
                RouteVertex {
                    id: 2,
                    pos: [11.2, 49.9],
                    kind: RoutePointKind::Destination,
                },
            ],
            leg_conflict: vec![false],
            scrub_along_m: scrub,
            ..RenderRoute::default()
        }
    }

    /// Equality short-circuit like the other layers: re-pushing the same
    /// route (or `None` over `None`) must not rebuild anything.
    #[test]
    fn identical_route_is_ignored() {
        let mut layer = RouteLayer::default();
        assert!(!layer.set_route(None), "no route stays no route");
        assert!(layer.set_route(Some(route(None))));
        layer.data_dirty = false; // as after a prepare()
        assert!(!layer.set_route(Some(route(None))));
        assert!(!layer.data_dirty, "identical route must not re-tessellate");
        assert!(!layer.instances_dirty);
        assert!(layer.set_route(None), "clearing is a change");
        assert!(layer.data_dirty);
    }

    /// A scrub-only change repositions the marker from the retained
    /// artifacts — never a worker re-tessellation.
    #[test]
    fn scrub_moves_skip_the_geometry_rebuild() {
        let mut layer = RouteLayer::default();
        assert!(layer.set_route(Some(route(None))));
        layer.data_dirty = false; // as after a prepare()
        assert!(layer.set_route(Some(route(Some(10_000.0)))));
        assert!(!layer.data_dirty, "scrub must not re-tessellate the polyline");
        assert!(layer.instances_dirty);
        layer.instances_dirty = false;
        assert!(layer.set_route(Some(route(Some(20_000.0)))));
        assert!(layer.instances_dirty && !layer.data_dirty);
        // Geometry fields still trigger the full rebuild.
        let mut moved = route(Some(20_000.0));
        moved.points[1].pos = [11.3, 49.8];
        assert!(layer.set_route(Some(moved)));
        assert!(layer.data_dirty);
    }

    /// A hover-highlight change alone rides the same instance fast path as
    /// the scrub — re-assembled from the retained artifacts, never a
    /// re-tessellation (the drag-freshness contract must not regress when
    /// a hovered row's waypoint is being dragged).
    #[test]
    fn highlight_changes_take_the_instance_fast_path() {
        let mut layer = RouteLayer::default();
        assert!(layer.set_route(Some(route(None))));
        layer.data_dirty = false; // as after a prepare()

        let mut lit = route(None);
        lit.highlight = Some(2);
        assert!(layer.set_route(Some(lit.clone())), "highlighting is a change");
        assert!(!layer.data_dirty, "highlight must not re-tessellate");
        assert!(layer.instances_dirty, "the handle instance is re-assembled");
        layer.instances_dirty = false;

        // Moving the highlight to another vertex and clearing it are the
        // same cheap path; identical highlights stay idle.
        assert!(!layer.set_route(Some(lit)));
        assert!(!layer.instances_dirty);
        let mut moved = route(None);
        moved.highlight = Some(1);
        assert!(layer.set_route(Some(moved)));
        assert!(layer.instances_dirty && !layer.data_dirty);
        layer.instances_dirty = false;
        assert!(layer.set_route(Some(route(None))), "clearing is a change");
        assert!(layer.instances_dirty && !layer.data_dirty);
    }

    /// The sync/worker split (drag freshness): every realistically sized
    /// route — way beyond the spec'd <100 points — must take the
    /// synchronous in-`prepare` path, because the worker path cannot
    /// deliver per-frame geometry under continuous `set_route` (each
    /// rebuild invalidates the in-flight job). The worker remains solely
    /// for absurd sizes.
    #[test]
    fn small_routes_take_the_synchronous_path() {
        let sized = |n: usize| RenderRoute {
            points: (0..n)
                .map(|i| RouteVertex {
                    id: i as u64,
                    pos: [8.0 + i as f64 * 0.01, 50.0],
                    kind: RoutePointKind::Waypoint,
                })
                .collect(),
            ..RenderRoute::default()
        };
        assert!(RouteLayer::builds_synchronously(&route(None)));
        assert!(RouteLayer::builds_synchronously(&sized(100)));
        assert!(RouteLayer::builds_synchronously(&sized(SYNC_BUILD_MAX_POINTS)));
        assert!(
            !RouteLayer::builds_synchronously(&sized(SYNC_BUILD_MAX_POINTS + 1)),
            "absurd sizes keep the worker fallback"
        );
    }

    /// An idle layer (no route, or a settled one) demands no redraws — the
    /// explorer's idle map stays idle.
    #[test]
    fn wants_redraw_is_honest_without_pending_work() {
        let mut layer = RouteLayer::default();
        assert!(!layer.wants_redraw());
        layer.set_route(Some(route(None)));
        // Dirty but not yet submitted (no prepare ran): nothing in flight.
        assert!(!layer.wants_redraw());
        layer.pending = true; // as after a prepare() submitted the job
        assert!(layer.wants_redraw());
    }

    /// Label-only changes (a recompute landing on unchanged geometry) must
    /// rebuild the artifacts — the labels live there — while a snap-ring
    /// change alone rebuilds nothing: it rides a per-frame uniform.
    #[test]
    fn label_changes_rebuild_but_ring_moves_do_not() {
        let mut layer = RouteLayer::default();
        layer.set_route(Some(route(None)));
        layer.data_dirty = false; // as after a prepare()

        let mut labelled = route(None);
        labelled.leg_labels = vec![Some("MH 094 · 110 kt · 4500".to_owned())];
        assert!(layer.set_route(Some(labelled.clone())));
        assert!(layer.data_dirty, "new labels need a worker rebuild");
        layer.data_dirty = false;

        let mut snapped = labelled.clone();
        snapped.snap_ring = Some([9.5, 50.1]);
        assert!(layer.set_route(Some(snapped.clone())), "ring change is a change");
        assert!(!layer.data_dirty, "ring must not re-tessellate");
        assert!(!layer.instances_dirty, "ring must not re-upload instances");

        let mut moved = snapped.clone();
        moved.snap_ring = Some([9.6, 50.0]);
        assert!(layer.set_route(Some(moved)));
        assert!(!layer.data_dirty && !layer.instances_dirty);

        // Clearing the ring is also only a uniform-side change.
        assert!(layer.set_route(Some(labelled)));
        assert!(!layer.data_dirty && !layer.instances_dirty);
    }

    /// The ring animation drives redraw demand: pulsing while a target is
    /// snapped, idle the moment it clears — and the pulse restarts when the
    /// snap target moves to another feature.
    #[test]
    fn ring_pulses_while_snapped_and_idles_after() {
        use std::time::Duration;

        let mut layer = RouteLayer::default();
        assert!(!layer.ring.active());

        let dt = Duration::from_millis(16);
        let pulse = layer.ring.advance(Some([9.5, 50.1]), dt).expect("pulse");
        assert!(layer.ring.active());
        assert!(layer.wants_redraw(), "an active ring keeps animating");
        assert_eq!(pulse, ring::pulse(0.0), "fresh target, fresh pulse");

        let later = layer.ring.advance(Some([9.5, 50.1]), dt).expect("pulse");
        assert!(later.radius_px > pulse.radius_px, "the pulse expands");
        assert!(later.fade < pulse.fade, "… while fading");

        assert!(layer.ring.advance(None, dt).is_none());
        assert!(!layer.ring.active());
        assert!(!layer.wants_redraw(), "no ring, no redraw demand");
    }
}

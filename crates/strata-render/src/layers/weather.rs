//! SIGMET overlay polygons: shader-hatched translucent fills with dashed
//! outlines, tessellated on the worker pool, plus hazard labels at each
//! polygon's visual center. METAR dots live in
//! [`crate::layers::PointLayer`] as `PointKind::WeatherStation`.

use crate::features::RenderSigmet;
use crate::layer::{DrawCtx, MapLayer, PrepareCtx};
use crate::layers::pipelines::{self, GpuMesh, LINE_DASH_SHADER, OriginBinding, WEATHER_SHADER};
use crate::layers::polylabel::pole_of_inaccessibility;
use crate::layers::style::{label_color_from_border, priority};
use crate::layers::tess::{
    FillMesh, FillVertex, LineMesh, LineVertex, StrokeStyleSpec, ring_to_world, tessellate_fill,
    tessellate_ring_stroke,
};
use crate::map_theme::MapTheme;
use crate::text::{LabelAnchor, LabelPlacement, LabelRequest};
use crate::workers::JobQueue;

use glam::{DVec2, Vec2};

use std::sync::Arc;

/// SIGMET areas are large; their hazard labels show early.
pub const SIGMET_LABEL_MIN_ZOOM: f32 = 5.0;

/// Namespace bit for SIGMET label ids.
const LABEL_ID_NAMESPACE: u64 = 1 << 63;

const SIGMET_OUTLINE_WIDTH_PX: f32 = 1.6;
const SIGMET_OUTLINE_DASH_PX: (f32, f32) = (8.0, 5.0);

/// SIGMET hatched overlay polygons.
pub struct WeatherLayer {
    theme: Arc<MapTheme>,
    sigmets: Vec<RenderSigmet>,
    data_dirty: bool,
    pending: bool,
    jobs: JobQueue<Artifacts>,
    gpu: GpuState,
    meshes: Option<UploadedMeshes>,
    origin_world: DVec2,
    labels: Vec<LabelRequest>,
}

impl Default for WeatherLayer {
    fn default() -> Self {
        Self::new(Arc::new(MapTheme::oldworld()))
    }
}

impl WeatherLayer {
    pub fn new(theme: Arc<MapTheme>) -> Self {
        Self {
            theme,
            sigmets: Vec::new(),
            data_dirty: false,
            pending: false,
            jobs: JobQueue::new(),
            gpu: GpuState::default(),
            meshes: None,
            origin_world: DVec2::ZERO,
            labels: Vec::new(),
        }
    }

    /// Switch the color theme: re-tessellates the current SIGMET set with
    /// the new hatch/outline colors.
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
        self.data_dirty = true;
    }

    /// Replace the SIGMET set. Identical data is ignored — the periodic
    /// weather refresh usually returns the same areas; returns whether the
    /// set changed.
    pub fn set_sigmets(&mut self, sigmets: Vec<RenderSigmet>) -> bool {
        if self.sigmets == sigmets {
            return false;
        }
        self.sigmets = sigmets;
        self.data_dirty = true;
        true
    }

    pub fn sigmets(&self) -> &[RenderSigmet] {
        &self.sigmets
    }

    /// Hazard labels for the current SIGMETs; the renderer forwards them to
    /// the [`crate::text::TextSystem`] while the weather layer is enabled.
    pub fn labels(&self) -> &[LabelRequest] {
        &self.labels
    }
}

impl MapLayer for WeatherLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.gpu.ensure(ctx);

        if self.data_dirty {
            self.data_dirty = false;
            self.jobs.invalidate();
            if self.sigmets.is_empty() {
                self.meshes = None;
                self.labels.clear();
                self.pending = false;
            } else {
                let data = self.sigmets.clone();
                let theme = Arc::clone(&self.theme);
                self.pending = true;
                self.jobs
                    .submit(ctx.workers, move || build_artifacts(&data, &theme));
            }
        }

        if let Some(artifacts) = self.jobs.drain().into_iter().next_back() {
            self.origin_world = artifacts.origin_world;
            self.labels = artifacts.labels;
            self.meshes = Some(UploadedMeshes {
                fill: pipelines::upload_mesh(
                    ctx.device,
                    "strata sigmet fill",
                    &artifacts.fill.vertices,
                    &artifacts.fill.indices,
                ),
                outline: pipelines::upload_mesh(
                    ctx.device,
                    "strata sigmet outline",
                    &artifacts.outline.vertices,
                    &artifacts.outline.indices,
                ),
            });
            self.pending = false;
            tracing::debug!(labels = self.labels.len(), "sigmet tessellation uploaded");
        }

        if let (GpuState::Ready(gpu), Some(_)) = (&self.gpu, &self.meshes) {
            gpu.origin
                .update(ctx.queue, self.origin_world - ctx.camera.center());
        }
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, _ctx: &DrawCtx<'_>) {
        let (GpuState::Ready(gpu), Some(meshes)) = (&self.gpu, &self.meshes) else {
            return;
        };
        pass.set_bind_group(1, &gpu.origin.bind_group, &[]);
        if let Some(fill) = &meshes.fill {
            pass.set_pipeline(&gpu.hatch_pipeline);
            pass.set_vertex_buffer(0, fill.vertices.slice(..));
            pass.set_index_buffer(fill.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..fill.index_count, 0, 0..1);
        }
        if let Some(outline) = &meshes.outline {
            pass.set_pipeline(&gpu.outline_pipeline);
            pass.set_vertex_buffer(0, outline.vertices.slice(..));
            pass.set_index_buffer(outline.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..outline.index_count, 0, 0..1);
        }
    }

    fn wants_redraw(&self) -> bool {
        self.pending
    }
}

#[derive(Default)]
enum GpuState {
    #[default]
    Uninitialized,
    Ready(Gpu),
    Failed,
}

impl GpuState {
    fn ensure(&mut self, ctx: &PrepareCtx<'_>) {
        if !matches!(self, Self::Uninitialized) {
            return;
        }
        *self = match Gpu::new(ctx) {
            Ok(gpu) => Self::Ready(gpu),
            Err(e) => {
                tracing::error!(error = %e, "weather pipelines failed; layer disabled");
                Self::Failed
            }
        };
    }
}

struct Gpu {
    origin: OriginBinding,
    hatch_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
}

impl Gpu {
    fn new(ctx: &PrepareCtx<'_>) -> Result<Self, crate::error::RenderError> {
        let origin = OriginBinding::new(ctx.device, "strata weather origin");
        let hatch_module = pipelines::create_layer_module(ctx, WEATHER_SHADER)?;
        let line_module = pipelines::create_layer_module(ctx, LINE_DASH_SHADER)?;
        let hatch_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata sigmet hatch",
            &hatch_module,
            &origin.layout,
            &[FillVertex::layout()],
        );
        let outline_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata sigmet outline",
            &line_module,
            &origin.layout,
            &[LineVertex::layout()],
        );
        Ok(Self {
            origin,
            hatch_pipeline,
            outline_pipeline,
        })
    }
}

struct UploadedMeshes {
    fill: Option<GpuMesh>,
    outline: Option<GpuMesh>,
}

struct Artifacts {
    origin_world: DVec2,
    fill: FillMesh,
    outline: LineMesh,
    labels: Vec<LabelRequest>,
}

/// Pure worker job: tessellate and label the SIGMET set.
fn build_artifacts(sigmets: &[RenderSigmet], theme: &MapTheme) -> Artifacts {
    let rings: Vec<Vec<DVec2>> = sigmets.iter().map(|s| ring_to_world(&s.polygon)).collect();

    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in rings.iter().flatten() {
        min = min.min(*p);
        max = max.max(*p);
    }
    let origin_world = if rings.iter().all(Vec::is_empty) {
        DVec2::ZERO
    } else {
        (min + max) / 2.0
    };

    let color = theme.weather.sigmet;
    let stroke = StrokeStyleSpec {
        color: outline_color(theme),
        width_px: SIGMET_OUTLINE_WIDTH_PX,
        dash_px: Some(SIGMET_OUTLINE_DASH_PX),
    };
    let mut fill = FillMesh::default();
    let mut outline = LineMesh::default();
    let mut labels = Vec::new();
    for (index, (sigmet, ring)) in sigmets.iter().zip(&rings).enumerate() {
        tessellate_fill(ring, &[], origin_world, color, &mut fill);
        tessellate_ring_stroke(ring, origin_world, stroke, &mut outline);
        if let Some(label) = hazard_label(sigmet, ring, index, theme) {
            labels.push(label);
        }
    }
    Artifacts {
        origin_world,
        fill,
        outline,
        labels,
    }
}

fn outline_color(theme: &MapTheme) -> [f32; 4] {
    label_color_from_border(theme.weather.sigmet).map(|v| v * 0.85)
}

fn hazard_label(
    sigmet: &RenderSigmet,
    ring: &[DVec2],
    index: usize,
    theme: &MapTheme,
) -> Option<LabelRequest> {
    let text = sigmet.hazard_label.trim();
    if text.is_empty() {
        return None;
    }
    let anchor = pole_of_inaccessibility(ring, &[], precision_for(ring))?;
    Some(LabelRequest {
        text: text.into(),
        anchor: LabelAnchor::World(anchor),
        offset_px: Vec2::ZERO,
        placement: LabelPlacement::Center,
        size_px: 12.0,
        color: label_color_from_border(theme.weather.sigmet),
        priority: priority::SIGMET,
        min_zoom: SIGMET_LABEL_MIN_ZOOM,
        // SIGMETs carry no app-side id; the slot index is stable per dataset.
        id: index as u64 | LABEL_ID_NAMESPACE,
    })
}

fn precision_for(ring: &[DVec2]) -> f64 {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in ring {
        min = min.min(*p);
        max = max.max(*p);
    }
    let size = max - min;
    (size.x.min(size.y) / 64.0).max(1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::polylabel::signed_distance;

    fn build(sigmets: &[RenderSigmet]) -> Artifacts {
        build_artifacts(sigmets, &MapTheme::oldworld())
    }

    fn sigmet(label: &str) -> RenderSigmet {
        RenderSigmet {
            polygon: vec![[8.0, 49.0], [11.0, 49.0], [11.0, 51.0], [8.0, 51.0]],
            hazard_label: label.to_owned(),
        }
    }

    #[test]
    fn artifacts_contain_hatch_outline_and_label() {
        let artifacts = build(&[sigmet("SEV TURB")]);
        assert!(!artifacts.fill.indices.is_empty());
        assert!(!artifacts.outline.indices.is_empty());
        assert_eq!(artifacts.labels.len(), 1);
        let label = &artifacts.labels[0];
        assert_eq!(&*label.text, "SEV TURB");
        assert_eq!(label.priority, priority::SIGMET);
        assert!(label.id & LABEL_ID_NAMESPACE != 0);
        let LabelAnchor::World(anchor) = label.anchor else {
            panic!("sigmet labels are world-anchored");
        };
        let ring = ring_to_world(&sigmet("x").polygon);
        assert!(signed_distance(anchor, &ring, &[]) > 0.0);
    }

    #[test]
    fn outline_is_dashed() {
        let artifacts = build(&[sigmet("SEV ICE")]);
        let (on, off) = SIGMET_OUTLINE_DASH_PX;
        assert!(
            artifacts
                .outline
                .vertices
                .iter()
                .all(|v| v.dash_px == [on, off])
        );
    }

    #[test]
    fn blank_hazard_labels_are_dropped() {
        let artifacts = build(&[sigmet("   ")]);
        assert!(artifacts.labels.is_empty());
        assert!(!artifacts.fill.indices.is_empty(), "geometry still drawn");
    }

    /// The 5-minute weather refresh usually yields the same SIGMET set; an
    /// identical set must not re-tessellate.
    #[test]
    fn identical_sigmet_set_is_ignored() {
        let mut layer = WeatherLayer::default();
        assert!(layer.set_sigmets(vec![sigmet("SEV TURB")]));
        layer.data_dirty = false; // as after a prepare()
        assert!(!layer.set_sigmets(vec![sigmet("SEV TURB")]));
        assert!(!layer.data_dirty, "identical data must not re-tessellate");
        assert!(layer.set_sigmets(vec![sigmet("SEV ICE")]));
        assert!(layer.data_dirty);
    }
}

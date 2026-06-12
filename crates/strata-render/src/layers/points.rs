//! Point-feature symbols: instanced unit-scale meshes (one per
//! [`SymbolMeshKey`]) sized in logical px at draw time, zoom-gated per kind
//! for decluttering, with ident labels handed to the text system.
//!
//! One layer serves several toggle categories (obstacles, navaids, reporting
//! points, airports) plus METAR station dots that follow the Weather toggle;
//! instances are batched by kind so each category can be gated at draw time.

use crate::features::{PointKind, RenderPointFeature};
use crate::geo::world_from_lat_lon;
use crate::layer::{DrawCtx, LayerId, LayerToggles, MapLayer, PrepareCtx};
use crate::layers::pipelines::{self, OriginBinding};
use crate::layers::style::{flight_category_color, priority};
use crate::layers::symbols::{
    SymbolAtlas, SymbolInstance, SymbolMeshKey, SymbolVertex,
};
use crate::map_theme::MapTheme;
use crate::text::{LabelAnchor, LabelPlacement, LabelRequest};
use crate::workers::JobQueue;

use glam::{DVec2, Vec2};
use wgpu::util::DeviceExt as _;

use std::ops::Range;
use std::sync::Arc;

/// Namespace bit for point label ids (see airspace/weather namespaces).
const LABEL_ID_NAMESPACE: u64 = 1 << 61;

/// METAR dots sit right-top of the airport symbol (logical px, y down).
const WEATHER_DOT_OFFSET_PX: [f32; 2] = [10.0, -10.0];

/// Gap kept between a symbol's bottom edge and its label's top edge
/// (logical px, zoom-independent).
const LABEL_GAP_PX: f32 = 4.0;

/// Per-kind screen offset of the symbol mesh from the feature anchor.
/// Everything sits on its anchor except METAR dots (right-top of the
/// airport symbol so both stay visible).
fn symbol_offset_px(key: SymbolMeshKey) -> [f32; 2] {
    match key {
        SymbolMeshKey::WeatherStation => WEATHER_DOT_OFFSET_PX,
        _ => [0.0, 0.0],
    }
}

/// Screen offset from the feature anchor to the label's *top-center*
/// (labels use [`LabelPlacement::Below`]): straight below the symbol —
/// chart convention — clearing the symbol's own offset, its half-extent
/// (the unit mesh spans ±1 × `size_px`) and a fixed gap. Bigger symbols
/// (international airports) push their ident further down than small
/// triangles, all in zoom-independent logical px.
fn label_offset_px(key: SymbolMeshKey) -> Vec2 {
    let [sx, sy] = symbol_offset_px(key);
    Vec2::new(sx, sy + key.size_px() + LABEL_GAP_PX)
}

/// All point-feature symbols (airports, navaids, reporting points,
/// obstacles, METAR stations). One layer renders several toggle categories —
/// it filters by [`crate::features::PointKind::layer`] against
/// `ctx.layers` at draw time.
pub struct PointLayer {
    theme: Arc<MapTheme>,
    /// The symbol atlas (mesh colors) must be rebuilt for the new theme.
    atlas_dirty: bool,
    points: Vec<RenderPointFeature>,
    data_dirty: bool,
    pending: bool,
    jobs: JobQueue<Artifacts>,
    gpu: GpuState,
    instances: Option<InstanceData>,
    origin_world: DVec2,
    labels: Vec<(LayerId, LabelRequest)>,
}

impl Default for PointLayer {
    fn default() -> Self {
        Self::new(Arc::new(MapTheme::oldworld()))
    }
}

impl PointLayer {
    /// The toggle categories this layer renders (besides weather stations,
    /// which follow [`LayerId::Weather`]).
    pub const CATEGORIES: [LayerId; 4] = [
        LayerId::Obstacles,
        LayerId::Navaids,
        LayerId::ReportingPoints,
        LayerId::Airports,
    ];

    pub fn new(theme: Arc<MapTheme>) -> Self {
        Self {
            theme,
            atlas_dirty: false,
            points: Vec::new(),
            data_dirty: false,
            pending: false,
            jobs: JobQueue::new(),
            gpu: GpuState::default(),
            instances: None,
            origin_world: DVec2::ZERO,
            labels: Vec::new(),
        }
    }

    /// Switch the color theme: rebuilds the symbol atlas (mesh colors) and
    /// the instance/label artifacts (category tints, label text color).
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
        self.atlas_dirty = true;
        self.data_dirty = true;
    }

    /// Replace the full point set. Identical data is ignored — feed/weather
    /// refreshes that produce the same set must not pay for an instance
    /// rebuild; returns whether the set changed.
    pub fn set_points(&mut self, points: Vec<RenderPointFeature>) -> bool {
        if self.points == points {
            return false;
        }
        self.points = points;
        self.data_dirty = true;
        true
    }

    pub fn points(&self) -> &[RenderPointFeature] {
        &self.points
    }

    /// Ident labels for the currently enabled categories; the renderer
    /// forwards them to the [`crate::text::TextSystem`] each frame.
    pub fn visible_labels<'a>(
        &'a self,
        toggles: &'a LayerToggles,
    ) -> impl Iterator<Item = &'a LabelRequest> + 'a {
        self.labels
            .iter()
            .filter(|(layer, _)| toggles.enabled(*layer))
            .map(|(_, label)| label)
    }
}

impl MapLayer for PointLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.gpu.ensure(ctx, &self.theme.symbols);
        if self.atlas_dirty {
            // A fresh `ensure` above already used the new theme; rebuilding
            // an existing atlas swaps the mesh colors in place.
            if let GpuState::Ready(gpu) = &mut self.gpu {
                gpu.rebuild_atlas(ctx.device, &self.theme.symbols);
            }
            self.atlas_dirty = false;
        }

        if self.data_dirty {
            self.data_dirty = false;
            self.jobs.invalidate();
            if self.points.is_empty() {
                self.instances = None;
                self.labels.clear();
                self.pending = false;
            } else {
                let data = self.points.clone();
                let theme = Arc::clone(&self.theme);
                self.pending = true;
                // The rebuild model stays: instance building is cheap (no
                // tessellation) — the timing below keeps that claim honest.
                self.jobs.submit(ctx.workers, move || {
                    let started = std::time::Instant::now();
                    let artifacts = build_artifacts(&data, &theme);
                    tracing::debug!(
                        points = data.len(),
                        ms = started.elapsed().as_secs_f64() * 1e3,
                        "point artifacts built"
                    );
                    artifacts
                });
            }
        }

        if let Some(artifacts) = self.jobs.drain().into_iter().next_back() {
            self.origin_world = artifacts.origin_world;
            self.labels = artifacts.labels;
            self.instances = (!artifacts.instances.is_empty()).then(|| InstanceData {
                buffer: ctx
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("strata symbol instances"),
                        contents: bytemuck::cast_slice(&artifacts.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                batches: artifacts.batches,
            });
            self.pending = false;
            tracing::debug!(labels = self.labels.len(), "symbol instances uploaded");
        }

        if let (GpuState::Ready(gpu), Some(_)) = (&self.gpu, &self.instances) {
            gpu.origin
                .update(ctx.queue, self.origin_world - ctx.camera.center());
        }
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ctx: &DrawCtx<'_>) {
        let (GpuState::Ready(gpu), Some(instances)) = (&self.gpu, &self.instances) else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(1, &gpu.origin.bind_group, &[]);
        pass.set_vertex_buffer(0, gpu.mesh_vertices.slice(..));
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        pass.set_index_buffer(gpu.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
        let zoom = ctx.camera.zoom();
        for batch in &instances.batches {
            if !ctx.layers.enabled(batch.layer) || zoom < batch.min_zoom as f64 {
                continue;
            }
            let range = gpu.atlas.range(batch.key);
            pass.draw_indexed(
                range.indices.clone(),
                range.base_vertex,
                batch.instances.clone(),
            );
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
    Ready(Box<Gpu>),
    Failed,
}

impl GpuState {
    fn ensure(&mut self, ctx: &PrepareCtx<'_>, theme: &crate::map_theme::SymbolTheme) {
        if !matches!(self, Self::Uninitialized) {
            return;
        }
        *self = match Gpu::new(ctx, theme) {
            Ok(gpu) => Self::Ready(Box::new(gpu)),
            Err(e) => {
                tracing::error!(error = %e, "symbol pipeline failed; point layer disabled");
                Self::Failed
            }
        };
    }
}

struct Gpu {
    origin: OriginBinding,
    pipeline: wgpu::RenderPipeline,
    atlas: SymbolAtlas,
    mesh_vertices: wgpu::Buffer,
    mesh_indices: wgpu::Buffer,
}

impl Gpu {
    fn new(
        ctx: &PrepareCtx<'_>,
        theme: &crate::map_theme::SymbolTheme,
    ) -> Result<Self, crate::error::RenderError> {
        let origin = OriginBinding::new(ctx.device, "strata symbol origin");
        // `symbol.wgsl` is part of the embedded library.
        let module = ctx.shaders.create_module(ctx.device, "symbol.wgsl")?;
        let pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata symbols",
            &module,
            &origin.layout,
            &[SymbolVertex::layout(), SymbolInstance::layout()],
        );
        let atlas = SymbolAtlas::build(theme);
        let (mesh_vertices, mesh_indices) = upload_atlas(ctx.device, &atlas);
        Ok(Self {
            origin,
            pipeline,
            atlas,
            mesh_vertices,
            mesh_indices,
        })
    }

    /// Rebuild the symbol meshes with new theme colors (pipeline, origin and
    /// instance buffers stay).
    fn rebuild_atlas(&mut self, device: &wgpu::Device, theme: &crate::map_theme::SymbolTheme) {
        self.atlas = SymbolAtlas::build(theme);
        let (vertices, indices) = upload_atlas(device, &self.atlas);
        self.mesh_vertices = vertices;
        self.mesh_indices = indices;
    }
}

fn upload_atlas(device: &wgpu::Device, atlas: &SymbolAtlas) -> (wgpu::Buffer, wgpu::Buffer) {
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("strata symbol mesh vertices"),
        contents: bytemuck::cast_slice(&atlas.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("strata symbol mesh indices"),
        contents: bytemuck::cast_slice(&atlas.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vertices, indices)
}

struct InstanceData {
    buffer: wgpu::Buffer,
    batches: Vec<SymbolBatch>,
}

/// One instanced draw: a contiguous run of instances sharing a mesh.
#[derive(Debug, Clone, PartialEq)]
struct SymbolBatch {
    key: SymbolMeshKey,
    layer: LayerId,
    /// Symbols of this kind are hidden below this zoom (declutter).
    min_zoom: f32,
    instances: Range<u32>,
}

/// Worker-side instance build output.
struct Artifacts {
    origin_world: DVec2,
    instances: Vec<SymbolInstance>,
    batches: Vec<SymbolBatch>,
    labels: Vec<(LayerId, LabelRequest)>,
}

/// Pure worker job: project, sort by draw order, batch by mesh kind and
/// collect labels.
fn build_artifacts(points: &[RenderPointFeature], theme: &MapTheme) -> Artifacts {
    let mut projected: Vec<(SymbolMeshKey, DVec2, &RenderPointFeature)> = points
        .iter()
        .map(|p| {
            (
                SymbolMeshKey::from_kind(p.kind),
                world_from_lat_lon(p.position),
                p,
            )
        })
        .collect();
    projected.sort_by_key(|(key, _, _)| (key.draw_order(), key.index()));

    let origin_world = {
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        for (_, w, _) in &projected {
            min = min.min(*w);
            max = max.max(*w);
        }
        if projected.is_empty() {
            DVec2::ZERO
        } else {
            (min + max) / 2.0
        }
    };

    let mut instances = Vec::with_capacity(projected.len());
    let mut batches: Vec<SymbolBatch> = Vec::new();
    let mut labels = Vec::new();
    for (key, world, feature) in projected {
        let local = world - origin_world; // f64, keeps deep zoom crisp
        let color_mul = match feature.kind {
            PointKind::WeatherStation(category) => flight_category_color(&theme.weather, category),
            _ => [1.0, 1.0, 1.0, 1.0],
        };
        let index = instances.len() as u32;
        instances.push(SymbolInstance {
            anchor_local: [local.x as f32, local.y as f32],
            offset_px: symbol_offset_px(key),
            size_px: key.size_px(),
            rotation_rad: feature.rotation_deg.unwrap_or(0.0).to_radians(),
            color_mul,
        });
        match batches.last_mut() {
            Some(batch) if batch.key == key => batch.instances.end = index + 1,
            _ => batches.push(SymbolBatch {
                key,
                layer: key.category(),
                min_zoom: key.min_zoom(),
                instances: index..index + 1,
            }),
        }
        if let Some(label) = ident_label(feature, key, world, theme) {
            labels.push((key.category(), label));
        }
    }
    Artifacts {
        origin_world,
        instances,
        batches,
        labels,
    }
}

fn ident_label(
    feature: &RenderPointFeature,
    key: SymbolMeshKey,
    world: DVec2,
    theme: &MapTheme,
) -> Option<LabelRequest> {
    let text = feature.label.as_ref()?.trim();
    if text.is_empty() {
        return None;
    }
    let (label_priority, size_px) = match key.category() {
        LayerId::Airports => (priority::AIRPORT, 11.0),
        LayerId::Navaids => (priority::NAVAID, 10.0),
        LayerId::ReportingPoints => (priority::REPORTING_POINT, 10.0),
        LayerId::Obstacles => (priority::OBSTACLE, 9.0),
        _ => (priority::WEATHER_STATION, 10.0),
    };
    Some(LabelRequest {
        text: text.into(),
        anchor: LabelAnchor::World(world),
        // Hang the ident below the symbol so the text never covers it.
        offset_px: label_offset_px(key),
        placement: LabelPlacement::Below,
        size_px,
        color: theme.labels.text,
        priority: label_priority,
        // Labels declutter slightly later than their symbols.
        min_zoom: key.min_zoom() + 0.5,
        id: feature.id | LABEL_ID_NAMESPACE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::FlightCategoryColor;
    use crate::geo::LatLon;
    use crate::layer::LayerToggles;

    fn feature(id: u64, kind: PointKind, label: Option<&str>) -> RenderPointFeature {
        RenderPointFeature {
            id,
            kind,
            position: LatLon::new(50.0 + id as f64 * 0.1, 9.0),
            label: label.map(str::to_owned),
            rotation_deg: None,
        }
    }

    fn build(points: &[RenderPointFeature]) -> Artifacts {
        build_artifacts(points, &MapTheme::oldworld())
    }

    #[test]
    fn batches_follow_draw_order_with_weather_on_top() {
        let artifacts = build(&[
            feature(1, PointKind::WeatherStation(FlightCategoryColor::Ifr), None),
            feature(2, PointKind::AirportIntl, Some("EDDF")),
            feature(3, PointKind::Obstacle, None),
            feature(4, PointKind::Vor, Some("FFM 114.2")),
            feature(5, PointKind::AirportIntl, Some("EDDM")),
        ]);
        assert_eq!(artifacts.instances.len(), 5);
        let order: Vec<SymbolMeshKey> = artifacts.batches.iter().map(|b| b.key).collect();
        assert_eq!(
            order,
            vec![
                SymbolMeshKey::Obstacle,
                SymbolMeshKey::Vor,
                SymbolMeshKey::AirportIntl,
                SymbolMeshKey::WeatherStation,
            ]
        );
        // Equal-kind instances coalesce into one batch.
        let airports = &artifacts.batches[2];
        assert_eq!(airports.instances.len(), 2);
        // Batch ranges tile the instance buffer.
        let covered: usize = artifacts.batches.iter().map(|b| b.instances.len()).sum();
        assert_eq!(covered, artifacts.instances.len());
    }

    #[test]
    fn weather_station_instances_carry_category_tint_and_offset() {
        let artifacts = build(&[feature(
            1,
            PointKind::WeatherStation(FlightCategoryColor::Vfr),
            None,
        )]);
        let instance = &artifacts.instances[0];
        assert_eq!(instance.offset_px, WEATHER_DOT_OFFSET_PX);
        assert_eq!(
            instance.color_mul,
            flight_category_color(&MapTheme::oldworld().weather, FlightCategoryColor::Vfr)
        );
        assert_eq!(artifacts.batches[0].layer, LayerId::Weather);
    }

    #[test]
    fn labels_carry_category_priorities_and_namespaced_ids() {
        let artifacts = build(&[
            feature(2, PointKind::AirportIntl, Some("EDDF")),
            feature(4, PointKind::Vor, Some("FFM 114.2")),
            feature(6, PointKind::ReportingPointMandatory, Some("MIKE")),
            feature(8, PointKind::Obstacle, Some("  ")), // blank → dropped
        ]);
        let by_text: Vec<(&str, u8)> = artifacts
            .labels
            .iter()
            .map(|(_, l)| (&*l.text, l.priority))
            .collect();
        assert!(by_text.contains(&("EDDF", priority::AIRPORT)));
        assert!(by_text.contains(&("FFM 114.2", priority::NAVAID)));
        assert!(by_text.contains(&("MIKE", priority::REPORTING_POINT)));
        assert_eq!(artifacts.labels.len(), 3, "blank labels are dropped");
        for (_, label) in &artifacts.labels {
            assert!(label.id & LABEL_ID_NAMESPACE != 0);
        }
    }

    #[test]
    fn visible_labels_filter_by_layer_toggle() {
        let layer = PointLayer {
            labels: label_fixtures(),
            ..PointLayer::default()
        };
        let mut toggles = LayerToggles::all_enabled();
        toggles.set(LayerId::Navaids, false);
        let visible: Vec<&str> = layer
            .visible_labels(&toggles)
            .map(|l| &*l.text)
            .collect();
        assert_eq!(visible, vec!["EDDF"]);
    }

    fn label_fixtures() -> Vec<(LayerId, LabelRequest)> {
        vec![
            (
                LayerId::Airports,
                LabelRequest {
                    text: "EDDF".into(),
                    anchor: LabelAnchor::World(DVec2::ZERO),
                    offset_px: label_offset_px(SymbolMeshKey::AirportIntl),
                    placement: LabelPlacement::Below,
                    size_px: 11.0,
                    color: [1.0; 4],
                    priority: priority::AIRPORT,
                    min_zoom: 5.5,
                    id: 1,
                },
            ),
            (
                LayerId::Navaids,
                LabelRequest {
                    text: "FFM".into(),
                    anchor: LabelAnchor::World(DVec2::ZERO),
                    offset_px: label_offset_px(SymbolMeshKey::Vor),
                    placement: LabelPlacement::Below,
                    size_px: 10.0,
                    color: [1.0; 4],
                    priority: priority::NAVAID,
                    min_zoom: 7.0,
                    id: 2,
                },
            ),
        ]
    }

    /// Every queued ident hangs strictly below its symbol: `Below`
    /// placement plus a downward offset clearing the symbol's bottom edge
    /// (its own screen offset + half-extent) by the fixed gap.
    #[test]
    fn labels_hang_below_their_symbol() {
        for key in SymbolMeshKey::ALL {
            let offset = label_offset_px(key);
            let [sx, sy] = symbol_offset_px(key);
            assert_eq!(offset.x, sx, "{key:?}: label must stay centered on its symbol");
            assert_eq!(
                offset.y,
                sy + key.size_px() + LABEL_GAP_PX,
                "{key:?}: label top must clear the symbol bottom by the gap"
            );
        }
        let artifacts = build(&[
            feature(2, PointKind::AirportIntl, Some("EDDF")),
            feature(4, PointKind::ReportingPointMandatory, Some("MIKE")),
        ]);
        for (_, label) in &artifacts.labels {
            assert_eq!(label.placement, LabelPlacement::Below);
        }
        let offset_of = |text: &str| {
            artifacts
                .labels
                .iter()
                .find(|(_, l)| &*l.text == text)
                .map(|(_, l)| l.offset_px)
                .expect("label queued")
        };
        // Per-kind: the big airport disc pushes its ident further down than
        // the small reporting-point triangle.
        assert_eq!(offset_of("EDDF"), label_offset_px(SymbolMeshKey::AirportIntl));
        assert!(offset_of("EDDF").y > offset_of("MIKE").y);
    }

    /// The METAR dot (right-top of the airport symbol) must sit fully above
    /// an airport ident hung below the symbol — the dot may never cover the
    /// label.
    #[test]
    fn weather_dot_clears_airport_labels() {
        let dot_bottom = WEATHER_DOT_OFFSET_PX[1] + SymbolMeshKey::WeatherStation.size_px();
        for key in [
            SymbolMeshKey::AirportIntl,
            SymbolMeshKey::AirportRegional,
            SymbolMeshKey::Airfield,
            SymbolMeshKey::GliderSite,
            SymbolMeshKey::Heliport,
            SymbolMeshKey::UltraLight,
        ] {
            assert!(
                dot_bottom < label_offset_px(key).y,
                "{key:?}: METAR dot (bottom {dot_bottom}px) overlaps the label \
                 top at {}px",
                label_offset_px(key).y
            );
        }
        // And a weather-station label (if one is ever fed) clears its dot.
        let wx = label_offset_px(SymbolMeshKey::WeatherStation);
        assert_eq!(wx.x, WEATHER_DOT_OFFSET_PX[0]);
        assert!(wx.y >= dot_bottom + LABEL_GAP_PX);
    }

    /// Re-feeding the same set (weather refresh with unchanged data, camera
    /// settle inside the fed bbox) must not rebuild the instance buffer.
    #[test]
    fn identical_point_set_is_ignored() {
        let mut layer = PointLayer::default();
        let points = vec![feature(1, PointKind::AirportIntl, Some("EDDF"))];
        assert!(layer.set_points(points.clone()));
        layer.data_dirty = false; // as after a prepare()
        assert!(!layer.set_points(points));
        assert!(!layer.data_dirty, "identical data must not rebuild");
        assert!(layer.set_points(vec![feature(2, PointKind::Vor, None)]));
        assert!(layer.data_dirty);
    }

    /// Instances pack the feature's heading as radians; missing rotation
    /// packs as 0 (un-rotated — the canonical mesh orientation).
    #[test]
    fn rotation_packs_as_radians_with_zero_default() {
        let mut rotated = feature(1, PointKind::AirportIntl, None);
        rotated.rotation_deg = Some(90.0);
        let artifacts = build(&[rotated, feature(2, PointKind::AirportIntl, None)]);
        assert!((artifacts.instances[0].rotation_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(artifacts.instances[1].rotation_rad, 0.0);
    }

    /// The METAR dot's screen offset is applied after rotation in the shader
    /// and must be packed un-rotated — a (hypothetically) rotated weather
    /// station keeps its dot right-top of the airport symbol.
    #[test]
    fn weather_dot_offset_is_unaffected_by_rotation() {
        let mut wx = feature(1, PointKind::WeatherStation(FlightCategoryColor::Vfr), None);
        wx.rotation_deg = Some(135.0);
        let artifacts = build(&[wx]);
        let instance = &artifacts.instances[0];
        assert_eq!(instance.offset_px, WEATHER_DOT_OFFSET_PX);
        // The offset itself never rotates; only the unit mesh does.
        assert!((instance.rotation_rad - 135.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn anchors_are_relative_to_the_dataset_origin() {
        let artifacts = build(&[
            feature(0, PointKind::AirportIntl, None),
            feature(10, PointKind::AirportIntl, None),
        ]);
        // Origin is the bbox center → anchors are symmetric and small.
        let ys: Vec<f32> = artifacts
            .instances
            .iter()
            .map(|i| i.anchor_local[1])
            .collect();
        assert!((ys[0] + ys[1]).abs() < 1e-6);
        assert!(ys.iter().all(|y| y.abs() < 0.01));
    }
}

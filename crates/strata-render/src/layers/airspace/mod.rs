//! Class-correct styled airspace polygons: translucent lyon fills (with
//! holes) plus screen-stable dashed borders, with vertical-band labels at
//! each polygon's pole of inaccessibility.
//!
//! ## Incremental pipeline
//!
//! Tessellation results persist in a per-feature [`cache::MeshCache`] (LRU
//! byte budget), so a new set from the app's feature feed only pays for the
//! features it has never seen (or whose theme generation changed):
//!
//! 1. **Diff** ([`AirspaceLayer::plan_set`]): look every feature up in the
//!    cache; misses are split into small batches submitted as separate
//!    worker jobs so a cold set tessellates on all pool threads in parallel.
//! 2. **Progressive assembly**: draw buffers are concatenated from whatever
//!    is already cached — cached features appear on the very next frame,
//!    and each draining batch triggers a re-assembly, so features pop in
//!    progressively instead of all-or-nothing after a full retessellation.
//! 3. Cached meshes are local to a per-feature origin; assembly rebases
//!    them to a common set origin (see [`assemble`]) and the per-frame
//!    `origin − camera_center` uniform keeps deep-zoom precision as before.
//!
//! The [`crate::workers::JobQueue`] generation semantics are unchanged: a
//! new set or theme invalidates in-flight batches before they start.

mod assemble;
mod build;
mod cache;

pub use cache::DEFAULT_AIRSPACE_MESH_CACHE_BYTES;

use self::build::TessBatch;
use self::cache::{CacheKey, FeatureMesh, MeshCache};
use crate::features::RenderAirspace;
use crate::layer::{DrawCtx, MapLayer, PrepareCtx};
use crate::layers::pipelines::{
    self, FILL_AIRSPACE_SHADER, GpuMesh, LINE_DASH_SHADER, OriginBinding,
};
use crate::layers::tess::{FillVertex, LineVertex};
use crate::map_theme::MapTheme;
use crate::text::LabelRequest;
use crate::workers::{JobQueue, WorkerPool};

use glam::DVec2;

use std::sync::Arc;
use std::time::Instant;

/// Vertical-band labels appear from this camera zoom.
pub const AIRSPACE_LABEL_MIN_ZOOM: f32 = 9.0;

/// Namespace bit for airspace label ids (keeps text-shaping caches from
/// colliding with point-feature labels).
const LABEL_ID_NAMESPACE: u64 = 1 << 62;

/// Cache-missing features per tessellation job: small enough that a cold
/// Germany-sized set (~750 airspaces) fans out across every worker thread,
/// large enough that per-job overhead stays negligible.
const TESS_BATCH_FEATURES: usize = 24;

/// One feature of the current set, in draw order. `mesh` is `None` while
/// its tessellation batch is still in flight.
struct SetEntry {
    id: u64,
    fingerprint: u64,
    mesh: Option<Arc<FeatureMesh>>,
}

/// Class-correct styled airspace polygons with vertical-band labels.
pub struct AirspaceLayer {
    theme: Arc<MapTheme>,
    /// Bumped by [`Self::set_theme`]; part of every cache key, so a theme
    /// switch misses the cache naturally and stale generations get swept.
    theme_generation: u64,
    airspaces: Vec<RenderAirspace>,
    data_dirty: bool,
    cache: MeshCache,
    /// The current set with per-feature meshes as they become available.
    current: Vec<SetEntry>,
    /// Common rebase origin of the current set (stable across progressive
    /// re-assemblies; the draw uniform is `origin − camera_center`).
    origin_world: DVec2,
    /// Draw buffers need re-concatenation (new set or a batch landed).
    assembly_dirty: bool,
    /// Tessellation batches still in flight for the current set.
    pending_batches: usize,
    jobs: JobQueue<TessBatch>,
    gpu: GpuState,
    meshes: Option<UploadedMeshes>,
    labels: Vec<LabelRequest>,
}

impl Default for AirspaceLayer {
    fn default() -> Self {
        Self::new(Arc::new(MapTheme::oldworld()))
    }
}

impl AirspaceLayer {
    pub fn new(theme: Arc<MapTheme>) -> Self {
        Self::with_cache_budget(theme, DEFAULT_AIRSPACE_MESH_CACHE_BYTES)
    }

    /// `cache_budget_bytes` bounds the persistent feature-mesh cache
    /// (vertex + index bytes, LRU eviction) — see
    /// [`crate::renderer::RendererConfig::airspace_mesh_cache_bytes`].
    pub fn with_cache_budget(theme: Arc<MapTheme>, cache_budget_bytes: usize) -> Self {
        Self {
            theme,
            theme_generation: 0,
            airspaces: Vec::new(),
            data_dirty: false,
            cache: MeshCache::new(cache_budget_bytes),
            current: Vec::new(),
            origin_world: DVec2::ZERO,
            assembly_dirty: false,
            pending_batches: 0,
            jobs: JobQueue::new(),
            gpu: GpuState::default(),
            meshes: None,
            labels: Vec::new(),
        }
    }

    /// Switch the color theme: bumps the cache generation so the current
    /// set re-tessellates with the new fill/border colors (in parallel
    /// batches; the old generation is swept from the cache).
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
        self.theme_generation += 1;
        self.data_dirty = true;
    }

    /// Replace the full airspace set. Identical data is ignored — a feature
    /// feed that settles on the same set must not pay even for the diff;
    /// returns whether the set changed. Overlapping sets (pan/zoom within
    /// the same region) only tessellate the features not already cached.
    pub fn set_airspaces(&mut self, airspaces: Vec<RenderAirspace>) -> bool {
        if self.airspaces == airspaces {
            return false;
        }
        self.airspaces = airspaces;
        self.data_dirty = true;
        true
    }

    pub fn airspaces(&self) -> &[RenderAirspace] {
        &self.airspaces
    }

    /// Vertical-band labels for the currently assembled features; the
    /// renderer forwards them to the [`crate::text::TextSystem`] each frame
    /// while the airspace layer is enabled.
    pub fn labels(&self) -> &[LabelRequest] {
        &self.labels
    }

    /// Diff the new set against the mesh cache and submit only the missing
    /// features, in batches, as separate pool jobs.
    fn plan_set(&mut self, workers: &WorkerPool) {
        self.origin_world = assemble::set_origin(&self.airspaces);
        let mut missing: Vec<usize> = Vec::new();
        self.current = self
            .airspaces
            .iter()
            .enumerate()
            .map(|(index, airspace)| {
                let fingerprint = build::fingerprint(airspace);
                let key = CacheKey {
                    id: airspace.id,
                    theme_generation: self.theme_generation,
                };
                let mesh = self.cache.get(key, fingerprint);
                if mesh.is_none() {
                    missing.push(index);
                }
                SetEntry {
                    id: airspace.id,
                    fingerprint,
                    mesh,
                }
            })
            .collect();

        for chunk in missing.chunks(TESS_BATCH_FEATURES) {
            let features: Vec<(usize, RenderAirspace)> = chunk
                .iter()
                .map(|&index| (index, self.airspaces[index].clone()))
                .collect();
            let theme = Arc::clone(&self.theme);
            self.jobs
                .submit(workers, move || build::build_batch(&features, &theme));
            self.pending_batches += 1;
        }
        self.assembly_dirty = true;

        let stats = self.cache.take_stats();
        tracing::debug!(
            features = self.airspaces.len(),
            hits = stats.hits,
            misses = stats.misses,
            evictions = stats.evictions,
            resident_bytes = self.cache.resident_bytes(),
            resident_entries = self.cache.entry_count(),
            batches = self.pending_batches,
            "airspace set diffed against mesh cache"
        );
    }

    /// Drain finished tessellation batches into the cache and the current
    /// set. Returns whether anything landed (stale generations were already
    /// filtered by the queue).
    fn drain_batches(&mut self) -> bool {
        let mut landed = false;
        for batch in self.jobs.drain() {
            self.pending_batches = self.pending_batches.saturating_sub(1);
            for built in batch.built {
                self.cache.insert(
                    CacheKey {
                        id: built.id,
                        theme_generation: self.theme_generation,
                    },
                    Arc::clone(&built.mesh),
                );
                if let Some(entry) = self.current.get_mut(built.set_index)
                    && entry.id == built.id
                    && entry.fingerprint == built.mesh.fingerprint
                {
                    entry.mesh = Some(built.mesh);
                }
            }
            landed = true;
        }
        if landed {
            self.assembly_dirty = true;
        }
        landed
    }

    /// Concatenate every already-available feature mesh of the current set
    /// (set order) relative to the set origin.
    fn assemble_current(&self) -> assemble::Assembled {
        let meshes: Vec<&FeatureMesh> = self
            .current
            .iter()
            .filter_map(|entry| entry.mesh.as_deref())
            .collect();
        assemble::assemble(self.origin_world, &meshes)
    }
}

impl MapLayer for AirspaceLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.gpu.ensure(ctx);

        if self.data_dirty {
            self.data_dirty = false;
            self.jobs.invalidate();
            self.pending_batches = 0;
            self.cache.sweep_stale(self.theme_generation);
            if self.airspaces.is_empty() {
                self.current.clear();
                self.meshes = None;
                self.labels.clear();
                self.assembly_dirty = false;
            } else {
                self.plan_set(ctx.workers);
            }
        }

        self.drain_batches();

        if self.assembly_dirty {
            self.assembly_dirty = false;
            let started = Instant::now();
            let assembled = self.assemble_current();
            self.labels = assembled.labels;
            self.meshes = Some(UploadedMeshes {
                fill: pipelines::upload_mesh(
                    ctx.device,
                    "strata airspace fill",
                    &assembled.fill.vertices,
                    &assembled.fill.indices,
                ),
                border: pipelines::upload_mesh(
                    ctx.device,
                    "strata airspace border",
                    &assembled.border.vertices,
                    &assembled.border.indices,
                ),
            });
            tracing::debug!(
                features = assembled.features,
                set_size = self.current.len(),
                fill_vertices = assembled.fill.vertices.len(),
                border_vertices = assembled.border.vertices.len(),
                pending_batches = self.pending_batches,
                ms = started.elapsed().as_secs_f64() * 1e3,
                "airspace assembly uploaded"
            );
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
            pass.set_pipeline(&gpu.fill_pipeline);
            pass.set_vertex_buffer(0, fill.vertices.slice(..));
            pass.set_index_buffer(fill.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..fill.index_count, 0, 0..1);
        }
        if let Some(border) = &meshes.border {
            pass.set_pipeline(&gpu.border_pipeline);
            pass.set_vertex_buffer(0, border.vertices.slice(..));
            pass.set_index_buffer(border.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..border.index_count, 0, 0..1);
        }
    }

    fn wants_redraw(&self) -> bool {
        self.data_dirty || self.assembly_dirty || self.pending_batches > 0
    }
}

#[derive(Default)]
enum GpuState {
    #[default]
    Uninitialized,
    Ready(Gpu),
    /// Pipeline creation failed (broken shader include); logged once, layer
    /// stays inert instead of retrying every frame.
    Failed,
}

struct Gpu {
    origin: OriginBinding,
    fill_pipeline: wgpu::RenderPipeline,
    border_pipeline: wgpu::RenderPipeline,
}

impl GpuState {
    fn ensure(&mut self, ctx: &PrepareCtx<'_>) {
        if !matches!(self, Self::Uninitialized) {
            return;
        }
        *self = match Gpu::new(ctx) {
            Ok(gpu) => Self::Ready(gpu),
            Err(e) => {
                tracing::error!(error = %e, "airspace pipelines failed; layer disabled");
                Self::Failed
            }
        };
    }
}

impl Gpu {
    fn new(ctx: &PrepareCtx<'_>) -> Result<Self, crate::error::RenderError> {
        let origin = OriginBinding::new(ctx.device, "strata airspace origin");
        let fill_module = pipelines::create_layer_module(ctx, FILL_AIRSPACE_SHADER)?;
        let line_module = pipelines::create_layer_module(ctx, LINE_DASH_SHADER)?;
        let fill_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata airspace fill",
            &fill_module,
            &origin.layout,
            &[FillVertex::layout()],
        );
        let border_pipeline = pipelines::create_layer_pipeline(
            ctx,
            "strata airspace border",
            &line_module,
            &origin.layout,
            &[LineVertex::layout()],
        );
        Ok(Self {
            origin,
            fill_pipeline,
            border_pipeline,
        })
    }
}

struct UploadedMeshes {
    fill: Option<GpuMesh>,
    border: Option<GpuMesh>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::features::AirspaceStyleKey;

    use std::time::Duration;

    pub(crate) fn ctr(id: u64) -> RenderAirspace {
        RenderAirspace {
            id,
            style: AirspaceStyleKey::Ctr,
            polygon: vec![[10.0, 50.0], [10.2, 50.0], [10.2, 50.1], [10.0, 50.1]],
            holes: vec![],
            lower_label: "GND".into(),
            upper_label: "2500 MSL".into(),
            name: "FRANKFURT CTR".into(),
        }
    }

    /// `n` distinct synthetic airspaces spread over a Germany-sized grid,
    /// each a 24-gon (realistic ring sizes for CTR-style features).
    pub(crate) fn synthetic_airspaces(n: usize) -> Vec<RenderAirspace> {
        (0..n as u64)
            .map(|id| {
                let cx = 6.0 + (id % 30) as f64 * 0.3;
                let cy = 47.0 + (id / 30) as f64 * 0.3;
                let polygon = (0..24)
                    .map(|k| {
                        let a = k as f64 / 24.0 * std::f64::consts::TAU;
                        [cx + 0.1 * a.cos(), cy + 0.07 * a.sin()]
                    })
                    .collect();
                RenderAirspace {
                    id,
                    style: AirspaceStyleKey::Ctr,
                    polygon,
                    holes: vec![],
                    lower_label: "GND".into(),
                    upper_label: "2500 MSL".into(),
                    name: format!("SYNTH {id}"),
                }
            })
            .collect()
    }

    /// Drive the worker side without a GPU: drain until all batches landed.
    fn settle(layer: &mut AirspaceLayer) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while layer.pending_batches > 0 {
            layer.drain_batches();
            assert!(
                Instant::now() < deadline,
                "tessellation batches never landed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Plan a set the way `prepare` does (without the GPU parts).
    fn plan(layer: &mut AirspaceLayer, workers: &WorkerPool, set: Vec<RenderAirspace>) {
        if layer.set_airspaces(set) {
            layer.data_dirty = false;
            layer.jobs.invalidate();
            layer.pending_batches = 0;
            layer.cache.sweep_stale(layer.theme_generation);
            layer.plan_set(workers);
        }
    }

    /// Re-feeding the same set (every camera settle inside the already-fed
    /// bbox) must not even pay for the diff.
    #[test]
    fn identical_airspace_set_is_ignored() {
        let mut layer = AirspaceLayer::default();
        assert!(layer.set_airspaces(vec![ctr(1), ctr(2)]));
        layer.data_dirty = false; // as after a prepare()
        assert!(!layer.set_airspaces(vec![ctr(1), ctr(2)]));
        assert!(!layer.data_dirty, "identical data must not re-plan");
        assert!(layer.set_airspaces(vec![ctr(3)]));
        assert!(layer.data_dirty);
    }

    /// Job-count probe: a fully cached set submits **zero** tessellation
    /// batches; a half-new set only tessellates the new half.
    #[test]
    fn cache_hits_skip_tessellation_jobs() {
        let workers = WorkerPool::new(2);
        let mut layer = AirspaceLayer::default();

        plan(&mut layer, &workers, synthetic_airspaces(50));
        let cold_batches = layer.pending_batches;
        assert_eq!(cold_batches, 50usize.div_ceil(TESS_BATCH_FEATURES));
        settle(&mut layer);

        // Same 50 features, different Vec (e.g. bbox re-query): no jobs.
        plan(&mut layer, &workers, synthetic_airspaces(50));
        assert_eq!(layer.pending_batches, 0, "warm set must not tessellate");
        assert!(layer.current.iter().all(|e| e.mesh.is_some()));

        // Pan-style overlap: 50 cached + 25 new → only the new ones batch.
        plan(&mut layer, &workers, synthetic_airspaces(75));
        assert_eq!(layer.pending_batches, 25usize.div_ceil(TESS_BATCH_FEATURES));
        settle(&mut layer);
    }

    /// Progressive assembly: with one feature cached and the other's batch
    /// still pending, the assembled buffers (and labels) contain exactly
    /// the cached feature — it is drawable before the set completes.
    #[test]
    fn partial_set_assembles_from_cache_while_batches_pend() {
        let workers = WorkerPool::new(2);
        let mut layer = AirspaceLayer::default();

        // Warm the cache with feature 1 only.
        plan(&mut layer, &workers, vec![ctr(1)]);
        settle(&mut layer);

        // Gate the pool so feature 2's batch cannot finish.
        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        for _ in 0..workers.thread_count() {
            let rx = gate_rx.clone();
            workers.execute(move || {
                let _ = rx.recv();
            });
        }

        plan(&mut layer, &workers, vec![ctr(1), ctr(2)]);
        assert_eq!(layer.pending_batches, 1);
        assert!(layer.assembly_dirty, "cached subset assembles immediately");

        let partial = layer.assemble_current();
        assert_eq!(partial.features, 1, "only the cached feature is drawable");
        assert_eq!(partial.labels.len(), 1);
        assert_eq!(partial.labels[0].id, 1 | LABEL_ID_NAMESPACE);
        assert!(!partial.fill.vertices.is_empty());
        assert!(
            layer.wants_redraw(),
            "pending batches keep frames scheduled"
        );

        // Release the workers; the missing feature completes the set.
        drop(gate_tx);
        settle(&mut layer);
        let full = layer.assemble_current();
        assert_eq!(full.features, 2);
        assert_eq!(full.labels.len(), 2);
        layer.assembly_dirty = false;
        assert!(!layer.wants_redraw(), "settled layer goes idle");
    }

    /// A theme switch bumps the generation: the same set re-tessellates
    /// (cache misses) and the old generation is swept from the cache.
    #[test]
    fn theme_switch_invalidates_cached_generation() {
        let workers = WorkerPool::new(2);
        let mut layer = AirspaceLayer::default();
        plan(&mut layer, &workers, synthetic_airspaces(30));
        settle(&mut layer);
        let resident = layer.cache.resident_bytes();
        assert!(resident > 0);

        layer.set_theme(Arc::new(MapTheme::oldworld()));
        assert!(layer.data_dirty);
        // Re-plan the *same* airspaces under the new generation.
        layer.data_dirty = false;
        layer.jobs.invalidate();
        layer.pending_batches = 0;
        layer.cache.sweep_stale(layer.theme_generation);
        layer.plan_set(&workers);
        assert_eq!(
            layer.pending_batches,
            30usize.div_ceil(TESS_BATCH_FEATURES),
            "every feature must re-tessellate under the new theme generation"
        );
        settle(&mut layer);
        // Old generation swept: resident bytes hold one generation, not two.
        assert_eq!(layer.cache.entry_count(), 30);
    }

    /// The band label is part of the cached mesh: a cache hit serves the
    /// identical label without recomputation.
    #[test]
    fn labels_are_cached_with_their_mesh() {
        let workers = WorkerPool::new(2);
        let mut layer = AirspaceLayer::default();
        plan(&mut layer, &workers, vec![ctr(7)]);
        settle(&mut layer);
        let first = layer.current[0].mesh.clone().expect("mesh landed");
        let label = first.label.clone().expect("band label");
        assert_eq!(&*label.text, "FRANKFURT CTR\n2500 MSL / GND");

        // Warm re-plan: the very same Arc comes back from the cache.
        plan(&mut layer, &workers, vec![ctr(7), ctr(8)]);
        let warm = layer.current[0].mesh.clone().expect("cache hit");
        assert!(Arc::ptr_eq(&first, &warm), "hit must reuse the cached mesh");
        assert_eq!(warm.label.as_ref(), Some(&label));
        settle(&mut layer);
    }

    /// Bench-ish: ~750 synthetic airspaces cold (parallel batches) vs warm
    /// (pure cache + assembly). Timings print to stderr; run with
    /// `--release` for representative numbers.
    #[test]
    fn cold_vs_warm_timing_750_airspaces() {
        let workers = WorkerPool::new(
            std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8)),
        );
        let mut layer = AirspaceLayer::default();
        let set = synthetic_airspaces(750);

        let cold_start = Instant::now();
        plan(&mut layer, &workers, set.clone());
        settle(&mut layer);
        let assembled = layer.assemble_current();
        let cold = cold_start.elapsed();
        assert_eq!(assembled.features, 750);

        // Warm: same features, new Vec identity (bbox re-query after pan).
        let mut warm_layer = AirspaceLayer::default();
        std::mem::swap(&mut warm_layer.cache, &mut layer.cache);
        let warm_start = Instant::now();
        plan(&mut warm_layer, &workers, set);
        settle(&mut warm_layer);
        let assembled = warm_layer.assemble_current();
        let warm = warm_start.elapsed();
        assert_eq!(assembled.features, 750);
        assert_eq!(warm_layer.pending_batches, 0);

        eprintln!(
            "airspace 750 features: cold {:.1} ms (tessellation, {} threads), \
             warm {:.1} ms (cache diff + assembly), resident {:.1} MiB",
            cold.as_secs_f64() * 1e3,
            workers.thread_count(),
            warm.as_secs_f64() * 1e3,
            warm_layer.cache.resident_bytes() as f64 / (1024.0 * 1024.0),
        );
        assert!(warm < cold, "warm path must beat cold tessellation");
    }
}

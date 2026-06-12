//! Vector basemap layer: MVT tiles (geozero) tessellated with lyon on the
//! worker pool, per-tile GPU buffers in an LRU cache, ancestor fallback and
//! ~150 ms fade-in so zooming never pops or shows holes.
//!
//! Per frame (`prepare`): drain finished worker decodes → upload meshes →
//! request missing coverage tiles → plan draws (wanted tile, else nearest
//! ready ancestor; fading tiles stacked over an opaque base) → write per-draw
//! uniforms. `draw` replays the plan: every draw is scissored to its wanted
//! tile's screen rect, which clips both MVT buffer spill and oversized
//! ancestor meshes.
//!
//! Tiles *absent at the source* (mid-ingest archive, transient read error)
//! are distinct from tiles that *decoded to nothing*: only the latter are
//! authoritative emptiness. Absent tiles keep the ancestor fallback alive —
//! a deeply zoomed view over an incomplete archive stays covered by the
//! nearest available ancestor, never blank.

mod decode;
mod fallback;
mod labels;
mod pipeline;
pub mod style;
mod tess;

use self::decode::{DecodedTile, TileOutcome};
use self::fallback::TileReadiness;
use self::labels::LabelSpec;
use self::pipeline::{BasemapGpu, GpuMesh, TileUniform};
use crate::camera::Camera;
use crate::layer::{DrawCtx, MapLayer, PrepareCtx};
use crate::map_theme::{BasemapTheme, MapTheme};
use crate::text::LabelRequest;
use crate::tiles::{TileId, TileSource, viewport_coverage};
use crate::workers::JobQueue;

use lru::LruCache;
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// GPU tile cache budget (meshes, including negative entries).
const TILE_BUDGET: NonZeroUsize = NonZeroUsize::new(400).expect("non-zero budget");

/// Fade-in duration for freshly decoded tiles.
const FADE: Duration = Duration::from_millis(150);

/// Negative cache entries are re-checked after this. A re-check is one cheap
/// SQLite point query, and it lets tiles written by a concurrent
/// `strata-ingest basemap` (or dropped by a transient read error) appear
/// without restarting the app.
const NEGATIVE_TTL: Duration = Duration::from_secs(15);

/// What a finished tile job left in the cache.
enum TileContent {
    /// Drawable geometry, resident on the GPU.
    Mesh(GpuMesh),
    /// Decoded successfully but contains no drawable geometry —
    /// *authoritative* emptiness: nothing is drawn here and no ancestor
    /// stands in (it would only paint stale coarse geometry over a truly
    /// empty area).
    Empty,
    /// Absent at the source (or undecodable). NOT authoritative emptiness:
    /// a mid-ingest archive or an over-deep request simply has no row yet,
    /// so the ancestor fallback must keep covering this area. Re-checked
    /// after [`NEGATIVE_TTL`].
    Absent,
}

impl TileContent {
    fn mesh(&self) -> Option<&GpuMesh> {
        match self {
            Self::Mesh(mesh) => Some(mesh),
            Self::Empty | Self::Absent => None,
        }
    }
}

struct TileEntry {
    content: TileContent,
    labels: Vec<LabelSpec>,
    loaded_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct Scissor {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct DrawCmd {
    mesh: TileId,
    uniform_index: u32,
    scissor: Scissor,
}

/// Vector basemap layer.
pub struct BasemapLayer {
    source: Option<Arc<dyn TileSource>>,
    max_source_zoom: u8,
    /// Zoom-selection bias (see [`crate::tiles::display_level`]).
    detail_bias: f64,
    /// Active color theme; shared with the worker jobs (tiles are
    /// tessellated with the theme that was active when they were requested,
    /// see [`set_theme`](Self::set_theme)).
    theme: Arc<MapTheme>,
    jobs: JobQueue<DecodedTile>,
    in_flight: FxHashSet<TileId>,
    /// Tiles of the current viewport coverage, shared with queued worker
    /// jobs: a job whose tile left the coverage before it started bails out
    /// (one hash lookup) instead of decoding a tile nobody wants anymore.
    wanted: Arc<Mutex<FxHashSet<TileId>>>,
    cache: LruCache<TileId, TileEntry>,
    gpu: Option<BasemapGpu>,
    gpu_init_failed: bool,
    draw_list: Vec<DrawCmd>,
    labels: Vec<LabelRequest>,
    wants_redraw: bool,
}

impl BasemapLayer {
    /// `fallback_max_zoom` caps source-tile selection only when the source
    /// itself does not report its depth ([`TileSource::max_zoom`], e.g. the
    /// MBTiles `metadata.maxzoom` row). Deeper views overzoom.
    pub fn new(
        source: Option<Arc<dyn TileSource>>,
        fallback_max_zoom: u8,
        detail_bias: f64,
        theme: Arc<MapTheme>,
    ) -> Self {
        let max_source_zoom = match source.as_ref().and_then(|s| s.max_zoom()) {
            Some(reported) => {
                if reported != fallback_max_zoom {
                    tracing::info!(
                        reported,
                        fallback_max_zoom,
                        "basemap source reports its own max zoom"
                    );
                }
                reported
            }
            None => fallback_max_zoom,
        };
        Self {
            source,
            max_source_zoom,
            detail_bias,
            theme,
            jobs: JobQueue::new(),
            in_flight: FxHashSet::default(),
            wanted: Arc::new(Mutex::new(FxHashSet::default())),
            cache: LruCache::new(TILE_BUDGET),
            gpu: None,
            gpu_init_failed: false,
            draw_list: Vec::new(),
            labels: Vec::new(),
            wants_redraw: false,
        }
    }

    pub fn source(&self) -> Option<&Arc<dyn TileSource>> {
        self.source.as_ref()
    }

    pub fn max_source_zoom(&self) -> u8 {
        self.max_source_zoom
    }

    /// Change the zoom-selection bias at runtime (future user setting).
    pub fn set_detail_bias(&mut self, bias: f64) {
        self.detail_bias = bias;
    }

    pub fn detail_bias(&self) -> f64 {
        self.detail_bias
    }

    /// Switch the color theme: drops every cached tile mesh and invalidates
    /// in-flight decodes, so the next `prepare` re-requests the viewport
    /// coverage and re-tessellates it with the new colors.
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
        self.cache.clear();
        self.jobs.invalidate();
        // Results of invalidated jobs are dropped on drain, so their
        // in-flight marks would never resolve — clear them so the tiles are
        // re-requested immediately.
        self.in_flight.clear();
        self.wants_redraw = true;
    }

    /// This frame's place/waterway labels (computed in `prepare`). The
    /// renderer forwards them — zoom-filtered — into the shared
    /// [`crate::text::TextSystem`] before its `prepare` runs.
    pub fn pending_labels(&self) -> &[LabelRequest] {
        &self.labels
    }

    /// Drain-side bookkeeping for one worker result. `entry == None` (a
    /// skipped job) only clears the in-flight mark: nothing is cached, so
    /// the tile is re-requested if it becomes wanted again.
    fn finish_job(&mut self, id: TileId, entry: Option<TileEntry>) {
        self.in_flight.remove(&id);
        if let Some(entry) = entry {
            self.cache.put(id, entry);
        }
    }

    fn readiness(&self, id: TileId, now: Instant) -> TileReadiness {
        match self.cache.peek(&id) {
            None => TileReadiness::Missing,
            Some(entry) => match entry.content {
                // Absent-at-source reads as Missing: the fallback walk must
                // continue to an ancestor — otherwise a mid-ingest archive
                // (or any zoom past the data) blanks the basemap.
                TileContent::Absent => TileReadiness::Missing,
                TileContent::Empty => TileReadiness::Empty,
                TileContent::Mesh(_) => TileReadiness::Ready {
                    fading: fade_alpha(entry, now) < 1.0,
                },
            },
        }
    }

    fn ensure_gpu(&mut self, ctx: &PrepareCtx<'_>) {
        if self.gpu.is_some() || self.gpu_init_failed {
            return;
        }
        match BasemapGpu::new(ctx) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(error) => {
                tracing::error!(%error, "basemap pipeline init failed; layer disabled");
                self.gpu_init_failed = true;
            }
        }
    }
}

impl MapLayer for BasemapLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.draw_list.clear();
        self.labels.clear();
        self.wants_redraw = false;
        let Some(source) = self.source.clone() else {
            return;
        };
        self.ensure_gpu(ctx);
        if self.gpu.is_none() {
            return;
        }
        let now = Instant::now();

        // Finished decodes → GPU upload → cache (negative results included;
        // skipped jobs only resolve their in-flight mark).
        for decoded in self.jobs.drain() {
            let entry = match decoded.outcome {
                TileOutcome::Loaded(data) => {
                    tracing::debug!(
                        tile = ?decoded.id,
                        indices = data.mesh.indices.len(),
                        labels = data.labels.len(),
                        "basemap tile uploaded"
                    );
                    Some(TileEntry {
                        content: if data.mesh.is_empty() {
                            TileContent::Empty
                        } else {
                            TileContent::Mesh(GpuMesh::upload(ctx.device, &data.mesh, decoded.id))
                        },
                        labels: data.labels,
                        loaded_at: now,
                    })
                }
                TileOutcome::Missing => Some(TileEntry {
                    content: TileContent::Absent,
                    labels: Vec::new(),
                    loaded_at: now,
                }),
                TileOutcome::Skipped => None,
            };
            self.finish_job(decoded.id, entry);
        }

        let coverage = viewport_coverage(ctx.camera, self.max_source_zoom, self.detail_bias);

        // Keep the worker-visible wanted set in sync with the coverage so
        // queued jobs for tiles of a superseded view bail out cheaply.
        {
            let mut wanted_tiles = self.wanted.lock();
            wanted_tiles.clear();
            wanted_tiles.extend(coverage.tiles.iter().copied());
        }

        // Request uncovered tiles (IO + decode + tessellation on workers).
        // Stale negative entries no longer suppress the request — the tile
        // is re-checked, healing "ingested while the app runs" gaps. The
        // entry itself stays cached until the retry lands, so readiness()
        // keeps reporting it and nothing flickers meanwhile.
        for &wanted in &coverage.tiles {
            let cached = self.cache.peek(&wanted).is_some_and(|e| {
                entry_blocks_request(!matches!(e.content, TileContent::Absent), e.loaded_at, now)
            });
            if cached || self.in_flight.contains(&wanted) {
                continue;
            }
            self.in_flight.insert(wanted);
            let source = Arc::clone(&source);
            let wanted_tiles = Arc::clone(&self.wanted);
            let theme = Arc::clone(&self.theme);
            self.jobs.submit(ctx.workers, move || {
                decode_if_wanted(
                    wanted,
                    &wanted_tiles,
                    || source.tile(wanted),
                    &theme.basemap,
                )
            });
        }

        // Draw plan: per wanted tile the nearest ready mesh(es), bottom
        // first; one uniform slot per distinct mesh.
        let mut uniforms: Vec<TileUniform> = Vec::new();
        let mut slot_of: FxHashMap<TileId, u32> = FxHashMap::default();
        let mut fading = false;
        for &wanted in &coverage.tiles {
            let plan = fallback::plan_draws(wanted, |id| self.readiness(id, now));
            if plan.is_empty() {
                continue;
            }
            let Some(scissor) = tile_scissor(ctx.camera, wanted) else {
                continue;
            };
            for mesh_id in plan {
                let uniform_index = *slot_of.entry(mesh_id).or_insert_with(|| {
                    let alpha = self
                        .cache
                        .peek(&mesh_id)
                        .map(|e| fade_alpha(e, now))
                        .unwrap_or(1.0);
                    fading |= alpha < 1.0;
                    let origin_rel = mesh_id.world_bounds().0 - ctx.camera.center();
                    uniforms.push(TileUniform {
                        origin_rel: [origin_rel.x as f32, origin_rel.y as f32],
                        scale: mesh_id.world_size() as f32,
                        alpha,
                    });
                    (uniforms.len() - 1) as u32
                });
                self.draw_list.push(DrawCmd {
                    mesh: mesh_id,
                    uniform_index,
                    scissor,
                });
            }
        }

        // Touch everything used or wanted so the LRU evicts offscreen tiles
        // first.
        for cmd in &self.draw_list {
            self.cache.promote(&cmd.mesh);
        }
        for wanted in &coverage.tiles {
            self.cache.promote(wanted);
        }

        // Labels of every distinct drawn mesh (ancestors stand in for their
        // missing descendants here too).
        let mut seen: FxHashSet<TileId> = FxHashSet::default();
        for cmd in &self.draw_list {
            if !seen.insert(cmd.mesh) {
                continue;
            }
            if let Some(entry) = self.cache.peek(&cmd.mesh) {
                self.labels
                    .extend(entry.labels.iter().map(LabelSpec::to_request));
            }
        }

        if let Some(gpu) = &mut self.gpu {
            gpu.upload_tile_uniforms(ctx.device, ctx.queue, &uniforms);
        }

        self.wants_redraw = !self.in_flight.is_empty() || fading;
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ctx: &DrawCtx<'_>) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        if self.draw_list.is_empty() {
            return;
        }
        pass.set_pipeline(gpu.pipeline());
        let mut scissored = false;
        for cmd in &self.draw_list {
            let Some(mesh) = self.cache.peek(&cmd.mesh).and_then(|e| e.content.mesh()) else {
                continue;
            };
            pass.set_scissor_rect(cmd.scissor.x, cmd.scissor.y, cmd.scissor.w, cmd.scissor.h);
            scissored = true;
            pass.set_bind_group(
                1,
                gpu.tile_bind_group(),
                &[gpu.uniform_offset(cmd.uniform_index)],
            );
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
        if scissored {
            // Leave the pass with a full-viewport scissor for later layers.
            let size = ctx.camera.viewport().size_px();
            pass.set_scissor_rect(0, 0, size.x.max(1), size.y.max(1));
        }
    }

    fn wants_redraw(&self) -> bool {
        self.wants_redraw
    }
}

fn fade_alpha(entry: &TileEntry, now: Instant) -> f32 {
    if entry.content.mesh().is_none() {
        return 1.0;
    }
    (now.duration_since(entry.loaded_at).as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0)
}

/// Whether a cached entry suppresses re-requesting its tile: decoded
/// entries (mesh or authoritative empty) always do; absent-at-source
/// entries only until [`NEGATIVE_TTL`] passes, so tiles ingested or
/// repaired while the app runs appear without a restart.
fn entry_blocks_request(decoded: bool, loaded_at: Instant, now: Instant) -> bool {
    decoded || now.duration_since(loaded_at) < NEGATIVE_TTL
}

/// Worker-side gate: fetch + decode only while the tile is still wanted.
/// Stale jobs from a superseded coverage cost one hash lookup instead of an
/// SQLite read + MVT decode + lyon tessellation.
fn decode_if_wanted(
    tile: TileId,
    wanted: &Mutex<FxHashSet<TileId>>,
    fetch: impl FnOnce() -> Option<Vec<u8>>,
    theme: &BasemapTheme,
) -> DecodedTile {
    if !wanted.lock().contains(&tile) {
        return DecodedTile::skipped(tile);
    }
    decode::decode_tile(tile, fetch(), theme)
}

/// Physical-pixel screen rect of a tile, clamped to the render target.
/// Adjacent tiles share rounded edges, so the rects partition exactly.
/// `None` when the tile is fully offscreen.
fn tile_scissor(camera: &Camera, tile: TileId) -> Option<Scissor> {
    let (world_min, world_max) = tile.world_bounds();
    let sf = camera.viewport().scale_factor() as f64;
    let size = camera.viewport().size_px();
    let a = camera.project(world_min) * sf;
    let b = camera.project(world_max) * sf;
    let x0 = a.x.round().clamp(0.0, size.x as f64) as u32;
    let y0 = a.y.round().clamp(0.0, size.y as f64) as u32;
    let x1 = b.x.round().clamp(0.0, size.x as f64) as u32;
    let y1 = b.y.round().clamp(0.0, size.y as f64) as u32;
    (x1 > x0 && y1 > y0).then_some(Scissor {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Viewport;

    use glam::{DVec2, UVec2};

    fn oldworld() -> Arc<MapTheme> {
        Arc::new(MapTheme::oldworld())
    }

    fn camera_at(center: DVec2, zoom: f64, size: UVec2, sf: f32) -> Camera {
        let mut camera = Camera::new(Viewport::new(size, sf));
        // Drive the camera to an exact pose through its public API.
        camera.fly_to(crate::geo::lat_lon_from_world(center), zoom);
        for _ in 0..10_000 {
            if !camera.tick(Duration::from_millis(16)) {
                break;
            }
        }
        camera
    }

    #[test]
    fn adjacent_tile_scissors_partition_without_overlap() {
        let camera = camera_at(DVec2::new(0.53, 0.34), 8.0, UVec2::new(1280, 960), 2.0);
        let level = crate::tiles::display_level(camera.zoom(), 13, 0.3);
        let center_tile = TileId::containing(level, camera.center());
        let right = TileId::new(level, center_tile.x + 1, center_tile.y).expect("right tile");
        let a = tile_scissor(&camera, center_tile).expect("center visible");
        let b = tile_scissor(&camera, right).expect("right visible");
        assert_eq!(a.x + a.w, b.x, "shared edge must be exact");
        assert!(a.w > 0 && a.h > 0);
    }

    #[test]
    fn offscreen_tiles_have_no_scissor() {
        let camera = camera_at(DVec2::new(0.53, 0.34), 10.0, UVec2::new(800, 600), 1.0);
        let level = crate::tiles::display_level(camera.zoom(), 13, 0.3);
        let far = TileId::new(level, 0, 0).expect("corner tile");
        assert!(tile_scissor(&camera, far).is_none());
    }

    #[test]
    fn layer_without_source_stays_inert() {
        let layer = BasemapLayer::new(None, 13, -0.5, oldworld());
        assert!(!layer.wants_redraw());
        assert!(layer.pending_labels().is_empty());
        assert!(layer.source().is_none());
        assert_eq!(layer.max_source_zoom(), 13);
        assert_eq!(layer.detail_bias(), -0.5);
    }

    /// A theme switch must drop every cached mesh (they were tessellated
    /// with the old colors) and clear in-flight marks so the coverage is
    /// re-requested with the new theme.
    #[test]
    fn theme_change_drops_cached_tiles_and_inflight_marks() {
        let mut layer = BasemapLayer::new(None, 13, -0.5, oldworld());
        let cached = TileId::new(8, 135, 84).expect("valid tile");
        let flying = TileId::new(8, 136, 84).expect("valid tile");
        layer.cache.put(
            cached,
            TileEntry {
                content: TileContent::Empty,
                labels: Vec::new(),
                loaded_at: Instant::now(),
            },
        );
        layer.in_flight.insert(flying);

        layer.set_theme(Arc::new(MapTheme::pastel_light()));
        assert!(layer.cache.is_empty(), "stale-color meshes must be dropped");
        assert!(layer.in_flight.is_empty(), "in-flight marks must clear");
        assert!(layer.wants_redraw(), "a redraw must be scheduled");
        assert_eq!(layer.theme.id, "pastel-light");
    }

    #[test]
    fn detail_bias_is_adjustable_at_runtime() {
        let mut layer = BasemapLayer::new(None, 13, -0.5, oldworld());
        layer.set_detail_bias(0.25);
        assert_eq!(layer.detail_bias(), 0.25);
    }

    /// A source that reports its own depth via metadata.
    struct DepthSource(u8);
    impl TileSource for DepthSource {
        fn tile(&self, _id: TileId) -> Option<Vec<u8>> {
            None
        }
        fn max_zoom(&self) -> Option<u8> {
            Some(self.0)
        }
    }

    #[test]
    fn source_reported_max_zoom_overrides_the_fallback() {
        let layer = BasemapLayer::new(Some(Arc::new(DepthSource(11))), 13, -0.5, oldworld());
        assert_eq!(
            layer.max_source_zoom(),
            11,
            "metadata maxzoom must win over the configured default"
        );

        struct Silent;
        impl TileSource for Silent {
            fn tile(&self, _id: TileId) -> Option<Vec<u8>> {
                None
            }
        }
        let layer = BasemapLayer::new(Some(Arc::new(Silent)), 13, -0.5, oldworld());
        assert_eq!(layer.max_source_zoom(), 13, "no hint: fallback applies");
    }

    #[test]
    fn negative_entries_block_requests_only_while_fresh() {
        let now = Instant::now();
        let fresh = now - NEGATIVE_TTL / 2;
        let stale = now - NEGATIVE_TTL * 2;
        assert!(entry_blocks_request(false, fresh, now), "fresh negative");
        assert!(!entry_blocks_request(false, stale, now), "stale negative");
        assert!(
            entry_blocks_request(true, stale, now),
            "decoded entries never expire"
        );
    }

    /// Absent-at-source tiles (mid-ingest, or zoom past the data) must read
    /// as `Missing` so the ancestor fallback keeps covering them; only a
    /// decoded-empty tile is authoritative and blocks fallback.
    #[test]
    fn absent_tiles_keep_the_ancestor_fallback_alive() {
        let mut layer = BasemapLayer::new(None, 13, -0.5, oldworld());
        let now = Instant::now();
        let wanted = TileId::new(13, 4400, 2686).expect("valid tile");
        let ancestor = wanted.ancestor(6).expect("ancestor");

        layer.cache.put(
            wanted,
            TileEntry {
                content: TileContent::Absent,
                labels: Vec::new(),
                loaded_at: now,
            },
        );
        assert_eq!(layer.readiness(wanted, now), TileReadiness::Missing);

        // End-to-end through the planner: the deep ancestor still draws.
        let empty_sibling = TileId::new(13, 4401, 2686).expect("valid tile");
        layer.cache.put(
            empty_sibling,
            TileEntry {
                content: TileContent::Empty,
                labels: Vec::new(),
                loaded_at: now,
            },
        );
        assert_eq!(layer.readiness(empty_sibling, now), TileReadiness::Empty);
        let readiness = |id: TileId| {
            if id == ancestor {
                TileReadiness::Ready { fading: false }
            } else {
                layer.readiness(id, now)
            }
        };
        assert_eq!(
            fallback::plan_draws(wanted, readiness),
            vec![ancestor],
            "absent tile falls back to the ready ancestor"
        );
        assert!(
            fallback::plan_draws(empty_sibling, readiness).is_empty(),
            "decoded-empty tile stays authoritative"
        );
    }

    /// Skipped jobs must leave no cache entry: the tile stays re-requestable
    /// when it becomes wanted again (the request loop only skips cached or
    /// in-flight tiles).
    #[test]
    fn skipped_results_clear_in_flight_without_caching() {
        let mut layer = BasemapLayer::new(None, 13, -0.5, oldworld());
        let id = TileId::new(8, 135, 84).expect("valid tile");

        layer.in_flight.insert(id);
        layer.finish_job(id, None);
        assert!(!layer.in_flight.contains(&id));
        assert!(layer.cache.peek(&id).is_none(), "skipped jobs never cache");

        // A finished decode (negative here) does land in the cache.
        layer.in_flight.insert(id);
        layer.finish_job(
            id,
            Some(TileEntry {
                content: TileContent::Absent,
                labels: Vec::new(),
                loaded_at: Instant::now(),
            }),
        );
        assert!(!layer.in_flight.contains(&id));
        assert!(layer.cache.peek(&id).is_some());
    }

    /// The worker-side gate skips fetch + decode entirely for tiles that
    /// left the wanted coverage before their job started.
    #[test]
    fn unwanted_tiles_skip_fetch_and_decode() {
        let id = TileId::new(8, 135, 84).expect("valid tile");
        let wanted = Mutex::new(FxHashSet::default());

        let theme = oldworld();
        let result = decode_if_wanted(id, &wanted, || panic!("must not fetch"), &theme.basemap);
        assert!(matches!(result.outcome, TileOutcome::Skipped));

        wanted.lock().insert(id);
        let result = decode_if_wanted(id, &wanted, || None, &theme.basemap);
        assert!(matches!(result.outcome, TileOutcome::Missing));
    }
}

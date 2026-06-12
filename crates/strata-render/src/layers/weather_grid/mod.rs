//! Gridded weather overlays: scalar fields (cloud cover, precipitation
//! rate, thunderstorm potential) on regular lat-lon grids, rendered as
//! colormapped, temporally-interpolated textures under one toggleable
//! [`LayerId`] per field.
//!
//! ## Data flow
//!
//! The app pushes per-field working sets of [`WeatherGridFrame`]s
//! (`set_frames`) and a slider position (`set_time`). Pushes are diffed
//! by **content identity**: a frame whose `(valid_time, grid, values)`
//! is bit-identical to the stored one (NaN no-data cells included) keeps
//! its content id, and with it its GPU texture and any in-flight
//! conversion. The fetch scheduler re-pushes the full list after every
//! arrival, so supersets and identical refreshes must never drop or
//! re-upload textures that are in use — only genuinely new content (a
//! fresher model run / radar refresh for the same valid time) converts
//! again, and the old texture keeps drawing until the replacement is
//! resident (the swap is atomic; there is never a blank frame).
//!
//! Each prepare picks the two frames bracketing the current weather time
//! ([`timeline::bracket_hold`] — a pair spanning a data hole pins the
//! nearest frame instead of blending non-adjacent times), converts them
//! to premultiplied `(value · coverage, coverage)` `Rg16Float` texels on
//! the worker pool (NaN → coverage 0) and uploads them into a small
//! per-field LRU texture cache keyed by valid time — scrubbing re-uses
//! cached neighbors instead of re-converting. The fragment shader blends
//! the two textures by the temporal fraction, so scrubbing is continuous;
//! at the range ends (or with a single frame) the same texture is bound
//! twice with fraction 0. While a newly needed texture is still
//! converting, the last-ready pair keeps drawing with a clamped fraction
//! — no popping.
//!
//! When the draw was pinned to a single held frame (time past the cached
//! frontier, or inside a hole) and the missing data arrives, the blend
//! does not snap to the new pair: the displayed fraction ramps from the
//! held frame's position to the correct value over [`REBLEND_RAMP`]
//! ([`timeline::ramp_fraction`], wall-clock based). If the held frame is
//! not an endpoint of the new pair at all, a short transitional crossfade
//! from the held texture to the nearer new frame bridges the jump first.
//!
//! ## Geometry
//!
//! Each field draws as **one quad** over the union of the two frames'
//! extents, transformed by the shared camera/origin mechanics (group 1
//! origin uniform, f64 camera subtraction on the CPU). Latitude is
//! nonlinear in Web-Mercator, so the fragment shader recovers the
//! geographic latitude per fragment instead of interpolating V linearly —
//! see the rationale in `shaders/weather_grid.wgsl` and the mapping tests
//! in [`grid`].
//!
//! Colormaps are piecewise-linear stop ramps from the active
//! [`MapTheme`] (see [`crate::map_theme::Colormap`] for why they are
//! uniforms, not LUT textures); the thunderstorm field adds the subtle
//! screen-space hatch shared with the SIGMET overlay.

mod grid;
mod timeline;
mod uniform;

use self::grid::{GridParams, union_world_rect};
use self::uniform::GridLocalsUniform;
use crate::features::{GriddedField, WeatherGridFrame};
use crate::layer::{DrawCtx, LayerId, LayerToggles, MapLayer, PrepareCtx};
use crate::map_theme::{Colormap, MapTheme};
use crate::workers::JobQueue;

use glam::DVec2;
use lru::LruCache;
use rustc_hash::FxHashMap;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Per-field GPU texture budget. Scrubbing the slider needs the bracketing
/// pair plus the neighbors it is about to hit; at full ICON-D2 resolution
/// one frame is ~3.6 MB (`Rg16Float`), so this caps a field at ~22 MB.
const TEXTURE_CACHE_PER_FIELD: NonZeroUsize =
    NonZeroUsize::new(6).expect("non-zero texture budget");

/// Hatch strength for the thunderstorm overlay (see `weather_grid.wgsl`).
const THUNDERSTORM_HATCH: f32 = 1.0;

/// Bracket pairs spanning more than this are data holes (the working set
/// skipped missing steps), not adjacent model steps: hold the nearest
/// frame instead of dissolving across the hole. ICON-D2 steps are hourly
/// and radar steps are 5-minutely, so 2 h tolerates one missing hourly
/// step while still catching real backfill gaps.
const MAX_BLEND_GAP_SECONDS: i64 = 2 * 3600;

/// Length of the re-blend ramp: how long the displayed blend fraction
/// takes to ease from a held frame into the correct blend after missing
/// data arrives (and of the transitional crossfade out of a held frame
/// that is not part of the new pair).
const REBLEND_RAMP: Duration = Duration::from_millis(200);

/// Gridded weather overlay layer (all three fields; per-field toggles).
pub struct GriddedWeatherLayer {
    theme: Arc<MapTheme>,
    /// Weather-time slider position, unix seconds.
    time: i64,
    fields: [FieldState; GriddedField::COUNT],
    gpu: GpuState,
}

impl GriddedWeatherLayer {
    /// The layer toggles this layer renders (z-order, bottom first).
    pub const CATEGORIES: [LayerId; GriddedField::COUNT] = [
        LayerId::CloudCover,
        LayerId::Precipitation,
        LayerId::Thunderstorms,
    ];

    pub fn new(theme: Arc<MapTheme>) -> Self {
        Self {
            theme,
            time: 0,
            fields: GriddedField::ALL.map(FieldState::new),
            gpu: GpuState::default(),
        }
    }

    /// Switch the color theme: the per-field colormaps are re-packed into
    /// the uniforms on the next prepare.
    pub fn set_theme(&mut self, theme: Arc<MapTheme>) {
        self.theme = theme;
    }

    /// Replace the working set of frames for `field`; returns whether the
    /// set changed (identical data — the periodic refresh — is a no-op,
    /// **including** frames with NaN no-data cells, which are compared
    /// bit-exactly).
    ///
    /// Frames are validated (grid shape, extent), sorted by valid time and
    /// de-duplicated; invalid or foreign-field frames are dropped with a
    /// warning. Unchanged frames keep their GPU textures and in-flight
    /// conversions — a superset push (the fetch scheduler re-pushes the
    /// full list after every arrival) re-uploads nothing. A frame with
    /// genuinely new content for an existing valid time converts again and
    /// swaps in atomically: the old texture keeps drawing until the
    /// replacement is resident.
    pub fn set_frames(&mut self, field: GriddedField, frames: Vec<WeatherGridFrame>) -> bool {
        let incoming = normalize(field, frames);
        self.fields[field.index()].replace_frames(incoming)
    }

    /// Move the weather-time slider; returns whether the time changed.
    pub fn set_time(&mut self, unix_seconds: i64) -> bool {
        if self.time == unix_seconds {
            return false;
        }
        self.time = unix_seconds;
        true
    }

    /// The current weather-time slider position (unix seconds).
    pub fn time(&self) -> i64 {
        self.time
    }

    /// True if any field that currently holds frames is toggled visible —
    /// lets the renderer skip redraws for slider moves nothing would show.
    pub fn any_visible_field(&self, toggles: &LayerToggles) -> bool {
        self.fields
            .iter()
            .any(|f| !f.frames.is_empty() && toggles.enabled(f.field.layer()))
    }

    /// Valid times of the stored working set (sorted), for tests/tooling.
    pub fn frame_times(&self, field: GriddedField) -> &[i64] {
        &self.fields[field.index()].times
    }

    #[cfg(test)]
    pub(crate) fn has_drawable(&self, field: GriddedField) -> bool {
        self.fields[field.index()].current.is_some()
    }

    /// Total frame textures uploaded for `field` since creation — the
    /// upload-count probe for the texture-stability tests.
    #[cfg(test)]
    pub(crate) fn upload_count(&self, field: GriddedField) -> u64 {
        self.fields[field.index()].uploads
    }

    /// Valid times of the currently bound draw pair (a transitional
    /// crossfade reports the bridge pair).
    #[cfg(test)]
    pub(crate) fn current_pair(&self, field: GriddedField) -> Option<(i64, i64)> {
        self.fields[field.index()]
            .current
            .as_ref()
            .map(|d| (d.time_a, d.time_b))
    }

    /// Content ids of the currently bound draw pair.
    #[cfg(test)]
    pub(crate) fn current_contents(&self, field: GriddedField) -> Option<(u64, u64)> {
        self.fields[field.index()]
            .current
            .as_ref()
            .map(|d| (d.content_a, d.content_b))
    }

    /// The blend fraction the bound pair drew with last prepare.
    #[cfg(test)]
    pub(crate) fn displayed_fraction(&self, field: GriddedField) -> Option<f32> {
        self.fields[field.index()]
            .current
            .as_ref()
            .map(|d| d.displayed)
    }
}

impl Default for GriddedWeatherLayer {
    fn default() -> Self {
        Self::new(Arc::new(MapTheme::oldworld()))
    }
}

impl MapLayer for GriddedWeatherLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.gpu.ensure(ctx);
        let GpuState::Ready(gpu) = &self.gpu else {
            return;
        };
        for (state, field_gpu) in self.fields.iter_mut().zip(&gpu.per_field) {
            if !ctx.layers.enabled(state.field.layer()) {
                // A hidden field keeps no animation alive, but finished
                // conversions still land so `wants_redraw` settles and a
                // re-enable shows instantly.
                state.ramp = None;
                state.drain_uploads(ctx);
                continue;
            }
            state.advance(ctx, gpu, self.time);
            if let Some(draw) = &state.current {
                let colormap = colormap_for(&self.theme, state.field);
                let hatch = match state.field {
                    GriddedField::ThunderstormPotential => THUNDERSTORM_HATCH,
                    GriddedField::CloudCover | GriddedField::PrecipRate => 0.0,
                };
                let uniform = draw.pack_uniform(ctx.camera.center(), hatch, colormap);
                ctx.queue
                    .write_buffer(&field_gpu.buffer, 0, bytemuck::bytes_of(&uniform));
            }
        }
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ctx: &DrawCtx<'_>) {
        let GpuState::Ready(gpu) = &self.gpu else {
            return;
        };
        for (state, field_gpu) in self.fields.iter().zip(&gpu.per_field) {
            if !ctx.layers.enabled(state.field.layer()) {
                continue;
            }
            let Some(draw) = &state.current else {
                continue;
            };
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(1, &field_gpu.locals_bind_group, &[]);
            pass.set_bind_group(2, &draw.texture_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// Only while frame textures are still converting/uploading or a
    /// re-blend ramp is animating — slider and data changes mark the
    /// renderer dirty through their `set_*` paths, so an idle map stays
    /// idle.
    fn wants_redraw(&self) -> bool {
        self.fields
            .iter()
            .any(|f| !f.in_flight.is_empty() || f.ramp.is_some())
    }
}

/// One stored frame: validated grid + shared values (jobs clone the `Arc`,
/// not the data). The `content_id` is the frame's identity for the GPU
/// texture cache: it survives pushes whose data is bit-identical and
/// changes when the same valid time arrives with new values (a fresher
/// model run / radar refresh) — see [`FieldState::replace_frames`].
#[derive(Debug, Clone)]
struct StoredFrame {
    valid_time: i64,
    grid: GridParams,
    values: Arc<Vec<f32>>,
    /// Assigned in `replace_frames` (0 = not yet adopted into a set).
    content_id: u64,
}

/// Bit-exact content equality (grid + values). `f32` `PartialEq` would
/// treat every real frame as different — NaN marks no-data cells (see
/// [`WeatherGridFrame::values`]) and `NaN != NaN` — which is exactly the
/// poison that used to nuke the texture cache on identical refreshes.
fn same_content(a: &StoredFrame, b: &StoredFrame) -> bool {
    a.grid == b.grid
        && bytemuck::cast_slice::<f32, u8>(&a.values) == bytemuck::cast_slice::<f32, u8>(&b.values)
}

/// Content id of the stored frame at `time`, if any (`frames` is sorted).
fn content_of(frames: &[StoredFrame], time: i64) -> Option<u64> {
    frames
        .binary_search_by_key(&time, |f| f.valid_time)
        .ok()
        .map(|i| frames[i].content_id)
}

/// Validate, sort and de-duplicate an incoming working set.
fn normalize(field: GriddedField, frames: Vec<WeatherGridFrame>) -> Vec<StoredFrame> {
    let mut stored: Vec<StoredFrame> = Vec::with_capacity(frames.len());
    for frame in frames {
        if frame.field != field {
            tracing::warn!(
                expected = ?field,
                got = ?frame.field,
                valid_time = frame.valid_time,
                "weather grid frame for the wrong field dropped"
            );
            continue;
        }
        match GridParams::from_frame(&frame) {
            Ok(grid) => stored.push(StoredFrame {
                valid_time: frame.valid_time,
                grid,
                values: Arc::new(frame.values),
                content_id: 0,
            }),
            Err(e) => {
                tracing::warn!(
                    field = ?field,
                    valid_time = frame.valid_time,
                    error = %e,
                    "invalid weather grid frame dropped"
                );
            }
        }
    }
    stored.sort_by_key(|f| f.valid_time);
    // Equal valid times: the stable sort kept input order, the last wins.
    let mut deduped: Vec<StoredFrame> = Vec::with_capacity(stored.len());
    for frame in stored {
        match deduped.last_mut() {
            Some(last) if last.valid_time == frame.valid_time => *last = frame,
            _ => deduped.push(frame),
        }
    }
    deduped
}

/// A frame texture resident on the GPU.
struct FrameTexture {
    /// Kept alive explicitly: the LRU may evict an entry that the current
    /// draw pair still references.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    grid: GridParams,
}

/// A cache slot: the texture plus the content identity it was converted
/// from, so a frame replaced by newer data (same valid time) is detected
/// as stale without dropping the still-drawing old texture.
struct CachedTexture {
    content_id: u64,
    texture: Arc<FrameTexture>,
}

/// One endpoint of a (candidate) draw pair.
struct PairTexture {
    time: i64,
    content: u64,
    texture: Arc<FrameTexture>,
}

/// Worker → render thread conversion result.
struct ConvertedFrame {
    valid_time: i64,
    content_id: u64,
    grid: GridParams,
    /// `Rg16Float` texels: `(value · coverage, coverage)` per grid point.
    texels: Vec<u8>,
}

/// The ready-to-draw state of one field: the bound texture pair.
struct FieldDraw {
    time_a: i64,
    time_b: i64,
    content_a: u64,
    content_b: u64,
    /// A crossfade bridge out of a held frame, not the real bracket pair:
    /// the displayed fraction ramps toward 1 (fully the new frame), then
    /// the next prepare rebinds the real pair. The pair times may be
    /// unordered here — nothing below depends on their order.
    transitional: bool,
    /// The blend fraction this pair drew with last prepare — the "current
    /// visual state" that ramps start from and hold detection reads.
    displayed: f32,
    a: Arc<FrameTexture>,
    b: Arc<FrameTexture>,
    texture_bind_group: wgpu::BindGroup,
}

impl FieldDraw {
    fn pack_uniform(
        &self,
        camera_center: DVec2,
        hatch: f32,
        colormap: &Colormap,
    ) -> GridLocalsUniform {
        let (nw, size) = union_world_rect(&self.a.grid, &self.b.grid);
        uniform::pack(
            uniform::QuadPlacement {
                origin_rel: nw - camera_center,
                nw_abs: nw,
                size_world: size,
            },
            self.displayed,
            hatch,
            &self.a.grid,
            &self.b.grid,
            colormap,
        )
    }

    /// The single frame this draw is visually pinned to, if any: a
    /// degenerate pair (range clamp / hole hold) or a blend whose
    /// displayed fraction sits at an endpoint (clamped while a texture
    /// converts, or a finished crossfade). A pair change away from a held
    /// frame starts a re-blend ramp instead of snapping.
    fn held(&self) -> Option<PairTexture> {
        const EPS: f32 = 1e-3;
        if self.time_a == self.time_b || self.displayed <= EPS {
            Some(PairTexture {
                time: self.time_a,
                content: self.content_a,
                texture: Arc::clone(&self.a),
            })
        } else if self.displayed >= 1.0 - EPS {
            Some(PairTexture {
                time: self.time_b,
                content: self.content_b,
                texture: Arc::clone(&self.b),
            })
        } else {
            None
        }
    }
}

/// A running re-blend ramp: the displayed fraction eases from `from` to
/// the live target over [`REBLEND_RAMP`], by wall time.
struct Ramp {
    from: f32,
    started: Instant,
}

struct FieldState {
    field: GriddedField,
    frames: Vec<StoredFrame>,
    /// `frames[i].valid_time`, kept in sync for bracket lookups.
    times: Vec<i64>,
    /// Source of fresh frame content ids (`replace_frames`).
    next_content_id: u64,
    jobs: JobQueue<ConvertedFrame>,
    /// valid time → content id of the conversion in flight.
    in_flight: FxHashMap<i64, u64>,
    cache: LruCache<i64, CachedTexture>,
    current: Option<FieldDraw>,
    ramp: Option<Ramp>,
    /// Total textures uploaded (probe for the texture-stability tests).
    uploads: u64,
}

impl FieldState {
    fn new(field: GriddedField) -> Self {
        Self {
            field,
            frames: Vec::new(),
            times: Vec::new(),
            next_content_id: 1,
            jobs: JobQueue::new(),
            in_flight: FxHashMap::default(),
            cache: LruCache::new(TEXTURE_CACHE_PER_FIELD),
            current: None,
            ramp: None,
            uploads: 0,
        }
    }

    /// Diff the incoming working set against the stored one. Frames whose
    /// `(valid_time, grid, values)` are bit-identical to a stored frame
    /// keep their identity — and with it their GPU texture and any
    /// in-flight conversion; genuinely new content gets a fresh id and
    /// converts lazily in `advance`. Returns whether anything changed.
    ///
    /// The draw pair is deliberately **not** touched: a stale-but-drawable
    /// pair keeps drawing (its `Arc`s outlive cache eviction) until
    /// `advance` swaps in the replacement — there is never a frame where
    /// neither old nor new data is drawable.
    fn replace_frames(&mut self, incoming: Vec<StoredFrame>) -> bool {
        let mut changed = incoming.len() != self.frames.len();
        let mut merged = Vec::with_capacity(incoming.len());
        for mut frame in incoming {
            match self
                .frames
                .binary_search_by_key(&frame.valid_time, |f| f.valid_time)
            {
                Ok(i) if same_content(&self.frames[i], &frame) => {
                    merged.push(self.frames[i].clone());
                }
                _ => {
                    changed = true;
                    frame.content_id = self.next_content_id;
                    self.next_content_id += 1;
                    merged.push(frame);
                }
            }
        }
        if !changed {
            return false;
        }
        self.frames = merged;
        self.times = self.frames.iter().map(|f| f.valid_time).collect();
        // Retire cache entries and in-flight conversions that no longer
        // match a stored frame. Stale jobs already running waste a worker
        // slot at worst — their results are dropped on drain.
        let stale: Vec<i64> = self
            .cache
            .iter()
            .filter(|(time, cached)| content_of(&self.frames, **time) != Some(cached.content_id))
            .map(|(time, _)| *time)
            .collect();
        for time in stale {
            self.cache.pop(&time);
        }
        let frames = &self.frames;
        self.in_flight
            .retain(|time, id| content_of(frames, *time) == Some(*id));
        if self.frames.is_empty() {
            self.jobs.invalidate();
            self.current = None;
            self.ramp = None;
        }
        true
    }

    /// The cached texture for `(time, content)` if it is resident and
    /// up to date.
    fn resident(&self, time: i64, content: u64) -> Option<PairTexture> {
        self.cache
            .peek(&time)
            .filter(|cached| cached.content_id == content)
            .map(|cached| PairTexture {
                time,
                content,
                texture: Arc::clone(&cached.texture),
            })
    }

    /// Upload finished conversions into the texture cache. Results that
    /// were superseded while converting (content replaced or frame dropped
    /// from the set) are discarded.
    fn drain_uploads(&mut self, ctx: &PrepareCtx<'_>) {
        for converted in self.jobs.drain() {
            if self.in_flight.get(&converted.valid_time) != Some(&converted.content_id) {
                continue;
            }
            self.in_flight.remove(&converted.valid_time);
            let texture = upload_frame_texture(ctx, &converted);
            self.cache.put(
                converted.valid_time,
                CachedTexture {
                    content_id: converted.content_id,
                    texture: Arc::new(texture),
                },
            );
            self.uploads += 1;
            tracing::debug!(
                field = ?self.field,
                valid_time = converted.valid_time,
                total_uploads = self.uploads,
                "weather grid frame uploaded"
            );
        }
    }

    /// Kick conversions for the frames bracketing `time`, drain finished
    /// uploads, (re)bind the draw pair once both textures are resident,
    /// and update the displayed blend fraction (ramps included).
    fn advance(&mut self, ctx: &PrepareCtx<'_>, gpu: &Gpu, time: i64) {
        let now = Instant::now();
        if self
            .ramp
            .as_ref()
            .is_some_and(|r| now.duration_since(r.started) >= REBLEND_RAMP)
        {
            self.ramp = None;
        }
        self.drain_uploads(ctx);
        let Some((ia, ib, _)) = timeline::bracket_hold(&self.times, time, MAX_BLEND_GAP_SECONDS)
        else {
            self.current = None;
            self.ramp = None;
            return;
        };
        let (time_a, content_a) = (self.frames[ia].valid_time, self.frames[ia].content_id);
        let (time_b, content_b) = (self.frames[ib].valid_time, self.frames[ib].content_id);
        let needed = [ia, ib];
        for &index in &needed[..if ia == ib { 1 } else { 2 }] {
            let frame = &self.frames[index];
            let key = frame.valid_time;
            if self
                .cache
                .peek(&key)
                .is_some_and(|cached| cached.content_id == frame.content_id)
            {
                self.cache.promote(&key);
                continue;
            }
            if self.in_flight.get(&key) == Some(&frame.content_id) {
                continue;
            }
            self.in_flight.insert(key, frame.content_id);
            let values = Arc::clone(&frame.values);
            let (grid, content_id) = (frame.grid, frame.content_id);
            self.jobs.submit(ctx.workers, move || ConvertedFrame {
                valid_time: key,
                content_id,
                grid,
                texels: texels_rg16(&values),
            });
        }

        let pair_current = self.current.as_ref().is_some_and(|d| {
            !d.transitional
                && (d.time_a, d.content_a, d.time_b, d.content_b)
                    == (time_a, content_a, time_b, content_b)
        });
        // A running crossfade finishes before the real pair binds — its
        // endpoint then matches a held frame and ramps in smoothly.
        let mid_crossfade =
            self.current.as_ref().is_some_and(|d| d.transitional) && self.ramp.is_some();
        if !pair_current
            && !mid_crossfade
            && let (Some(a), Some(b)) = (
                self.resident(time_a, content_a),
                self.resident(time_b, content_b),
            )
        {
            let target = timeline::clamped_fraction(time_a, time_b, time);
            match self.current.as_ref().and_then(FieldDraw::held) {
                Some(held) if held.time == time_a => {
                    self.bind_pair(ctx, gpu, a, b, false);
                    self.ramp = Some(Ramp {
                        from: 0.0,
                        started: now,
                    });
                }
                Some(held) if held.time == time_b => {
                    self.bind_pair(ctx, gpu, a, b, false);
                    self.ramp = Some(Ramp {
                        from: 1.0,
                        started: now,
                    });
                }
                Some(held) => {
                    // The held frame is not part of the new pair (far
                    // scrub past the cached frontier): crossfade from it
                    // to the nearer endpoint first; the follow-up rebind
                    // ramps from that endpoint into the real blend.
                    let to = if target < 0.5 { a } else { b };
                    self.bind_pair(ctx, gpu, held, to, true);
                    self.ramp = Some(Ramp {
                        from: 0.0,
                        started: now,
                    });
                }
                None => {
                    // First show, or a mid-blend pair change (fast slider
                    // jump / atomic content swap): bind directly.
                    self.bind_pair(ctx, gpu, a, b, false);
                    self.ramp = None;
                }
            }
        }

        if let Some(draw) = &mut self.current {
            let target = if draw.transitional {
                1.0
            } else {
                timeline::clamped_fraction(draw.time_a, draw.time_b, time)
            };
            draw.displayed = match &self.ramp {
                Some(ramp) => timeline::ramp_fraction(
                    ramp.from,
                    target,
                    now.duration_since(ramp.started),
                    REBLEND_RAMP,
                ),
                None => target,
            };
        }
    }

    /// Bind `(a, b)` as the field's draw pair. The previous pair's
    /// textures stay alive through their `Arc`s until this replacement, so
    /// the swap is atomic for the draw.
    fn bind_pair(
        &mut self,
        ctx: &PrepareCtx<'_>,
        gpu: &Gpu,
        a: PairTexture,
        b: PairTexture,
        transitional: bool,
    ) {
        let texture_bind_group =
            gpu.texture_bind_group(ctx.device, &a.texture.view, &b.texture.view);
        self.current = Some(FieldDraw {
            time_a: a.time,
            time_b: b.time,
            content_a: a.content,
            content_b: b.content,
            transitional,
            displayed: 0.0,
            a: a.texture,
            b: b.texture,
            texture_bind_group,
        });
    }
}

/// Pure worker job: scalar values → premultiplied `(value · coverage,
/// coverage)` `Rg16Float` texels, NaN/no-data → coverage 0. Premultiplying
/// keeps bilinear filtering correct across data holes (zeros never bleed
/// into valid neighbors at full weight; the shader divides by the filtered
/// coverage).
fn texels_rg16(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for &v in values {
        let (value, coverage) = if v.is_finite() { (v, 1.0f32) } else { (0.0, 0.0) };
        out.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
        out.extend_from_slice(&half::f16::from_f32(coverage).to_le_bytes());
    }
    out
}

fn upload_frame_texture(ctx: &PrepareCtx<'_>, converted: &ConvertedFrame) -> FrameTexture {
    let (ni, nj) = (converted.grid.ni, converted.grid.nj);
    let size = wgpu::Extent3d {
        width: ni,
        height: nj,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("strata weather grid frame"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &converted.texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ni * 4),
            rows_per_image: Some(nj),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    FrameTexture {
        texture,
        view,
        grid: converted.grid,
    }
}

fn colormap_for(theme: &MapTheme, field: GriddedField) -> &Colormap {
    match field {
        GriddedField::CloudCover => &theme.weather.cloud_cover,
        GriddedField::PrecipRate => &theme.weather.precip_rate,
        GriddedField::ThunderstormPotential => &theme.weather.thunderstorm,
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
                tracing::error!(error = %e, "weather grid pipeline failed; layer disabled");
                Self::Failed
            }
        };
    }
}

struct FieldGpu {
    buffer: wgpu::Buffer,
    locals_bind_group: wgpu::BindGroup,
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    per_field: [FieldGpu; GriddedField::COUNT],
}

impl Gpu {
    fn new(ctx: &PrepareCtx<'_>) -> Result<Self, crate::error::RenderError> {
        let device = ctx.device;
        let module = ctx.shaders.create_module(device, "weather_grid.wgsl")?;

        let locals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strata weather grid locals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<GridLocalsUniform>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let frame_texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strata weather grid textures"),
            entries: &[
                frame_texture_entry(0),
                frame_texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strata weather grid"),
            bind_group_layouts: &[
                Some(ctx.globals_layout),
                Some(&locals_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("strata weather grid"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Bilinear between grid points; clamp keeps the half-texel border
        // from wrapping (the shader's window test masks anything outside).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("strata weather grid"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let per_field = GriddedField::ALL.map(|field| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strata weather grid locals"),
                size: std::mem::size_of::<GridLocalsUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let locals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(match field {
                    GriddedField::CloudCover => "strata weather grid locals (clouds)",
                    GriddedField::PrecipRate => "strata weather grid locals (precip)",
                    GriddedField::ThunderstormPotential => "strata weather grid locals (storms)",
                }),
                layout: &locals_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            FieldGpu {
                buffer,
                locals_bind_group,
            }
        });
        Ok(Self {
            pipeline,
            sampler,
            texture_layout,
            per_field,
        })
    }

    fn texture_bind_group(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("strata weather grid frames"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerToggles;

    fn frame(valid_time: i64, value: f32) -> WeatherGridFrame {
        WeatherGridFrame {
            field: GriddedField::PrecipRate,
            valid_time,
            extent: (47.0, 6.0, 55.0, 15.0),
            ni: 3,
            nj: 2,
            values: vec![value; 6],
        }
    }

    #[test]
    fn set_frames_sorts_dedups_and_drops_invalid() {
        let mut layer = GriddedWeatherLayer::default();
        let mut wrong_field = frame(500, 1.0);
        wrong_field.field = GriddedField::CloudCover;
        let mut bad_shape = frame(600, 1.0);
        bad_shape.values.pop();
        let changed = layer.set_frames(
            GriddedField::PrecipRate,
            vec![
                frame(300, 1.0),
                frame(100, 1.0),
                frame(300, 2.0), // duplicate valid time: last wins
                wrong_field,
                bad_shape,
                frame(200, 1.0),
            ],
        );
        assert!(changed);
        assert_eq!(layer.frame_times(GriddedField::PrecipRate), [100, 200, 300]);
        assert!(layer.frame_times(GriddedField::CloudCover).is_empty());
        let state = &layer.fields[GriddedField::PrecipRate.index()];
        assert_eq!(state.frames[2].values[0], 2.0, "later duplicate wins");
    }

    /// The periodic refresh usually returns identical data — must be a
    /// no-op so the renderer stays idle and caches survive.
    #[test]
    fn identical_frame_set_is_ignored() {
        let mut layer = GriddedWeatherLayer::default();
        assert!(layer.set_frames(GriddedField::CloudCover, vec![frame_cc(100, 50.0)]));
        assert!(!layer.set_frames(GriddedField::CloudCover, vec![frame_cc(100, 50.0)]));
        assert!(
            layer.set_frames(GriddedField::CloudCover, vec![frame_cc(100, 80.0)]),
            "same valid time, new values (newer run) must replace"
        );
        assert!(
            layer.set_frames(GriddedField::CloudCover, Vec::new()),
            "clearing is a change"
        );
        assert!(!layer.set_frames(GriddedField::CloudCover, Vec::new()));
    }

    /// Real frames contain NaN no-data cells. `NaN != NaN` under `f32`
    /// equality, which used to make byte-identical refreshes register as
    /// changes and nuke the texture cache — content comparison must be
    /// bit-exact instead.
    #[test]
    fn identical_frames_with_nan_holes_are_a_no_op() {
        let with_nan = || {
            let mut f = frame_cc(100, 50.0);
            f.values[2] = f32::NAN;
            f.values[4] = f32::NAN;
            f
        };
        let mut layer = GriddedWeatherLayer::default();
        assert!(layer.set_frames(GriddedField::CloudCover, vec![with_nan()]));
        assert!(
            !layer.set_frames(GriddedField::CloudCover, vec![with_nan()]),
            "bit-identical NaN frames are the same content"
        );
        let mut moved_hole = with_nan();
        moved_hole.values[4] = 50.0;
        moved_hole.values[5] = f32::NAN;
        assert!(
            layer.set_frames(GriddedField::CloudCover, vec![moved_hole]),
            "a different no-data mask is new content"
        );
    }

    /// Superset pushes (the fetch scheduler re-pushes the full list after
    /// every arrival) keep the content identity of unchanged frames — the
    /// key that lets `advance` reuse their GPU textures.
    #[test]
    fn superset_push_keeps_content_identity_of_unchanged_frames() {
        let field = GriddedField::CloudCover;
        let mut layer = GriddedWeatherLayer::default();
        layer.set_frames(field, vec![frame_cc(100, 50.0), frame_cc(200, 60.0)]);
        let ids = |layer: &GriddedWeatherLayer| -> Vec<(i64, u64)> {
            layer.fields[field.index()]
                .frames
                .iter()
                .map(|f| (f.valid_time, f.content_id))
                .collect()
        };
        let before = ids(&layer);

        assert!(layer.set_frames(
            field,
            vec![
                frame_cc(100, 50.0),
                frame_cc(200, 60.0),
                frame_cc(300, 70.0),
            ],
        ));
        let after = ids(&layer);
        assert_eq!(after[..2], before[..], "unchanged frames keep identity");
        assert!(
            after[2].1 > before[1].1,
            "the new frame gets a fresh content id"
        );

        // Same valid time, new values: only that frame's identity changes.
        assert!(layer.set_frames(
            field,
            vec![
                frame_cc(100, 50.0),
                frame_cc(200, 99.0),
                frame_cc(300, 70.0),
            ],
        ));
        let replaced = ids(&layer);
        assert_eq!(replaced[0], after[0]);
        assert_eq!(replaced[2], after[2]);
        assert_ne!(replaced[1].1, after[1].1, "new content, new identity");
    }

    fn frame_cc(valid_time: i64, value: f32) -> WeatherGridFrame {
        let mut f = frame(valid_time, value);
        f.field = GriddedField::CloudCover;
        f
    }

    #[test]
    fn set_time_reports_changes_only() {
        let mut layer = GriddedWeatherLayer::default();
        assert!(layer.set_time(1000));
        assert!(!layer.set_time(1000));
        assert!(layer.set_time(2000));
        assert_eq!(layer.time(), 2000);
    }

    /// Slider moves only matter when a toggled-on field holds frames.
    #[test]
    fn visibility_check_combines_toggles_and_data() {
        let mut layer = GriddedWeatherLayer::default();
        let mut toggles = LayerToggles::standard();
        assert!(!layer.any_visible_field(&toggles), "no data, all off");

        layer.set_frames(GriddedField::PrecipRate, vec![frame(100, 1.0)]);
        assert!(
            !layer.any_visible_field(&toggles),
            "data, but precipitation toggle still off"
        );
        toggles.set(LayerId::Precipitation, true);
        assert!(layer.any_visible_field(&toggles));
        toggles.set(LayerId::Precipitation, false);
        toggles.set(LayerId::CloudCover, true);
        assert!(!layer.any_visible_field(&toggles), "wrong field toggled");
    }

    /// NaN (no data) becomes coverage 0; finite values round-trip through
    /// f16 with full coverage.
    #[test]
    fn texel_conversion_premultiplies_coverage() {
        let texels = texels_rg16(&[1.5, f32::NAN, 0.0]);
        assert_eq!(texels.len(), 12);
        let pair = |i: usize| {
            (
                half::f16::from_le_bytes([texels[i * 4], texels[i * 4 + 1]]).to_f32(),
                half::f16::from_le_bytes([texels[i * 4 + 2], texels[i * 4 + 3]]).to_f32(),
            )
        };
        assert_eq!(pair(0), (1.5, 1.0));
        assert_eq!(pair(1), (0.0, 0.0), "NaN is transparent no-data");
        assert_eq!(pair(2), (0.0, 1.0), "zero is a valid value");
    }

    /// The layer never demands redraws while idle — only in-flight
    /// conversions count.
    #[test]
    fn idle_layer_wants_no_redraw() {
        let mut layer = GriddedWeatherLayer::default();
        assert!(!layer.wants_redraw());
        layer.set_frames(GriddedField::PrecipRate, vec![frame(100, 1.0)]);
        layer.set_time(100);
        assert!(
            !layer.wants_redraw(),
            "uploads start in prepare, not in set_*"
        );
    }
}

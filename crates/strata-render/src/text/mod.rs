//! Text overlay: cosmic-text shaping → glyph rasterization into an etagere
//! atlas → instanced screen-space quads (`text.wgsl`), with grid-based
//! collision avoidance (skip-not-shift, priority by feature importance).
//!
//! ## Frame ordering
//!
//! Labels are queued *every frame* — by layers during their `prepare` (via a
//! [`LabelQueue`] handle or `&mut TextSystem`) or by the app. [`MapRenderer`]
//! calls [`TextSystem::prepare`] after every layer's `prepare` and
//! [`TextSystem::draw`] after every layer's `draw`, so labels queued by a
//! layer land in the same frame, on top of everything.
//!
//! All work resolves synchronously inside `prepare`: shaping and placement
//! are cached/cheap and the renderer's dirty flag drives redraws, so
//! [`TextSystem::wants_redraw`] is always `false`.
//!
//! [`MapRenderer`]: crate::renderer::MapRenderer

mod atlas;
mod collide;
mod pipeline;
mod shape;

use self::atlas::{AtlasFull, GlyphAtlas};
use self::collide::CollisionGrid;
use self::pipeline::{GlyphInstance, TextPipeline};
use self::shape::{ShapedLabel, Shaper};
use crate::camera::Camera;
use crate::layer::{DrawCtx, PrepareCtx};

use glam::{DVec2, Vec2};
use parking_lot::Mutex;

use std::sync::Arc;

/// Collision grid cell size, logical px.
const COLLISION_CELL_PX: f32 = 64.0;
/// Minimum clearance kept between placed label boxes, logical px.
const COLLISION_PADDING_PX: f64 = 2.0;
/// Labels whose anchor is further than this outside the viewport are culled
/// before shaping. Generous so wide labels straddling the edge still show.
const CULL_MARGIN_PX: f64 = 512.0;

/// Where a label is pinned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelAnchor {
    /// Normalized Web-Mercator world position (moves with the map).
    World(DVec2),
    /// Fixed logical-pixel screen position (HUD-style).
    ScreenPx(DVec2),
}

/// How the shaped label box hangs off the projected (and pixel-offset)
/// anchor point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelPlacement {
    /// Box centered on the anchor (area labels, place names).
    #[default]
    Center,
    /// The anchor is the box's top-center: the label hangs below it. Point
    /// symbols use this with a downward [`LabelRequest::offset_px`] so the
    /// text clears the symbol regardless of the shaped text height.
    Below,
}

/// A label to draw this frame. Queue every frame; the text system culls by
/// `min_zoom`, resolves collisions by `priority` (higher wins) and caches
/// shaping by text + size. The label box hangs off the anchor — shifted by
/// `offset_px` — according to `placement`.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelRequest {
    /// Refcounted so the per-frame re-queueing clones are pointer bumps, not
    /// heap allocations (labels are queued every frame, see module docs).
    pub text: Arc<str>,
    pub anchor: LabelAnchor,
    /// Screen-space offset in logical px (x right, y down) added to the
    /// projected anchor before placement. Zoom-independent; the collision
    /// box moves with it. Point layers use it to push idents clear of their
    /// symbol.
    pub offset_px: Vec2,
    /// How the label box is positioned relative to the offset anchor.
    pub placement: LabelPlacement,
    /// Font size in logical pixels.
    pub size_px: f32,
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
    /// Collision priority; higher wins placement. Ties break by lower `id`,
    /// then submission order.
    pub priority: u8,
    /// Hidden below this camera zoom.
    pub min_zoom: f32,
    /// Stable id for deterministic collision ordering across frames.
    pub id: u64,
}

/// Clonable, thread-safe label submission handle. Layers that cannot reach
/// the [`TextSystem`] directly (it is prepared after them by the renderer)
/// hold one of these and push during their own `prepare`.
#[derive(Clone, Default)]
pub struct LabelQueue {
    inner: Arc<Mutex<Vec<LabelRequest>>>,
}

impl LabelQueue {
    /// Queue a label for the coming frame.
    pub fn push(&self, request: LabelRequest) {
        self.inner.lock().push(request);
    }

    fn drain(&self) -> Vec<LabelRequest> {
        std::mem::take(&mut *self.inner.lock())
    }
}

/// A label that survived culling and collision this frame.
struct PlacedLabel {
    /// Label-box top-left, logical px (already snapped when at rest).
    origin: Vec2,
    shaped: Arc<ShapedLabel>,
    color: [f32; 4],
}

/// Shared text renderer drawn above all map layers.
pub struct TextSystem {
    queued: Vec<LabelRequest>,
    shared: LabelQueue,
    shaper: Shaper,
    atlas: GlyphAtlas,
    pipeline: Option<TextPipeline>,
    pipeline_failed: bool,
    /// Glyph halo color (premultiplied); fully transparent disables halos.
    /// Light map themes set a light halo so dark text reads over fills.
    halo_color: [f32; 4],
    /// Scratch instance list built each frame.
    scratch: Vec<GlyphInstance>,
    /// Mirror of what the GPU instance buffer currently holds.
    uploaded: Vec<GlyphInstance>,
    atlas_exhausted: bool,
}

impl TextSystem {
    /// Loads system fonts and creates the (initially 1024²) glyph atlas. The
    /// render pipeline is created lazily on first `prepare` (it needs the
    /// target format).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let _ = queue; // uploads happen in `prepare`; kept for signature stability
        Self {
            queued: Vec::new(),
            shared: LabelQueue::default(),
            shaper: Shaper::new(),
            atlas: GlyphAtlas::new(device),
            pipeline: None,
            pipeline_failed: false,
            halo_color: [0.0; 4],
            scratch: Vec::new(),
            uploaded: Vec::new(),
            atlas_exhausted: false,
        }
    }

    /// Set the glyph halo color (premultiplied). Fully transparent (the
    /// default) renders no halo and adds no instances; takes effect next
    /// `prepare`.
    pub fn set_halo_color(&mut self, color: [f32; 4]) {
        self.halo_color = color;
    }

    pub fn halo_color(&self) -> [f32; 4] {
        self.halo_color
    }

    /// Queue a label for the coming frame.
    pub fn queue_label(&mut self, request: LabelRequest) {
        self.queued.push(request);
    }

    /// Labels queued via [`queue_label`](Self::queue_label) since the last
    /// frame (labels pushed through a [`LabelQueue`] are not visible here).
    pub fn queued(&self) -> &[LabelRequest] {
        &self.queued
    }

    /// A clonable handle layers can use to queue labels during their own
    /// `prepare`, before this system's `prepare` runs.
    pub fn label_queue(&self) -> LabelQueue {
        self.shared.clone()
    }

    /// Always `false`: shaping, placement and uploads resolve synchronously
    /// in [`prepare`](Self::prepare); redraws are driven by camera/input/data
    /// changes, which already mark the renderer dirty.
    pub fn wants_redraw(&self) -> bool {
        false
    }

    /// Shape, declutter and upload this frame's labels.
    pub fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        let mut labels = std::mem::take(&mut self.queued);
        labels.extend(self.shared.drain());

        if self.pipeline.is_none() && !self.pipeline_failed {
            match TextPipeline::new(
                ctx.device,
                ctx.target_format,
                ctx.globals_layout,
                self.atlas.layout(),
                ctx.shaders,
            ) {
                Ok(pipeline) => self.pipeline = Some(pipeline),
                Err(error) => {
                    tracing::error!(%error, "text pipeline creation failed; labels disabled");
                    self.pipeline_failed = true;
                }
            }
        }
        if self.pipeline.is_none() {
            return;
        }

        let placed = place_labels(&mut self.shaper, ctx.camera, labels);
        ensure_glyphs(
            &mut self.shaper,
            &mut self.atlas,
            ctx.device,
            ctx.queue,
            &placed,
            &mut self.atlas_exhausted,
        );
        build_instances(
            &self.atlas,
            &placed,
            ctx.camera.viewport().scale_factor(),
            self.halo_color,
            &mut self.scratch,
        );

        if self.scratch != self.uploaded
            && let Some(pipeline) = self.pipeline.as_mut()
        {
            tracing::trace!(
                labels = placed.len(),
                glyphs = self.scratch.len(),
                "text instances uploaded"
            );
            pipeline.upload(ctx.device, ctx.queue, &self.scratch);
            std::mem::swap(&mut self.uploaded, &mut self.scratch);
        }
    }

    /// Draw all placed labels in one instanced call (after every map layer).
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ctx: &DrawCtx<'_>) {
        let _ = ctx;
        if let Some(pipeline) = &self.pipeline {
            pipeline.draw(pass, self.atlas.bind_group());
        }
    }
}

/// Cull, shape and declutter; returns surviving labels in draw order.
fn place_labels(
    shaper: &mut Shaper,
    camera: &Camera,
    labels: Vec<LabelRequest>,
) -> Vec<PlacedLabel> {
    struct Candidate {
        anchor: DVec2,
        placement: LabelPlacement,
        shaped: Arc<ShapedLabel>,
        priority: u8,
        id: u64,
        color: [f32; 4],
    }

    let zoom = camera.zoom() as f32;
    let scale = camera.viewport().scale_factor() as f64;
    let viewport = camera.viewport().logical_size();
    // Snapping to the physical pixel grid keeps glyphs texel-exact at rest;
    // during animations fractional positions keep motion smooth.
    let snap = !camera.is_animating();

    let mut candidates = Vec::with_capacity(labels.len());
    for label in labels {
        if zoom < label.min_zoom
            || label.text.is_empty()
            || !label.size_px.is_finite()
            || label.size_px <= 0.5
        {
            continue;
        }
        // The pixel offset is part of the anchor: culling and the collision
        // box below both see the final, offset position.
        let anchor = match label.anchor {
            LabelAnchor::World(world) => camera.project(world),
            LabelAnchor::ScreenPx(px) => px,
        } + label.offset_px.as_dvec2();
        if anchor.x < -CULL_MARGIN_PX
            || anchor.y < -CULL_MARGIN_PX
            || anchor.x > viewport.x + CULL_MARGIN_PX
            || anchor.y > viewport.y + CULL_MARGIN_PX
        {
            continue;
        }
        let shaped = shaper.shape(&label.text, label.size_px, camera.viewport().scale_factor());
        if shaped.glyphs.is_empty() {
            continue;
        }
        candidates.push(Candidate {
            anchor,
            placement: label.placement,
            shaped,
            priority: label.priority,
            id: label.id,
            color: label.color,
        });
    }

    // Higher priority first; stable sort keeps submission order for full ties.
    candidates.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));

    let mut grid = CollisionGrid::new(COLLISION_CELL_PX);
    let mut placed = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let size = candidate.shaped.size.as_dvec2();
        let mut origin = match candidate.placement {
            LabelPlacement::Center => candidate.anchor - size * 0.5,
            // Top edge at the anchor, centered horizontally: the box hangs
            // fully below the anchor point.
            LabelPlacement::Below => candidate.anchor - DVec2::new(size.x * 0.5, 0.0),
        };
        if snap {
            origin = (origin * scale).round() / scale;
        }
        let min = origin - DVec2::splat(COLLISION_PADDING_PX);
        let max = origin + size + DVec2::splat(COLLISION_PADDING_PX);
        if max.x < 0.0 || max.y < 0.0 || min.x > viewport.x || min.y > viewport.y {
            continue; // fully offscreen
        }
        if !grid.try_insert(min.as_vec2(), max.as_vec2()) {
            continue; // skip-not-shift
        }
        placed.push(PlacedLabel {
            origin: origin.as_vec2(),
            shaped: candidate.shaped,
            color: candidate.color,
        });
    }
    placed
}

/// Rasterize and upload every glyph the placed labels need. Grows the atlas
/// (which resets it) and restarts on overflow; at maximum size, unplaceable
/// glyphs are marked empty so they are not retried every frame.
fn ensure_glyphs(
    shaper: &mut Shaper,
    atlas: &mut GlyphAtlas,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    placed: &[PlacedLabel],
    exhausted: &mut bool,
) {
    'attempt: loop {
        for label in placed {
            for glyph in &label.shaped.glyphs {
                if atlas.lookup(&glyph.key).is_some() {
                    continue;
                }
                let Some(raster) = shaper.rasterize(glyph.key) else {
                    atlas.mark_empty(glyph.key);
                    continue;
                };
                match atlas.ensure(queue, glyph.key, &raster) {
                    Ok(_) => {}
                    Err(AtlasFull) => {
                        if atlas.grow(device) {
                            tracing::debug!(size = atlas.size(), "glyph atlas grown");
                            continue 'attempt;
                        }
                        if !*exhausted {
                            tracing::warn!(
                                size = atlas.size(),
                                "glyph atlas exhausted; some glyphs will not render"
                            );
                            *exhausted = true;
                        }
                        atlas.mark_empty(glyph.key);
                    }
                }
            }
        }
        break;
    }
}

/// Halo offsets in logical px: 8 directions, 1px ring. Drawn under the
/// text pass so the union of offset copies forms an outline.
const HALO_OFFSETS_PX: [Vec2; 8] = [
    Vec2::new(-1.0, 0.0),
    Vec2::new(1.0, 0.0),
    Vec2::new(0.0, -1.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(-0.7, -0.7),
    Vec2::new(0.7, -0.7),
    Vec2::new(-0.7, 0.7),
    Vec2::new(0.7, 0.7),
];

/// Emit one quad per resident glyph of every placed label. With an opaque
/// `halo` color, every label is first emitted as a ring of halo-colored
/// copies (all labels' halos before any text, so a neighboring label's halo
/// never paints over already-drawn text).
fn build_instances(
    atlas: &GlyphAtlas,
    placed: &[PlacedLabel],
    scale_factor: f32,
    halo: [f32; 4],
    out: &mut Vec<GlyphInstance>,
) {
    out.clear();
    let inv_atlas = 1.0 / atlas.size() as f32;
    let inv_scale = 1.0 / scale_factor;
    let emit = |label: &PlacedLabel, shift: Vec2, color: [f32; 4], out: &mut Vec<GlyphInstance>| {
        for glyph in &label.shaped.glyphs {
            let Some(Some(slot)) = atlas.lookup(&glyph.key) else {
                continue; // empty glyph (space) or atlas overflow
            };
            let offset_px = Vec2::new(
                (glyph.pen_px.x + slot.left) as f32,
                (glyph.pen_px.y - slot.top) as f32,
            ) * inv_scale;
            out.push(GlyphInstance {
                pos_px: (label.origin + offset_px + shift).into(),
                size_px: [
                    slot.width as f32 * inv_scale,
                    slot.height as f32 * inv_scale,
                ],
                uv_min: [slot.x as f32 * inv_atlas, slot.y as f32 * inv_atlas],
                uv_max: [
                    (slot.x + slot.width) as f32 * inv_atlas,
                    (slot.y + slot.height) as f32 * inv_atlas,
                ],
                color,
            });
        }
    };
    if halo[3] > 0.0 {
        for label in placed {
            for shift in HALO_OFFSETS_PX {
                emit(label, shift, halo, out);
            }
        }
    }
    for label in placed {
        emit(label, Vec2::ZERO, label.color, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Viewport;

    use glam::UVec2;

    fn test_camera() -> Camera {
        Camera::new(Viewport::new(UVec2::new(800, 600), 1.0))
    }

    fn label(text: &str, x: f64, y: f64, priority: u8, id: u64) -> LabelRequest {
        LabelRequest {
            text: text.into(),
            anchor: LabelAnchor::ScreenPx(DVec2::new(x, y)),
            offset_px: Vec2::ZERO,
            placement: LabelPlacement::Center,
            size_px: 14.0,
            color: [1.0, 1.0, 1.0, 1.0],
            priority,
            min_zoom: 0.0,
            id,
        }
    }

    fn shaper_or_skip() -> Option<Shaper> {
        let shaper = Shaper::new();
        if shaper.has_fonts() {
            Some(shaper)
        } else {
            eprintln!("skipping: no system fonts available in this environment");
            None
        }
    }

    #[test]
    fn overlapping_labels_drop_the_lower_priority() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let labels = vec![
            label("low priority", 400.0, 300.0, 1, 1),
            label("high priority", 405.0, 302.0, 9, 2),
        ];
        let placed = place_labels(&mut shaper, &camera, labels);
        assert_eq!(placed.len(), 1, "overlapping labels must collapse to one");
        // The survivor is the high-priority one: same anchor area, but the
        // shaped text differs in width — verify via the shaped glyph count.
        let winner = shaper.shape(&Arc::from("high priority"), 14.0, 1.0);
        assert_eq!(placed[0].shaped.glyphs.len(), winner.glyphs.len());
    }

    #[test]
    fn disjoint_labels_both_place() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let labels = vec![
            label("EDDF", 100.0, 100.0, 1, 1),
            label("EDDM", 600.0, 400.0, 1, 2),
        ];
        let placed = place_labels(&mut shaper, &camera, labels);
        assert_eq!(placed.len(), 2);
    }

    #[test]
    fn far_offscreen_labels_are_culled() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let labels = vec![label("EDDF", -2000.0, 300.0, 1, 1)];
        assert!(place_labels(&mut shaper, &camera, labels).is_empty());
    }

    #[test]
    fn labels_below_min_zoom_are_hidden() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera(); // zoom 6
        let mut request = label("EDDF", 400.0, 300.0, 1, 1);
        request.min_zoom = 10.0;
        assert!(place_labels(&mut shaper, &camera, vec![request]).is_empty());
    }

    #[test]
    fn pixel_offset_shifts_the_placed_box() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let centered = place_labels(
            &mut shaper,
            &camera,
            vec![label("EDDF", 400.0, 300.0, 1, 1)],
        );
        let mut offset = label("EDDF", 400.0, 300.0, 1, 1);
        offset.offset_px = Vec2::new(10.0, 13.0);
        let shifted = place_labels(&mut shaper, &camera, vec![offset]);
        assert_eq!(centered.len(), 1);
        assert_eq!(shifted.len(), 1);
        let delta = shifted[0].origin - centered[0].origin;
        assert_eq!(
            delta,
            Vec2::new(10.0, 13.0),
            "origin must move by offset_px"
        );
    }

    #[test]
    fn below_placement_hangs_the_box_under_the_anchor() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let mut request = label("EDDF", 400.0, 300.0, 1, 1);
        request.placement = LabelPlacement::Below;
        let placed = place_labels(&mut shaper, &camera, vec![request]);
        assert_eq!(placed.len(), 1);
        let origin = placed[0].origin;
        let size = placed[0].shaped.size;
        // Top edge at the anchor (box fully below it), centered horizontally.
        assert_eq!(origin.y, 300.0, "box top must sit at the anchor y");
        assert!(
            (origin.x + size.x * 0.5 - 400.0).abs() <= 0.5,
            "box must stay centered: origin {origin:?} size {size:?}"
        );
    }

    /// The collision box must live at the OFFSET position: a high-priority
    /// label pushed away from its anchor must not block a low-priority label
    /// sitting at the raw anchor (and must block one at the offset spot).
    #[test]
    fn collision_box_follows_the_offset_position() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        let mut pushed = label("high priority", 400.0, 300.0, 9, 1);
        pushed.offset_px = Vec2::new(0.0, 120.0);
        let at_anchor = label("low priority", 400.0, 300.0, 1, 2);
        let placed = place_labels(
            &mut shaper,
            &camera,
            vec![pushed.clone(), at_anchor.clone()],
        );
        assert_eq!(placed.len(), 2, "offset label must vacate the raw anchor");

        let mut clashing = at_anchor;
        clashing.offset_px = Vec2::new(0.0, 120.0);
        let placed = place_labels(&mut shaper, &camera, vec![pushed, clashing]);
        assert_eq!(
            placed.len(),
            1,
            "boxes at the same offset spot must collide"
        );
    }

    #[test]
    fn resting_camera_snaps_label_origins_to_whole_pixels() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let camera = test_camera();
        assert!(!camera.is_animating());
        let labels = vec![label("EDDF", 400.37, 300.61, 1, 1)];
        let placed = place_labels(&mut shaper, &camera, labels);
        assert_eq!(placed.len(), 1);
        let origin = placed[0].origin;
        // scale_factor 1.0 → integer logical px.
        assert_eq!(origin.x.fract(), 0.0, "origin.x not snapped: {origin:?}");
        assert_eq!(origin.y.fract(), 0.0, "origin.y not snapped: {origin:?}");
    }
}

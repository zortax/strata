//! The custom-painted corridor profile view (design §3.3 "Profile" tab,
//! plan §5.2 `ui/profile_view/`): terrain silhouette, airspace bands with
//! sloped AGL edges, the draggable planned-altitude line, MSA / freezing
//! reference lines, conflicts and the scrub crosshair — gpui `PathBuilder`
//! painting plus the window text system (the chart components cannot do
//! sloped bands or line dragging).
//!
//! # Seam (consumed by the profile-drawer chrome)
//!
//! The drawer owns chrome, tabs and resizing; this module owns everything
//! inside the Profile tab:
//!
//! ```ignore
//! let profile = cx.new(|cx| ProfileView::new(app_state.clone(), cx));
//! // … drawer body:
//! div().size_full().child(profile.clone())
//! ```
//!
//! [`ProfileView`] subscribes to the flight lifecycle itself and rebuilds
//! its input series ([`ProfileSeries`]) per compute generation; the host
//! never feeds it data. Scrubbing goes through the one app-level sync
//! point ([`AppState::set_profile_scrub`]) so the map marker follows;
//! clicking an airspace band raises the standard selection (Inspect tab);
//! dragging the planned line within a leg commits the leg altitude
//! (snapped to 100 ft) through [`AppState::set_leg_altitude`] — the
//! debounced recompute flows automatically.
//!
//! # Performance
//!
//! Painting replays a cached pixel scene built in three stages:
//!
//! 1. [`world`] — world-space geometry (simplified to sub-pixel
//!    tolerance), pre-tessellated normalized fill meshes and label
//!    contents, cached per compute generation / style params,
//!    **resolution-independent**;
//! 2. [`layout`] — the per-frame px remap through the current bounds'
//!    [`mapping::ChartMapping`]: vertex/mesh mapping, dash quads, tick
//!    generation and label-fit decisions;
//! 3. [`scene`] — assembling paint ops: cached meshes become paths
//!    directly, only the few solid outlines stroke through lyon, and
//!    labels bind to the content-keyed shaped-text cache (shaping happens
//!    once per distinct label; resize frames only *reposition* the runs).
//!
//! A drawer drag-resize or window resize therefore re-runs only stages
//! 2–3 in the same frame ([`scene::rebuild_decision`]) — the chart tracks
//! the panel edge every frame instead of debouncing to quiescence, well
//! under a millisecond per remap for typical routes. Per-frame work for
//! scrub/drag is two thin overlay draws.
//!
//! The X axis is distance-only this phase; `XAxisMode` is the seam for the
//! design's distance↔time toggle.

mod layout;
mod mapping;
mod scene;
mod series;
pub mod sparkline;
mod world;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, canvas, div, point, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use strata_data::domain::{Meters, MetersAmsl};
use strata_data::store::Feature;
use strata_plan::flight::PlannedAltitude;
use strata_render::MapTheme;
use strata_render::layers::style::airspace_style;

use crate::state::{AppState, AppStateEvent, ComputeState};

use layout::{chart_mapping, layout_world};
use mapping::{FEET_PER_METER, leg_at, snap_altitude_feet};
use scene::{
    HoverTarget, RebuildDecision, Scene, ShapedTextCache, band_label_color, linear_to_rgba,
    paint_overlay_label, paint_scene, realize_scene, rebuild_decision, shape_overlay_label,
};
pub use series::{ProfileSeries, ScrubReadout};
use world::{BandStyle, Palette, SceneParams, WorldScene, build_world_scene};
// Part of the public series shape (`ProfileSeries::bands`), re-exported
// for the drawer even though this module never names them itself.
#[allow(unused_imports)]
pub use series::{BandSeries, BandStation};

/// Width of the hover readout card.
const READOUT_WIDTH_PX: f32 = 230.;
/// Scrub marker radius on the planned line.
const SCRUB_MARKER_RADIUS: f32 = 3.5;

/// X-axis mode. Distance-only this milestone; the elapsed-time toggle
/// (design §3.3) slots in as a second variant without touching the paint
/// pipeline (the mapping is the only consumer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum XAxisMode {
    #[default]
    Distance,
}

/// The shared scene-cache cell, one slot per pipeline stage: the
/// world-space scene (keyed by params, never by size), the realized px
/// scene (keyed by bounds) and the content-keyed shaped-text cache.
#[derive(Default)]
struct SceneCache {
    world: Option<WorldScene>,
    scene: Option<Scene>,
    text: ShapedTextCache,
}

/// An in-progress planned-altitude drag on one leg.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    leg: usize,
    /// Live preview altitude, already snapped to 100 ft.
    preview_ft: f64,
    /// Whether the pointer moved since mouse-down (a click on the line
    /// without movement commits nothing).
    moved: bool,
}

/// Current hover state (drives cursor, crosshair readout and band picks).
#[derive(Debug, Clone, PartialEq)]
struct Hover {
    target: HoverTarget,
    /// Pointer position in window px.
    position: (f32, f32),
    /// Along-track meters under the pointer.
    along_m: f64,
    readout: ScrubReadout,
}

/// The Profile tab's content view. See the module docs for the seam.
pub struct ProfileView {
    app_state: Entity<AppState>,
    /// Input series of the current compute generation; `None` while the
    /// flight has no computed outputs.
    series: Option<Rc<ProfileSeries>>,
    /// Monotonic generation of `series` — the scene-cache identity (an
    /// `Rc` pointer would be ABA-prone across rebuilds).
    series_generation: u64,
    /// The staged paint caches (world scene / px scene / shaped text),
    /// shared with the canvas closures. See [`scene::rebuild_decision`].
    scene: Rc<RefCell<SceneCache>>,
    hover: Option<Hover>,
    drag: Option<Drag>,
    /// Band index pressed on mouse-down (click-to-select completes on
    /// mouse-up over the same band).
    pressed_band: Option<usize>,
    // The distance↔time axis seam (design §3.3); the mapping consumes it
    // once the time mode exists.
    #[allow(dead_code)]
    x_axis: XAxisMode,
    _subscriptions: Vec<Subscription>,
}

impl ProfileView {
    /// Creates the view bound to the app state. The host (drawer) only
    /// embeds the entity — all data flow is internal.
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.subscribe(&app_state, Self::on_app_state_event)];
        let mut this = Self {
            app_state,
            series: None,
            series_generation: 0,
            scene: Rc::new(RefCell::new(SceneCache::default())),
            hover: None,
            drag: None,
            pressed_band: None,
            x_axis: XAxisMode::default(),
            _subscriptions: subscriptions,
        };
        this.rebuild_series(cx);
        this
    }

    fn on_app_state_event(
        &mut self,
        _app_state: Entity<AppState>,
        event: &AppStateEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::FlightOpened
            | AppStateEvent::FlightChanged
            | AppStateEvent::FlightComputed
            | AppStateEvent::FlightClosed => {
                self.rebuild_series(cx);
                cx.notify();
            }
            // The map (or a future surface) moved the scrub — redraw the
            // crosshair.
            AppStateEvent::ProfileScrubChanged => cx.notify(),
            // A drawer-header weather toggle flipped: the scene params
            // changed, the cached scene rebuilds on the next paint.
            AppStateEvent::ProfileWeatherChanged => cx.notify(),
            _ => {}
        }
    }

    /// Rebuilds the input series from the open flight's computed outputs
    /// (one flattening per compute generation).
    fn rebuild_series(&mut self, cx: &mut Context<Self>) {
        let state = self.app_state.read(cx);
        let next = state.flight.as_ref().and_then(|flight| {
            let computed = flight.computed.as_deref()?;
            Some(Rc::new(ProfileSeries::build(&flight.doc, computed)))
        });
        self.series = next;
        self.series_generation += 1;
        self.scene.replace(SceneCache::default());
        self.hover = None;
        // A vanished route cannot keep an in-progress drag meaningful.
        if self.series.is_none() {
            self.drag = None;
            self.pressed_band = None;
        }
    }

    // ── interaction ─────────────────────────────────────────────────────

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drag.is_some() {
            // The window-level listener registered during paint owns drag
            // updates (it keeps working outside the element).
            return;
        }
        let position = (f32::from(event.position.x), f32::from(event.position.y));
        let next = {
            let cache = self.scene.borrow();
            let (Some(scene), Some(series)) = (cache.scene.as_ref(), self.series.as_ref()) else {
                return;
            };
            match scene.hover_target(position) {
                HoverTarget::Outside => None,
                target => {
                    let along_m = scene.mapping.along_at(position.0);
                    Some(Hover {
                        target,
                        position,
                        along_m,
                        readout: series.readout_at(along_m),
                    })
                }
            }
        };
        let scrub = next.as_ref().map(|hover| Meters(hover.along_m));
        self.app_state
            .update(cx, |state, cx| state.set_profile_scrub(scrub, cx));
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        enum Press {
            StartDrag(Drag),
            Band(usize),
            None,
        }
        let position = (f32::from(event.position.x), f32::from(event.position.y));
        // Scoped borrow: decide first, mutate after.
        let press = {
            let cache = self.scene.borrow();
            let (Some(scene), Some(series)) = (cache.scene.as_ref(), self.series.as_ref()) else {
                return;
            };
            match scene.hover_target(position) {
                HoverTarget::PlannedLine => {
                    let along_m = scene.mapping.along_at(position.0);
                    match leg_at(&series.leg_ends_m, along_m) {
                        Some(leg) => {
                            // Seed the preview from the line's current
                            // altitude so a jitter-free grab does not jump.
                            let current = series.planned_at(along_m).unwrap_or(0.0);
                            Press::StartDrag(Drag {
                                leg,
                                preview_ft: snap_altitude_feet(current),
                                moved: false,
                            })
                        }
                        None => Press::None,
                    }
                }
                HoverTarget::Band(band) => Press::Band(band),
                HoverTarget::Chart | HoverTarget::Outside => Press::None,
            }
        };
        match press {
            Press::StartDrag(drag) => {
                self.drag = Some(drag);
                self.pressed_band = None;
                cx.notify();
            }
            Press::Band(band) => self.pressed_band = Some(band),
            Press::None => self.pressed_band = None,
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.commit_drag(cx);
            return;
        }
        let Some(pressed) = self.pressed_band.take() else {
            return;
        };
        let position = (f32::from(event.position.x), f32::from(event.position.y));
        let airspace = {
            let cache = self.scene.borrow();
            let (Some(scene), Some(series)) = (cache.scene.as_ref(), self.series.as_ref()) else {
                return;
            };
            if scene.hover_target(position) != HoverTarget::Band(pressed) {
                return;
            }
            let Some(band) = series.bands.get(pressed) else {
                return;
            };
            band.airspace.clone()
        };
        // Click a band → the existing selection flow (Inspect tab card).
        self.app_state.update(cx, |state, cx| {
            state.set_selection(vec![Feature::Airspace(airspace)], cx);
        });
    }

    /// Live drag update (called from the window-level move listener).
    fn update_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let preview_ft = {
            let cache = self.scene.borrow();
            let Some(scene) = cache.scene.as_ref() else {
                return;
            };
            snap_altitude_feet(scene.mapping.alt_at(f32::from(position.y)))
        };
        if let Some(drag) = &mut self.drag {
            let changed = drag.preview_ft != preview_ft;
            drag.moved = drag.moved || changed;
            if changed {
                drag.preview_ft = preview_ft;
                cx.notify();
            }
        }
    }

    /// Commits the dragged leg altitude through the document mutation API
    /// (design §3.3: re-plans that leg, conflicts re-evaluate live via the
    /// debounced recompute).
    fn commit_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.moved {
            let altitude = PlannedAltitude::Amsl(MetersAmsl::from_feet(drag.preview_ft));
            self.app_state.update(cx, |state, cx| {
                state.set_leg_altitude(drag.leg, Some(altitude), cx);
            });
        }
        cx.notify();
    }

    fn on_hover_changed(&mut self, hovered: &bool, cx: &mut Context<Self>) {
        if *hovered || self.drag.is_some() {
            return;
        }
        // Pointer left the view: clear the crosshair (and the map marker).
        self.hover = None;
        self.app_state
            .update(cx, |state, cx| state.set_profile_scrub(None, cx));
        cx.notify();
    }

    // ── render helpers ──────────────────────────────────────────────────

    fn cursor_style(&self) -> CursorStyle {
        if self.drag.is_some() {
            return CursorStyle::ResizeUpDown;
        }
        match self.hover.as_ref().map(|h| h.target) {
            Some(HoverTarget::PlannedLine) => CursorStyle::ResizeUpDown,
            Some(HoverTarget::Band(_)) | Some(HoverTarget::Chart) => CursorStyle::Crosshair,
            _ => CursorStyle::Arrow,
        }
    }

    /// Palette + per-band styles resolved from the UI theme and the active
    /// map theme (band colors must match the map — design §3.3).
    fn resolve_styles(&self, cx: &App) -> (Palette, Vec<BandStyle>) {
        let map_theme = MapTheme::by_id(self.app_state.read(cx).map_theme_id).unwrap_or_default();
        let theme = cx.theme();
        let terrain = gpui::Hsla::from(linear_to_rgba(map_theme.terrain.light_tint));
        let palette = Palette {
            axis_text: theme.muted_foreground,
            grid: theme.border.opacity(0.45),
            terrain_fill: terrain.opacity(0.20),
            terrain_stroke: terrain.opacity(0.55),
            obstacle: gpui::Hsla::from(linear_to_rgba(map_theme.symbols.obstacle)),
            planned: gpui::Hsla::from(linear_to_rgba(map_theme.route.line)),
            marker_fill: gpui::Hsla::from(linear_to_rgba(map_theme.route.handle_fill)),
            msa: theme.warning.opacity(0.9),
            // "Cyan-ish" per design §3.3 — the info token is the theme's
            // cool accent.
            freezing: theme.info,
            // The cloud-base band reads as weather, not advice: muted grey.
            cloud_base: theme.muted_foreground.opacity(0.9),
            danger: theme.danger,
            warning: theme.warning,
        };
        let band_styles = self
            .series
            .as_ref()
            .map(|series| {
                series
                    .bands
                    .iter()
                    .map(|band| {
                        let style = airspace_style(&map_theme.airspace, band.style);
                        BandStyle {
                            fill: gpui::Hsla::from(linear_to_rgba(style.fill)),
                            border: gpui::Hsla::from(linear_to_rgba(style.border)),
                            border_width: style.border_width_px,
                            dash: style.dash_px,
                            label: gpui::Hsla::from(band_label_color(style.border)),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        (palette, band_styles)
    }

    /// The "nothing to draw yet" body (no flight / not computed).
    fn render_placeholder(&self, cx: &Context<Self>) -> AnyElement {
        let state = self.app_state.read(cx);
        let hint = state
            .flight
            .as_ref()
            .map(|flight| match &flight.compute_state {
                ComputeState::Pending => "Computing…".to_owned(),
                ComputeState::Computed => "Profile data unavailable.".to_owned(),
                ComputeState::NotComputable(reason) => format!("Plan incomplete: {reason}."),
                ComputeState::Failed(error) => format!("Compute failed: {error}"),
            });
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("The profile appears once the route computes."),
            )
            .children(hint.map(|hint| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                    .child(hint)
            }))
            .into_any_element()
    }

    /// The hover readout card (distance, ETA, terrain, MSA, airspace
    /// stack), positioned beside the crosshair.
    fn render_readout(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let hover = self.hover.as_ref()?;
        if self.drag.is_some() {
            return None;
        }
        let bounds = self.scene.borrow().scene.as_ref().map(Scene::bounds)?;
        let rel_x = hover.position.0 - f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width);
        // Flip to the left of the cursor near the right edge.
        let left = if rel_x + 14.0 + READOUT_WIDTH_PX > width - 8.0 {
            (rel_x - 14.0 - READOUT_WIDTH_PX).max(8.0)
        } else {
            rel_x + 14.0
        };

        let readout = &hover.readout;
        let dash = || "—".to_owned();
        let eta = readout
            .eta
            .map_or_else(dash, |eta| eta.format("%H:%MZ").to_string());
        let terrain = readout
            .terrain_m
            .map_or_else(dash, |m| format!("{:.0} ft", m * FEET_PER_METER));
        let msa = readout
            .msa_m
            .map_or_else(dash, |m| format!("{:.0} ft", m * FEET_PER_METER));

        let row = |label: &str, value: String| {
            h_flex()
                .gap_2()
                .justify_between()
                .text_xs()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(label.to_owned()),
                )
                .child(value)
        };

        Some(
            v_flex()
                .absolute()
                .top(px(28.))
                .left(px(left))
                .w(px(READOUT_WIDTH_PX))
                .p_2()
                .gap_0p5()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover.opacity(0.92))
                .text_color(cx.theme().popover_foreground)
                .shadow_md()
                .child(row("Distance", format!("{:.1} NM", readout.distance_nm)))
                .child(row("ETA", eta))
                .child(row("Terrain", terrain))
                .child(row("MSA", msa))
                .when(!readout.stack.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .pt_0p5()
                            .child("AIRSPACE"),
                    )
                    .children(
                        readout
                            .stack
                            .iter()
                            .map(|label| div().text_xs().child(label.clone())),
                    )
                })
                .into_any_element(),
        )
    }

    /// The painted chart: cached scene + dynamic overlays. The weather
    /// overlays (freezing level, cloud base) follow the drawer header's
    /// toggles through [`AppState::profile_weather`] (design §3.3).
    fn render_canvas(&self, series: Rc<ProfileSeries>, cx: &mut Context<Self>) -> AnyElement {
        let (palette, band_styles) = self.resolve_styles(cx);
        let overlays = self.app_state.read(cx).profile_weather;
        let params = SceneParams {
            generation: self.series_generation,
            palette: palette.clone(),
            band_styles,
            show_freezing: overlays.freezing,
            show_cloud_base: overlays.cloud_base,
        };

        let scene_cache = self.scene.clone();
        let prepaint_series = series.clone();
        let prepaint = move |bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut App| {
            let mut cache = scene_cache.borrow_mut();
            let cache = &mut *cache;
            let world_ok = cache
                .world
                .as_ref()
                .is_some_and(|world| world.params_match(&params));
            let decision =
                rebuild_decision(world_ok, cache.scene.as_ref().map(Scene::bounds), bounds);
            if decision == RebuildDecision::Valid {
                return;
            }
            let started = std::time::Instant::now();
            if decision == RebuildDecision::BuildWorld {
                // New compute generation / theme / toggle: re-extract the
                // world-space geometry; shaped labels may have changed too.
                cache.world = Some(build_world_scene(&prepaint_series, params));
                cache.text.clear();
            }
            let Some(world) = cache.world.as_ref() else {
                return; // unreachable: just built above
            };
            // The cheap per-frame remap (drawer drag-resize, window
            // resize): map the cached world geometry through the current
            // bounds and reposition cached shaped labels — same frame, so
            // the chart tracks the panel edge with no lag.
            let mapping = chart_mapping(world, bounds);
            let text = &mut cache.text;
            let mut measure = |content: &gpui::SharedString, color: gpui::Hsla| {
                text.width(content, color, window)
            };
            let layout = layout_world(world, &mapping, &mut measure);
            cache.scene = Some(realize_scene(
                layout,
                mapping,
                bounds,
                &mut cache.text,
                window,
            ));
            tracing::trace!(
                ?decision,
                elapsed_us = started.elapsed().as_micros() as u64,
                "profile scene rebuilt"
            );
        };

        let scene_cache = self.scene.clone();
        let overlay = OverlayInput {
            scrub_m: self.app_state.read(cx).profile_scrub.map(|m| m.0),
            drag: self.drag,
            series,
            palette,
            view: cx.entity().downgrade(),
        };
        let paint = move |_bounds: Bounds<Pixels>, (), window: &mut Window, cx: &mut App| {
            let cache = scene_cache.borrow();
            let Some(scene) = cache.scene.as_ref() else {
                return;
            };
            paint_scene(scene, window, cx);
            paint_overlays(scene, &overlay, window, cx);
        };

        canvas(prepaint, paint)
            .absolute()
            .size_full()
            .into_any_element()
    }
}

/// Everything the per-frame overlay pass needs (captured at render time).
struct OverlayInput {
    scrub_m: Option<f64>,
    drag: Option<Drag>,
    series: Rc<ProfileSeries>,
    palette: Palette,
    view: WeakEntity<ProfileView>,
}

/// Paints the dynamic layers over the cached scene: the scrub crosshair +
/// marker, the drag preview — and (while dragging) registers the
/// window-level mouse listeners that keep the drag alive outside the
/// element (the gpui-component resize-handle pattern).
fn paint_overlays(scene: &Scene, overlay: &OverlayInput, window: &mut Window, cx: &mut App) {
    let mapping = &scene.mapping;
    let (ox, oy) = mapping.origin();
    let (_, h) = mapping.size();

    // Scrub crosshair (synced with the map marker via AppState).
    if let Some(scrub_m) = overlay.scrub_m {
        let x = mapping.x_at(scrub_m);
        let mut builder = gpui::PathBuilder::stroke(px(1.0));
        builder.move_to(point(px(x), px(oy)));
        builder.line_to(point(px(x), px(oy + h)));
        if let Ok(path) = builder.build() {
            window.paint_path(path, overlay.palette.axis_text.opacity(0.6));
        }
        if let Some(y) = scene.planned_y_at(x) {
            let r = SCRUB_MARKER_RADIUS;
            window.paint_quad(
                gpui::fill(
                    Bounds::new(
                        point(px(x - r), px(y - r)),
                        gpui::size(px(2. * r), px(2. * r)),
                    ),
                    overlay.palette.marker_fill,
                )
                .corner_radii(px(r)),
            );
        }
    }

    // Drag preview: the previewed cruise altitude across the dragged leg.
    if let Some(drag) = overlay.drag {
        let start_m = if drag.leg == 0 {
            0.0
        } else {
            overlay
                .series
                .leg_ends_m
                .get(drag.leg - 1)
                .copied()
                .unwrap_or(0.0)
        };
        let end_m = overlay
            .series
            .leg_ends_m
            .get(drag.leg)
            .copied()
            .unwrap_or(overlay.series.total_m);
        let y = mapping.y_at(mapping.clamp_alt(drag.preview_ft / FEET_PER_METER));
        let (x0, x1) = (mapping.x_at(start_m), mapping.x_at(end_m));

        let mut builder = gpui::PathBuilder::stroke(px(2.0)).dash_array(&[px(7.0), px(4.0)]);
        builder.move_to(point(px(x0), px(y)));
        builder.line_to(point(px(x1), px(y)));
        if let Ok(path) = builder.build() {
            window.paint_path(path, overlay.palette.planned.opacity(0.9));
        }
        let label = shape_overlay_label(
            format!("{:.0} ft", drag.preview_ft),
            overlay.palette.planned,
            window,
        );
        let label_x = ((x0 + x1) / 2.0 - f32::from(label.width()) / 2.0).max(ox + 4.0);
        paint_overlay_label(&label, point(px(label_x), px(y - 18.0)), window, cx);

        // Window-level listeners own the drag while it lasts: moves keep
        // updating outside the element; release anywhere commits.
        let view = overlay.view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |this, cx| this.update_drag(event.position, cx))
                    .ok();
            }
        });
        let view = overlay.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                view.update(cx, |this, cx| this.commit_drag(cx)).ok();
            }
        });
    }
}

impl Render for ProfileView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(series) = self.series.clone() else {
            return div()
                .size_full()
                .child(self.render_placeholder(cx))
                .into_any_element();
        };

        div()
            .id("profile-view")
            .relative()
            .size_full()
            .cursor(self.cursor_style())
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.on_hover_changed(hovered, cx);
            }))
            .child(self.render_canvas(series, cx))
            .children(self.render_readout(cx))
            .into_any_element()
    }
}

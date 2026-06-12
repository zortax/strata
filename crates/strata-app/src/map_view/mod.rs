//! The embedded wgpu map: owns the [`MapRenderer`] (shared with the paint
//! closure behind a `parking_lot::Mutex`), translates gpui mouse events to
//! [`MapInput`], paints the offscreen texture via `window.paint_surface`,
//! and feeds the renderer with store features.
//!
//! Feeding is eager, so aero features feel "already there":
//! - A startup **warm feed** loads every airspace plus all airports/navaids
//!   the store holds — never a country subset (guarded by
//!   [`WARM_FEED_MAX_AIRSPACES`]) — and marks the coverage *global* —
//!   camera moves then only re-query the zoom-gated point kinds
//!   (reporting points, obstacles).
//! - A **fly-to** immediately feeds the bbox the camera will land on
//!   ([`fly_target_snapshot`]) instead of waiting for animation + settle.
//! - Long pans/zooms **stream** periodic feeds of the moving viewport; the
//!   settle feed stays as the catch-all.
//!
//! In planning mode the view also owns the route-editing interactions
//! (handle drags, rubber-band inserts, the right-click menu) — see the
//! [`route_edit`] submodule; explorer-mode input is untouched.

mod gridded;
mod route_edit;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Bounds, Context, DevicePixels, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled as _, Subscription, Task,
    WeakEntity, Window, canvas, div, size,
};
use gpui_component::ActiveTheme as _;
use gpui_component::menu::ContextMenuExt as _;
use parking_lot::Mutex;
use strata_data::domain::{BoundingBox, LatLon as GeoLatLon};
use strata_data::store::{Feature, Store};
use strata_render::glam::{DVec2, UVec2};
use strata_render::{
    CameraSnapshot, LayerId, MapInput, MapRenderer, MapTheme, Redraw, RenderAirspace,
    RenderPointFeature, RenderRoute, RendererConfig, TileSource,
};

use crate::convert;
use crate::gridded_weather::GriddedWeatherController;
use crate::state::{AppState, AppStateEvent, world_bbox};
use crate::tile_sources::StoreTerrainSource;

/// Startup camera framing Germany.
const HOME_LAT: f64 = 51.2;
const HOME_LON: f64 = 10.4;
const HOME_ZOOM: f64 = 6.5;

/// Wheel zoom per scroll line; pixel deltas are scaled as one line per this
/// many logical px (typical trackpad notch equivalent).
const ZOOM_PER_LINE: f64 = 0.25;
const PX_PER_LINE: f64 = 48.0;

/// Mouse-up within this many travelled px still counts as a click.
const CLICK_SLOP_PX: f64 = 4.0;

/// Zoom for badge-navigation fly-tos (design §3.1: "fly to the conflict"):
/// one step wider than the airport fly-to (11) — a clearance/airspace
/// problem needs the surrounding terrain in frame.
const CONFLICT_FLY_ZOOM: f64 = 10.0;

/// Fraction of the viewport a fitted route bbox may fill (the rest is the
/// margin around it).
const ROUTE_FIT_VIEWPORT_FRACTION: f64 = 0.75;
/// Zoom cap for route fits: a short hop (or a single-waypoint route, whose
/// bbox is a point) should land at airport-overview zoom, not rooftops.
const ROUTE_FIT_MAX_ZOOM: f64 = 11.0;

/// Debounce between camera settle and the store feature query. Short — the
/// renderer caches tessellation across sets, so a redundant feed is cheap
/// and latency is the only thing left to optimize.
const FEED_DEBOUNCE: Duration = Duration::from_millis(50);
/// Margin added around the visible bbox when querying features. Generous
/// for the same reason: bigger margins mean more settles land inside the
/// last coverage and skip the feed entirely.
const FEED_BBOX_MARGIN: f64 = 0.6;

/// Once the camera has been moving for longer than this, start streaming
/// feeds of the moving viewport instead of waiting for it to stop.
const FEED_STREAM_AFTER: Duration = Duration::from_millis(250);
/// Minimum spacing between two in-flight streaming feeds.
const FEED_STREAM_INTERVAL: Duration = Duration::from_millis(250);

/// Warm-feed scaling guard: only background-load the store's *entire*
/// airspace set at startup when it holds at most this many. Germany is
/// ~750 airspaces and a handful of enabled countries stays well under the
/// limit — trivially warmable. A store grown past the threshold (many
/// countries) falls back to plain viewport feeding (bounded memory; the
/// renderer-side mesh LRU bounds the GPU/cache side independently).
const WARM_FEED_MAX_AIRSPACES: usize = 20_000;

/// Reporting points / obstacles are zoom-gated in the renderer anyway;
/// don't pull them from the store while far out.
const REPORTING_POINT_FEED_MIN_ZOOM: f64 = 8.0;
const OBSTACLE_FEED_MIN_ZOOM: f64 = 9.0;

/// Hit-test tolerance in screen px, converted to degrees by zoom. Only point
/// features (airport symbols etc.) use it — airspaces are matched by exact
/// containment in the store — so it can be generous without smearing
/// airspace picks.
const PICK_TOLERANCE_PX: f64 = 14.0;

/// Renderer plus the per-frame bookkeeping the paint closure needs.
struct RendererCell {
    renderer: MapRenderer,
    /// The wgpu device the renderer was built on — compared against the
    /// window's current device to detect a GPU recovery that swapped devices
    /// before we ever observed the lost flag.
    device: Arc<strata_render::wgpu::Device>,
    last_frame: Option<Instant>,
    size: UVec2,
    scale: f32,
}

struct DragState {
    last: Point<Pixels>,
    travelled_px: f64,
}

pub struct MapView {
    app_state: Entity<AppState>,
    cell: Option<Arc<Mutex<RendererCell>>>,
    init_error: Option<String>,
    /// The window's GPU device was lost; painting is suspended until the
    /// platform recovers it, then the renderer is recreated.
    device_lost: bool,
    /// Whether the renderer was created with a basemap / terrain source
    /// (sources can arrive late, after `strata-ingest` runs).
    basemap_installed: bool,
    terrain_installed: bool,
    bounds: Bounds<Pixels>,
    drag: Option<DragState>,
    /// Country-wide always-visible points (airports + navaids) from the warm
    /// feed; empty when the warm feed is over threshold or hasn't landed.
    base_points: Vec<RenderPointFeature>,
    /// Static features from the last bbox feed (weather dots excluded).
    feature_points: Vec<RenderPointFeature>,
    weather_points: Vec<RenderPointFeature>,
    last_fed: Option<CameraSnapshot>,
    /// What the completed feeds actually queried; settles inside this
    /// envelope skip the re-feed entirely.
    feed_coverage: FeedCoverage,
    last_pushed_camera: Option<CameraSnapshot>,
    /// First frame of the current uninterrupted camera motion, if any.
    camera_moving_since: Option<Instant>,
    /// Last in-flight streaming feed during the current motion.
    last_stream_feed: Option<Instant>,
    feed_task: Option<Task<()>>,
    warm_feed_task: Option<Task<()>>,
    pick_task: Option<Task<()>>,
    /// Gridded-weather providers, frame cache and fetch tasks (see the
    /// `gridded` submodule for the scheduling).
    gridded_weather: GriddedWeatherController,
    /// The open flight's route as pushed by `RootView` (`None` in explorer
    /// mode); the renderer receives it with any drag ghost applied.
    route: Option<RenderRoute>,
    /// Route-editing interaction state (see the `route_edit` submodule).
    route_edit: route_edit::RouteEditState,
    _state_subscription: Subscription,
}

impl MapView {
    pub fn new(window: &mut Window, app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let _state_subscription = cx.subscribe(&app_state, Self::on_state_event);

        let (cell, init_error) = match create_renderer(window, &app_state, cx) {
            Ok((renderer, device)) => (
                Some(Arc::new(Mutex::new(RendererCell {
                    renderer,
                    device,
                    last_frame: None,
                    size: UVec2::ONE,
                    scale: 1.0,
                }))),
                None,
            ),
            Err(err) => {
                tracing::error!(%err, "map renderer unavailable");
                (None, Some(err))
            }
        };

        if let Some(cell) = &cell {
            let mut cell = cell.lock();
            // Default layer set: everything on except obstacles (the
            // renderer's own default already keeps the gridded weather
            // overlays off).
            cell.renderer.set_layer_enabled(LayerId::Obstacles, false);
            cell.renderer
                .set_weather_time(app_state.read(cx).weather_time.selected().timestamp());
            cell.renderer.input(MapInput::FlyTo {
                lat_lon: strata_render::LatLon::new(HOME_LAT, HOME_LON),
                zoom: HOME_ZOOM,
            });
        }

        let state = app_state.read(cx);
        let basemap_installed = state.basemap.is_some();
        let terrain_installed = state.terrain_store.is_some();

        let mut this = Self {
            app_state,
            cell,
            init_error,
            device_lost: false,
            basemap_installed,
            terrain_installed,
            bounds: Bounds::default(),
            drag: None,
            base_points: Vec::new(),
            feature_points: Vec::new(),
            weather_points: Vec::new(),
            last_fed: None,
            feed_coverage: FeedCoverage::default(),
            last_pushed_camera: None,
            camera_moving_since: None,
            last_stream_feed: None,
            feed_task: None,
            warm_feed_task: None,
            pick_task: None,
            gridded_weather: GriddedWeatherController::new(),
            route: None,
            route_edit: route_edit::RouteEditState::default(),
            _state_subscription,
        };
        this.start_warm_feed(cx);
        this
    }

    pub fn layer_enabled(&self, layer: LayerId) -> bool {
        self.cell
            .as_ref()
            .is_some_and(|cell| cell.lock().renderer.layer_enabled(layer))
    }

    pub fn set_layer_enabled(&mut self, layer: LayerId, enabled: bool, cx: &mut Context<Self>) {
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_layer_enabled(layer, enabled);
            cx.notify();
        }
        // Gridded weather layers drive the fetch scheduler: first one on
        // starts (or refreshes) the loop, last one off stops it.
        if matches!(
            layer,
            LayerId::CloudCover | LayerId::Precipitation | LayerId::Thunderstorms
        ) {
            self.sync_gridded_weather(cx);
        }
    }

    /// The flight-route seam: `RootView` pushes
    /// `AppState::flight_render_route()` here on flight events (`None` in
    /// explorer mode clears the layer). The view keeps the doc route for
    /// screen-space hit-testing and forwards it — with any active drag
    /// ghost applied — to the renderer; identical routes keep it idle.
    pub fn set_route(&mut self, route: Option<RenderRoute>, cx: &mut Context<Self>) {
        if route.is_none() {
            // Flight closed: no route-editing residue in the explorer.
            self.reset_route_edit();
        }
        self.route = route;
        self.push_route_to_renderer(cx);
    }

    /// Restyle the renderer at runtime (no-op if the theme is identical).
    pub fn set_map_theme(&mut self, theme: MapTheme, cx: &mut Context<Self>) {
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_map_theme(theme);
            cx.notify();
        }
    }

    /// Live basemap level-of-detail bias (config `basemap_detail_bias`); the
    /// settings modal calls this so slider changes restyle without a restart.
    /// Startup still flows the configured value in via `RendererConfig`.
    pub fn set_basemap_detail_bias(&mut self, bias: f64, cx: &mut Context<Self>) {
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_basemap_detail_bias(bias);
            cx.notify();
        }
    }

    pub fn fly_to(&mut self, lat: f64, lon: f64, zoom: f64, cx: &mut Context<Self>) {
        if let Some(cell) = &self.cell {
            cell.lock().renderer.input(MapInput::FlyTo {
                lat_lon: strata_render::LatLon::new(lat, lon),
                zoom,
            });
            cx.notify();
        }
        // Predictive feed: the landing viewport is fully determined by the
        // target pose and the (known) viewport size, so query it now instead
        // of waiting for animation + settle + debounce. The settle feed
        // remains the catch-all (and will find itself covered).
        let viewport = DVec2::new(
            f64::from(self.bounds.size.width),
            f64::from(self.bounds.size.height),
        );
        if viewport.cmpge(DVec2::ONE).all() {
            let target = fly_target_snapshot(lat, lon, zoom, viewport);
            self.schedule_feed_with(target, Duration::ZERO, cx);
        }
    }

    /// Flies the camera to frame the just-opened flight's route: the route
    /// bbox plus margin ([`ROUTE_FIT_VIEWPORT_FRACTION`]), zoom capped at
    /// [`ROUTE_FIT_MAX_ZOOM`]. No-op for empty routes and before the first
    /// layout (no viewport to fit into yet).
    fn fly_to_open_route(&mut self, cx: &mut Context<Self>) {
        let positions: Vec<GeoLatLon> = {
            let state = self.app_state.read(cx);
            let Some(flight) = state.flight.as_ref() else {
                return;
            };
            flight.doc.route.iter().map(|w| w.position()).collect()
        };
        let viewport = DVec2::new(
            f64::from(self.bounds.size.width),
            f64::from(self.bounds.size.height),
        );
        if viewport.cmplt(DVec2::ONE).any() {
            return;
        }
        if let Some((lat, lon, zoom)) = route_fit_view(&positions, viewport) {
            self.fly_to(lat, lon, zoom, cx);
        }
    }

    // --- state events -----------------------------------------------------

    fn on_state_event(
        &mut self,
        _state: Entity<AppState>,
        event: &AppStateEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::WeatherUpdated | AppStateEvent::StationsLoaded => {
                self.rebuild_weather(cx)
            }
            AppStateEvent::DataReloaded => self.on_data_reloaded(cx),
            AppStateEvent::WeatherTimeChanged => self.on_weather_time_changed(cx),
            AppStateEvent::WeatherTimeReanchored => self.on_weather_time_reanchored(cx),
            // Opening a flight frames its route (bbox + margin, zoom-
            // capped); an empty route keeps the camera where it is (a
            // brand-new flight starts where the user is looking). The
            // route geometry itself arrives via `RootView` →
            // [`Self::set_route`].
            AppStateEvent::FlightOpened => self.fly_to_open_route(cx),
            AppStateEvent::FlightChanged
            | AppStateEvent::FlightComputed
            | AppStateEvent::FlightClosed => {}
            // The scrub marker and the hover highlight ride the retained
            // route geometry: re-push with the new `scrub_along_m` /
            // `highlight` (either change alone keeps the renderer's
            // tessellation idle). The corridor outline toggle re-pushes the
            // same way (with/without `corridor_halfwidth_m`).
            AppStateEvent::ProfileScrubChanged
            | AppStateEvent::RouteHighlightChanged
            | AppStateEvent::CorridorChanged => self.push_route_to_renderer(cx),
            // A badge navigated to a conflict (design §3.1): fly the map to
            // its cause; the scrub marker is already on it.
            AppStateEvent::PlanningFocusRequested(focus) => {
                if let crate::state::flight::PlanningFocus::Profile {
                    target: Some(target),
                } = focus
                {
                    self.fly_to(
                        target.position.lat(),
                        target.position.lon(),
                        CONFLICT_FLY_ZOOM,
                        cx,
                    );
                }
            }
            AppStateEvent::SelectionChanged
            | AppStateEvent::IngestProgressChanged
            | AppStateEvent::IngestNotice(_)
            | AppStateEvent::AircraftLibraryChanged
            | AppStateEvent::BriefingChanged
            | AppStateEvent::ProfileWeatherChanged
            | AppStateEvent::ExportFinished(_) => {}
        }
    }

    /// `strata-ingest` finished while the app was open: install tile sources
    /// that did not exist at startup and re-feed the visible features
    /// without waiting for a camera move.
    fn on_data_reloaded(&mut self, cx: &mut Context<Self>) {
        if let Some(cell) = self.cell.clone() {
            let state = self.app_state.read(cx);
            let new_basemap = (!self.basemap_installed)
                .then(|| state.basemap.clone())
                .flatten();
            let new_terrain = (!self.terrain_installed)
                .then(|| state.terrain_store.clone())
                .flatten();
            let mut cell = cell.lock();
            if let Some(source) = new_basemap {
                self.basemap_installed = true;
                cell.renderer
                    .set_basemap_source(Some(source as Arc<dyn TileSource>));
            }
            if let Some(store) = new_terrain {
                self.terrain_installed = true;
                cell.renderer.set_terrain_source(Some(
                    Arc::new(StoreTerrainSource::new(store)) as Arc<dyn TileSource>
                ));
            }
        }
        // The data on disk changed: previous coverage (including the warm
        // feed's global flag) describes stale data. Clear it, force a fresh
        // viewport feed at the current camera, and re-warm in the background.
        self.last_fed = None;
        self.feed_coverage.clear();
        self.base_points.clear();
        if let Some(snapshot) = self.last_pushed_camera {
            self.schedule_feed(snapshot, cx);
        }
        self.start_warm_feed(cx);
        cx.notify();
    }

    fn rebuild_weather(&mut self, cx: &mut Context<Self>) {
        let state = self.app_state.read(cx);
        let mut points = Vec::new();
        for (station, metar) in &state.weather.metars {
            let Some(position) = state.station_position(station) else {
                continue;
            };
            let Some(category) = metar.decoded.as_ref().and_then(|d| d.flight_category()) else {
                continue;
            };
            points.push(convert::weather_station(
                station.as_str(),
                position,
                category,
            ));
        }
        let sigmets: Vec<_> = state.weather.sigmets.iter().map(convert::sigmet).collect();

        self.weather_points = points;
        if let Some(cell) = &self.cell {
            let mut cell = cell.lock();
            cell.renderer.set_points(self.combined_points());
            cell.renderer.set_sigmets(sigmets);
        }
        cx.notify();
    }

    fn combined_points(&self) -> Vec<RenderPointFeature> {
        self.base_points
            .iter()
            .chain(self.feature_points.iter())
            .chain(self.weather_points.iter())
            .cloned()
            .collect()
    }

    // --- camera / feature feed --------------------------------------------

    /// Called from the paint closure on every frame with the fresh snapshot.
    fn on_frame(&mut self, snapshot: CameraSnapshot, idle: bool, cx: &mut Context<Self>) {
        let camera_changed = self.last_pushed_camera != Some(snapshot);
        if camera_changed {
            self.last_pushed_camera = Some(snapshot);
            self.app_state
                .update(cx, |state, cx| state.set_camera(snapshot, cx));
        }
        if idle {
            self.camera_moving_since = None;
            self.last_stream_feed = None;
            if self.last_fed != Some(snapshot) {
                self.schedule_feed(snapshot, cx);
            }
        } else if camera_changed {
            self.stream_feed_while_moving(snapshot, cx);
        } else if self.camera_moving_since.take().is_some() {
            // The camera stopped but the renderer still animates (tile/label
            // fades): this is the user-visible settle — feed now instead of
            // waiting for full idleness.
            self.last_stream_feed = None;
            if self.last_fed != Some(snapshot) {
                self.schedule_feed(snapshot, cx);
            }
        }
    }

    /// During long pans/zooms, feed the current (still moving) viewport
    /// periodically so data streams in while moving instead of only after
    /// stopping. The coverage check in [`Self::schedule_feed_with`] keeps
    /// already-covered intermediate viewports free.
    fn stream_feed_while_moving(&mut self, snapshot: CameraSnapshot, cx: &mut Context<Self>) {
        let now = Instant::now();
        let moving_since = *self.camera_moving_since.get_or_insert(now);
        if now.duration_since(moving_since) < FEED_STREAM_AFTER {
            return;
        }
        if self
            .last_stream_feed
            .is_some_and(|last| now.duration_since(last) < FEED_STREAM_INTERVAL)
        {
            return;
        }
        self.last_stream_feed = Some(now);
        self.schedule_feed_with(snapshot, Duration::ZERO, cx);
    }

    fn schedule_feed(&mut self, snapshot: CameraSnapshot, cx: &mut Context<Self>) {
        self.schedule_feed_with(snapshot, FEED_DEBOUNCE, cx);
    }

    fn schedule_feed_with(
        &mut self,
        snapshot: CameraSnapshot,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        // The completed feeds already cover a margin-expanded superset of
        // this view with the same zoom gates — refeeding would re-query and
        // re-push byte-identical data.
        if feed_covered(&self.feed_coverage, &snapshot) {
            self.last_fed = Some(snapshot);
            self.feed_task = None; // also drops a stale pending debounce
            return;
        }
        let Some(store) = self.app_state.read(cx).store.clone() else {
            return;
        };
        let Some(cell) = self.cell.clone() else {
            return;
        };
        // Global coverage (warm feed) means airspaces + airports + navaids
        // are already complete; only the zoom-gated point kinds re-query.
        let scope = if self.feed_coverage.global {
            FeedScope::GatedPointsOnly
        } else {
            FeedScope::Full
        };
        let scheduled = Instant::now();
        // Replacing the task drops (cancels) the previous debounce/feed.
        self.feed_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Some(bbox) = feed_bbox(&snapshot) else {
                return;
            };
            let zoom = snapshot.zoom;
            let feed = cx
                .background_spawn(async move { query_features(&store, bbox, zoom, scope) })
                .await;
            this.update(cx, |this, cx| {
                if scope == FeedScope::Full && this.feed_coverage.global {
                    // The warm feed landed while this full feed was in
                    // flight; applying the (airport/navaid-duplicating)
                    // subset now would only fight it.
                    return;
                }
                this.last_fed = Some(snapshot);
                this.feed_coverage.bbox = Some(BboxCoverage {
                    bbox,
                    gates: FeedGates::at_zoom(zoom),
                });
                this.feature_points = feed.points;
                {
                    let mut cell = cell.lock();
                    if let Some(airspaces) = feed.airspaces {
                        cell.renderer.set_airspaces(airspaces);
                    }
                    cell.renderer.set_points(this.combined_points());
                }
                tracing::debug!(
                    end_to_end_ms = scheduled.elapsed().as_millis() as u64,
                    delay_ms = delay.as_millis() as u64,
                    "feed applied"
                );
                cx.notify();
            })
            .ok();
        }));
    }

    /// Startup/reload warm feed: when the store's airspace count is within
    /// [`WARM_FEED_MAX_AIRSPACES`], load *all* of its airspaces plus the
    /// always-visible point kinds (airports, navaids) in the background,
    /// push them once, and mark coverage global — afterwards camera moves
    /// only ever re-query reporting points/obstacles. Over the threshold
    /// nothing changes: plain bounded viewport feeding. Deliberately not
    /// country-scoped: country selection controls what gets *ingested*,
    /// the map renders whatever the store holds.
    fn start_warm_feed(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.app_state.read(cx).store.clone() else {
            return;
        };
        let Some(cell) = self.cell.clone() else {
            return;
        };
        let started = Instant::now();
        self.warm_feed_task = Some(cx.spawn(async move |this, cx| {
            let warm = cx
                .background_spawn(async move { query_warm_feed(&store) })
                .await;
            let Some(warm) = warm else {
                return; // over threshold (or store error): viewport feeding
            };
            this.update(cx, |this, cx| {
                tracing::info!(
                    airspaces = warm.airspaces.len(),
                    points = warm.points.len(),
                    total_ms = started.elapsed().as_millis() as u64,
                    "warm feed complete — airspace coverage is now global"
                );
                this.feed_coverage.global = true;
                this.base_points = warm.points;
                // Drop bbox state from any earlier full feed: its airports/
                // navaids would duplicate `base_points`, and its coverage
                // gates may promise gated kinds we are about to re-own.
                this.feed_coverage.bbox = None;
                this.feature_points.clear();
                this.last_fed = None;
                {
                    let mut cell = cell.lock();
                    cell.renderer.set_airspaces(warm.airspaces);
                    cell.renderer.set_points(this.combined_points());
                }
                // Catch up the zoom-gated kinds for the current viewport;
                // this also replaces (cancels) any in-flight full feed that
                // could otherwise overwrite the warm data with a subset.
                if let Some(snapshot) = this.last_pushed_camera {
                    this.schedule_feed(snapshot, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // --- input -------------------------------------------------------------

    fn local_px(&self, position: Point<Pixels>) -> DVec2 {
        DVec2::new(
            f64::from(position.x - self.bounds.origin.x),
            f64::from(position.y - self.bounds.origin.y),
        )
    }

    fn input(&self, event: MapInput) {
        if let Some(cell) = &self.cell {
            cell.lock().renderer.input(event);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A press on a route handle or leg (planning mode) captures the
        // gesture — handle drags take precedence over the map pan only
        // there; everywhere else the pan starts exactly as today.
        if self.begin_route_gesture(self.local_px(event.position), cx) {
            return;
        }
        self.drag = Some(DragState {
            last: event.position,
            travelled_px: 0.0,
        });
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.local_px(event.position);
        self.input(MapInput::CursorMoved { px: local });

        if event.pressed_button == Some(MouseButton::Left) {
            if self.route_gesture_active() {
                self.update_route_gesture(local, cx);
            } else if let Some(drag) = &mut self.drag {
                let delta = DVec2::new(
                    f64::from(event.position.x - drag.last.x),
                    f64::from(event.position.y - drag.last.y),
                );
                drag.last = event.position;
                drag.travelled_px += delta.length();
                self.input(MapInput::PanBy { delta_px: delta });
                cx.notify();
            }
        } else {
            self.update_route_hover(local, cx);
        }

        // Status bar lat/lon (set_cursor quantizes, so this only notifies
        // when the displayed value actually changes).
        if let Some(cell) = &self.cell {
            let geo = cell.lock().renderer.pick(local);
            self.app_state.update(cx, |state, cx| {
                state.set_cursor(geo.lat_deg(), geo.lon_deg(), cx)
            });
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.route_gesture_active() {
            // A captured route gesture never panned; a within-slop release
            // falls through to the plain selection click, exactly as today.
            if self.finish_route_gesture(cx) {
                self.handle_click(self.local_px(event.position), cx);
            }
            return;
        }
        let was_click = self
            .drag
            .take()
            .is_some_and(|d| d.travelled_px < CLICK_SLOP_PX);
        self.input(MapInput::PanEnd);
        if was_click {
            self.handle_click(self.local_px(event.position), cx);
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => f64::from(lines.y),
            ScrollDelta::Pixels(pixels) => f64::from(pixels.y) / PX_PER_LINE,
        };
        if lines == 0.0 {
            return;
        }
        // Scroll up (positive y) zooms in.
        self.input(MapInput::ZoomAbout {
            anchor_px: self.local_px(event.position),
            zoom_delta: lines * ZOOM_PER_LINE,
        });
        cx.notify();
    }

    fn handle_click(&mut self, local: DVec2, cx: &mut Context<Self>) {
        let Some(cell) = &self.cell else {
            return;
        };
        let Some(store) = self.app_state.read(cx).store.clone() else {
            return;
        };
        let (geo, zoom) = {
            let cell = cell.lock();
            (cell.renderer.pick(local), cell.renderer.camera().zoom)
        };
        // Screen-px tolerance in longitude degrees at this zoom.
        let tolerance_deg = PICK_TOLERANCE_PX * 360.0 / (256.0 * 2f64.powf(zoom));
        let app_state = self.app_state.clone();
        self.pick_task = Some(cx.spawn(async move |_, cx| {
            let features = cx
                .background_spawn(async move {
                    let Ok(point) = GeoLatLon::new(geo.lat_deg(), geo.lon_deg()) else {
                        return Vec::new();
                    };
                    store
                        .feature_at(point, tolerance_deg)
                        .map(prioritize_point_features)
                        .unwrap_or_else(|err| {
                            tracing::warn!(%err, "feature_at failed");
                            Vec::new()
                        })
                })
                .await;
            app_state.update(cx, |state, cx| state.set_selection(features, cx));
        }));
    }

    // --- painting ----------------------------------------------------------

    fn paint_frame(
        cell: &Arc<Mutex<RendererCell>>,
        view: &WeakEntity<MapView>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // GPU device loss: never submit to a dead device, and never hand
        // gpui a texture from one — that would poison every recovered
        // frame. The view recreates the renderer once the platform reports
        // recovery (`Some(false)` again).
        let lost = window.gpu_device_lost();
        let device_swapped = lost == Some(false)
            && window
                .gpu_context()
                .and_then(|gpu| {
                    gpu.downcast::<(
                        Arc<strata_render::wgpu::Device>,
                        Arc<strata_render::wgpu::Queue>,
                    )>()
                    .ok()
                })
                .is_some_and(|current| !Arc::ptr_eq(&current.0, &cell.lock().device));
        if device_needs_recreation(lost, device_swapped) {
            view.update(cx, |this, cx| {
                if !this.device_lost {
                    tracing::warn!("GPU device lost; map painting suspended");
                    this.device_lost = true;
                    cx.notify();
                }
            })
            .ok();
            return;
        }

        let scale = window.scale_factor();
        let width = (f64::from(bounds.size.width) * f64::from(scale))
            .round()
            .max(1.0) as u32;
        let height = (f64::from(bounds.size.height) * f64::from(scale))
            .round()
            .max(1.0) as u32;
        let size_px = UVec2::new(width, height);

        let (redraw, snapshot, texture) = {
            let mut cell = cell.lock();
            if cell.size != size_px || cell.scale != scale {
                cell.renderer.resize(size_px, scale);
                cell.size = size_px;
                cell.scale = scale;
            }
            let now = Instant::now();
            // Clamp dt so a long pause (window hidden, debugger) doesn't
            // teleport animations.
            let dt = cell
                .last_frame
                .map(|t| now.duration_since(t))
                .unwrap_or(Duration::from_millis(16))
                .min(Duration::from_millis(100));
            cell.last_frame = Some(now);
            let redraw = cell.renderer.tick(dt);
            // gpui repaints this canvas on every window draw (status-bar
            // cursor, weather refresh, input caret, ...). When the renderer
            // is idle a new frame would be pixel-identical, so re-present
            // the front buffer instead of rendering. `tick` covers
            // everything: camera animation, pending layer work/fades, and
            // every mutating setter (resize included) marks dirty.
            let texture = match redraw {
                Redraw::Needed => cell.renderer.render().clone(),
                Redraw::Idle => cell.renderer.texture().clone(),
            };
            (redraw, cell.renderer.camera(), texture)
        };

        window.paint_surface(
            bounds,
            Arc::new(texture),
            size(DevicePixels(width as i32), DevicePixels(height as i32)),
        );

        if redraw == Redraw::Needed {
            window.request_animation_frame();
        }
        view.update(cx, |this, cx| {
            this.on_frame(snapshot, redraw == Redraw::Idle, cx)
        })
        .ok();
    }

    /// Drop the dead renderer and build a fresh one on the recovered device,
    /// restoring camera pose, layer toggles and weather overlays. Airspaces
    /// and point features come back through the forced re-feed.
    fn recreate_renderer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.cell.take();
        let toggles: Vec<(LayerId, bool)> = old
            .as_ref()
            .map(|cell| {
                let cell = cell.lock();
                LayerId::ALL
                    .iter()
                    .map(|&layer| (layer, cell.renderer.layer_enabled(layer)))
                    .collect()
            })
            .unwrap_or_default();
        drop(old);

        match create_renderer(window, &self.app_state, cx) {
            Ok((renderer, device)) => {
                let cell = Arc::new(Mutex::new(RendererCell {
                    renderer,
                    device,
                    last_frame: None,
                    size: UVec2::ONE,
                    scale: 1.0,
                }));
                {
                    let mut cell = cell.lock();
                    for (layer, enabled) in toggles {
                        cell.renderer.set_layer_enabled(layer, enabled);
                    }
                    cell.renderer.set_weather_time(
                        self.app_state.read(cx).weather_time.selected().timestamp(),
                    );
                    let (lat_lon, zoom) = self
                        .last_pushed_camera
                        .map(|snapshot| (snapshot.center, snapshot.zoom))
                        .unwrap_or((strata_render::LatLon::new(HOME_LAT, HOME_LON), HOME_ZOOM));
                    cell.renderer.input(MapInput::FlyTo { lat_lon, zoom });
                }
                self.cell = Some(cell);
                let state = self.app_state.read(cx);
                self.basemap_installed = state.basemap.is_some();
                self.terrain_installed = state.terrain_store.is_some();
                self.device_lost = false;
                self.init_error = None;
                // Feed + weather repopulate the fresh layers. The new
                // renderer holds nothing, so global coverage is void until
                // the warm feed lands again.
                self.last_fed = None;
                self.feed_coverage.clear();
                self.base_points.clear();
                self.last_pushed_camera = None;
                self.rebuild_weather(cx);
                self.push_route_to_renderer(cx);
                self.start_warm_feed(cx);
                // The fresh renderer holds no weather frames: forget the
                // pushed bookkeeping so the restarted cycle re-pushes the
                // (cached, mostly network-free) working sets.
                self.gridded_weather.renderer_reset();
                self.sync_gridded_weather(cx);
                tracing::info!("map renderer recreated after GPU device loss");
            }
            Err(err) => {
                tracing::warn!(%err, "map renderer recreation failed; will retry");
                self.init_error = Some("GPU device lost — recovering…".to_string());
            }
        }
    }
}

impl Render for MapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div().size_full().relative().bg(cx.theme().background);

        if self.device_lost {
            if window.gpu_device_lost() == Some(false) {
                // The platform recovered the device — rebuild on it.
                self.recreate_renderer(window, cx);
            }
            if self.device_lost {
                // Still lost (or recreation failed): placeholder + retry.
                window.request_animation_frame();
                return base
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child("GPU device lost — recovering…")
                    .into_any_element();
            }
        }

        let Some(cell) = self.cell.clone() else {
            let message = self
                .init_error
                .clone()
                .unwrap_or_else(|| "map renderer unavailable".to_string());
            return base
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Map unavailable: {message}"))
                .into_any_element();
        };

        let prepaint_view = cx.entity().downgrade();
        let paint_view = prepaint_view.clone();
        let map_canvas = canvas(
            move |bounds, _window, cx| {
                prepaint_view
                    .update(cx, |this, _| this.bounds = bounds)
                    .ok();
            },
            move |bounds, (), window, cx| {
                Self::paint_frame(&cell, &paint_view, bounds, window, cx);
            },
        )
        .size_full();

        let dragging_route = self.route_drag_engaged();
        let hovering_handle = self.route_handle_hovered();
        let surface = base
            .id("map-surface")
            .child(map_canvas)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .when(dragging_route, |el| el.cursor_grabbing())
            .when(!dragging_route && hovering_handle, |el| el.cursor_grab());

        // Planning mode adds the right-click route menu; the explorer map
        // stays exactly the element it always was (right-click does
        // nothing there).
        if self.app_state.read(cx).planning_mode() {
            let map = cx.entity();
            surface
                .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
                .context_menu(move |menu, _window, cx| {
                    route_edit::build_route_context_menu(menu, &map, cx)
                })
                .into_any_element()
        } else {
            surface.into_any_element()
        }
    }
}

// --- renderer construction & feed helpers (free, testable) -----------------

/// Builds the renderer from the window's shared wgpu device/queue; also
/// returns the device for later identity checks (GPU recovery detection).
fn create_renderer(
    window: &mut Window,
    app_state: &Entity<AppState>,
    cx: &mut Context<MapView>,
) -> Result<(MapRenderer, Arc<strata_render::wgpu::Device>), String> {
    let gpu = window
        .gpu_context()
        .ok_or_else(|| "window has no GPU context (not running the wgpu backend?)".to_string())?;
    let (device, queue) = *gpu
        .downcast::<(
            Arc<strata_render::wgpu::Device>,
            Arc<strata_render::wgpu::Queue>,
        )>()
        .map_err(|_| {
            "GPU context is not (Arc<wgpu::Device>, Arc<wgpu::Queue>) — \
             gpui-ce / strata-render wgpu version mismatch"
                .to_string()
        })?;

    let state = app_state.read(cx);
    let basemap_source = state.basemap.clone().map(|s| s as Arc<dyn TileSource>);
    let terrain_source = state
        .terrain_store
        .clone()
        .map(|store| Arc::new(StoreTerrainSource::new(store)) as Arc<dyn TileSource>);

    // `AppState::map_theme_id` is the single source of truth for the map
    // theme, so a renderer recreated after GPU device loss keeps its styling.
    let config = RendererConfig {
        basemap_source,
        terrain_source,
        theme: MapTheme::by_id(state.map_theme_id).unwrap_or_default(),
        // From the user config (clamped on load); a future settings-modal
        // slider can adjust it live via `set_basemap_detail_bias`.
        basemap_detail_bias: state.config.basemap_detail_bias,
        ..RendererConfig::default()
    };
    MapRenderer::new(Arc::clone(&device), queue, config)
        .map(|renderer| (renderer, device))
        .map_err(|err| err.to_string())
}

/// Whether the map renderer must stop submitting and be recreated.
///
/// `lost` is the platform's device-lost flag (`None`: a backend that cannot
/// know); `device_swapped` is whether the window's current device differs
/// from the one the renderer was built on (recovery already happened before
/// we ever observed the lost flag).
fn device_needs_recreation(lost: Option<bool>, device_swapped: bool) -> bool {
    match lost {
        Some(true) => true,
        Some(false) => device_swapped,
        None => false,
    }
}

/// Selection ordering for the info panel: point features (airports, navaids,
/// reporting points, obstacles) before airspaces. Every click on the map hits
/// the full airspace stack, so a deliberately clicked symbol must not drown
/// under it. Relative order within each group is preserved (the store already
/// sorts point hits by distance).
fn prioritize_point_features(features: Vec<Feature>) -> Vec<Feature> {
    let (points, airspaces): (Vec<_>, Vec<_>) = features
        .into_iter()
        .partition(|feature| !matches!(feature, Feature::Airspace(_)));
    points.into_iter().chain(airspaces).collect()
}

/// What a bbox feed queries: everything, or — once the warm feed covers
/// airspaces/airports/navaids store-wide — only the zoom-gated point kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FeedScope {
    Full,
    GatedPointsOnly,
}

struct FeatureFeed {
    /// `None` when the feed didn't query airspaces ([`FeedScope`]) — the
    /// renderer keeps its current (warm-fed) set.
    airspaces: Option<Vec<RenderAirspace>>,
    points: Vec<RenderPointFeature>,
}

/// Which zoom-gated feature kinds a feed at a given zoom selects.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FeedGates {
    reporting_points: bool,
    obstacles: bool,
}

impl FeedGates {
    /// No gated kind selected (far-out zooms).
    const NONE: Self = Self {
        reporting_points: false,
        obstacles: false,
    };

    fn at_zoom(zoom: f64) -> Self {
        Self {
            reporting_points: zoom >= REPORTING_POINT_FEED_MIN_ZOOM,
            obstacles: zoom >= OBSTACLE_FEED_MIN_ZOOM,
        }
    }
}

/// Margin-expanded bbox and zoom gates of the last completed bbox feed.
#[derive(Clone, Copy)]
struct BboxCoverage {
    bbox: BoundingBox,
    gates: FeedGates,
}

/// What the completed feeds cover, i.e. when a re-feed can be skipped.
#[derive(Default)]
struct FeedCoverage {
    /// The warm feed landed: airspaces, airports and navaids are loaded for
    /// the entire store. Cleared on data reload / renderer recreation.
    global: bool,
    /// Envelope of the last completed bbox feed, if any.
    bbox: Option<BboxCoverage>,
}

impl FeedCoverage {
    /// Data reload / renderer recreation: nothing is covered anymore.
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// True when everything a feed at `snapshot` would query is already loaded:
/// global (warm) coverage handles airspaces/airports/navaids store-wide,
/// and the zoom-gated point kinds are covered when either no gate is open
/// or the visible bbox is still inside the margin-expanded bbox the last
/// feed queried with the same gates — refeeding would be pure waste.
fn feed_covered(coverage: &FeedCoverage, snapshot: &CameraSnapshot) -> bool {
    let gates = FeedGates::at_zoom(snapshot.zoom);
    if coverage.global && gates == FeedGates::NONE {
        return true;
    }
    let Some(fed) = coverage.bbox else {
        return false;
    };
    if fed.gates != gates {
        return false;
    }
    let (sw, ne) = snapshot.bounds;
    let corner = |p: strata_render::LatLon| GeoLatLon::new(p.lat_deg(), p.lon_deg());
    corner(sw).is_ok_and(|p| fed.bbox.contains(p)) && corner(ne).is_ok_and(|p| fed.bbox.contains(p))
}

/// Warm-feed scaling guard: stores up to this many airspaces are loaded
/// whole; anything bigger keeps bounded viewport feeding.
fn warm_feed_allowed(airspace_count: usize) -> bool {
    airspace_count <= WARM_FEED_MAX_AIRSPACES
}

/// The camera snapshot a fly-to will land on: pure Web-Mercator math from
/// the target pose and the viewport size in logical px, mirroring
/// `strata_render::Camera` (`256 · 2^zoom` logical px per world unit, world
/// clamped to `[0, 1]²`, y grows south).
fn fly_target_snapshot(lat: f64, lon: f64, zoom: f64, viewport_px: DVec2) -> CameraSnapshot {
    use strata_render::camera::TILE_SIZE_PX;
    use strata_render::geo;

    let zoom = zoom.clamp(strata_render::MIN_ZOOM, strata_render::MAX_ZOOM);
    let center = geo::world_from_lat_lon(strata_render::LatLon::new(lat, lon))
        .clamp(DVec2::ZERO, DVec2::ONE);
    let half = viewport_px / (2.0 * TILE_SIZE_PX * zoom.exp2());
    let min = (center - half).clamp(DVec2::ZERO, DVec2::ONE);
    let max = (center + half).clamp(DVec2::ZERO, DVec2::ONE);
    CameraSnapshot {
        center: geo::lat_lon_from_world(center),
        zoom,
        bounds: (
            geo::lat_lon_from_world(DVec2::new(min.x, max.y)),
            geo::lat_lon_from_world(DVec2::new(max.x, min.y)),
        ),
    }
}

/// Center + zoom framing `positions` in a viewport of `viewport_px`
/// logical px: the Web-Mercator bbox center, zoomed so the bbox fills at
/// most [`ROUTE_FIT_VIEWPORT_FRACTION`] of each viewport axis, capped at
/// [`ROUTE_FIT_MAX_ZOOM`] (degenerate bboxes — a single waypoint — land at
/// the cap) and clamped to the camera's zoom range. `None` for an empty
/// route.
fn route_fit_view(positions: &[GeoLatLon], viewport_px: DVec2) -> Option<(f64, f64, f64)> {
    use strata_render::camera::TILE_SIZE_PX;
    use strata_render::geo;

    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for position in positions {
        let world =
            geo::world_from_lat_lon(strata_render::LatLon::new(position.lat(), position.lon()));
        min = min.min(world);
        max = max.max(world);
    }
    if !min.x.is_finite() {
        return None; // empty route
    }
    let center = geo::lat_lon_from_world((min + max) / 2.0);
    // Visible world units at zoom z: viewport / (256 · 2^z) — solve for the
    // zoom putting the bbox extent inside the usable fraction of each axis.
    let extent = max - min;
    let usable = viewport_px * ROUTE_FIT_VIEWPORT_FRACTION;
    let fit = |usable_px: f64, extent_world: f64| {
        if extent_world <= f64::EPSILON {
            f64::INFINITY // a point fits at any zoom — the cap decides
        } else {
            (usable_px / (TILE_SIZE_PX * extent_world)).log2()
        }
    };
    let zoom = fit(usable.x, extent.x)
        .min(fit(usable.y, extent.y))
        .min(ROUTE_FIT_MAX_ZOOM)
        .clamp(strata_render::MIN_ZOOM, strata_render::MAX_ZOOM);
    Some((center.lat_deg(), center.lon_deg(), zoom))
}

/// Visible bbox expanded by [`FEED_BBOX_MARGIN`], clamped to valid degrees.
fn feed_bbox(snapshot: &CameraSnapshot) -> Option<BoundingBox> {
    let (sw, ne) = snapshot.bounds;
    let (south, west) = (sw.lat_deg(), sw.lon_deg());
    let (north, east) = (ne.lat_deg(), ne.lon_deg());
    let margin_lat = (north - south).abs() * FEED_BBOX_MARGIN;
    let margin_lon = (east - west).abs() * FEED_BBOX_MARGIN;
    BoundingBox::new(
        (west - margin_lon).max(-180.0),
        (south - margin_lat).max(-90.0),
        (east + margin_lon).min(180.0),
        (north + margin_lat).min(90.0),
    )
    .map_err(|err| tracing::warn!(%err, "camera bbox invalid"))
    .ok()
}

fn fetch<T>(what: &'static str, result: Result<Vec<T>, strata_data::store::StoreError>) -> Vec<T> {
    result.unwrap_or_else(|err| {
        tracing::warn!(what, %err, "store query failed");
        Vec::new()
    })
}

/// Big airspaces first so small ones (and their labels) sit on top.
fn convert_airspaces(mut airspaces: Vec<strata_data::domain::Airspace>) -> Vec<RenderAirspace> {
    airspaces.sort_by(|a, b| {
        let area = |x: &strata_data::domain::Airspace| {
            let b = x.geometry.bounding_box();
            (b.east() - b.west()) * (b.north() - b.south())
        };
        area(b)
            .partial_cmp(&area(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    airspaces.iter().map(convert::airspace).collect()
}

/// Blocking store queries + domain→render conversion; worker threads only.
fn query_features(store: &Store, bbox: BoundingBox, zoom: f64, scope: FeedScope) -> FeatureFeed {
    let started = Instant::now();

    let airspaces = (scope == FeedScope::Full)
        .then(|| convert_airspaces(fetch("airspaces", store.airspaces_in_bbox(bbox))));

    let mut points: Vec<RenderPointFeature> = Vec::new();
    if scope == FeedScope::Full {
        points.extend(
            fetch("airports", store.airports_in_bbox(bbox))
                .iter()
                .filter_map(convert::airport),
        );
        points.extend(
            fetch("navaids", store.navaids_in_bbox(bbox))
                .iter()
                .map(convert::navaid),
        );
    }
    if zoom >= REPORTING_POINT_FEED_MIN_ZOOM {
        points.extend(
            fetch("reporting points", store.reporting_points_in_bbox(bbox))
                .iter()
                .map(convert::reporting_point),
        );
    }
    if zoom >= OBSTACLE_FEED_MIN_ZOOM {
        points.extend(
            fetch("obstacles", store.obstacles_in_bbox(bbox))
                .iter()
                .map(convert::obstacle),
        );
    }

    tracing::debug!(
        airspaces = airspaces.as_ref().map(Vec::len),
        points = points.len(),
        zoom,
        gated_only = scope == FeedScope::GatedPointsOnly,
        query_ms = started.elapsed().as_millis() as u64,
        "feature feed"
    );
    FeatureFeed { airspaces, points }
}

/// Result of [`query_warm_feed`]: every airspace plus all always-visible
/// point kinds (airports, navaids) the store holds.
struct WarmFeed {
    airspaces: Vec<RenderAirspace>,
    points: Vec<RenderPointFeature>,
}

/// Blocking warm-feed query; worker threads only. Covers the *whole store*
/// — never a country subset (rendering is country-agnostic; multi-country
/// stores warm-feed all of their countries at once). `None` when the
/// store's airspace count is over [`WARM_FEED_MAX_AIRSPACES`] (cheap
/// R*Tree count, nothing decoded) — the caller then keeps plain viewport
/// feeding.
fn query_warm_feed(store: &Store) -> Option<WarmFeed> {
    let bbox = world_bbox();
    let started = Instant::now();
    let count = store
        .airspace_count_in_bbox(bbox)
        .map_err(|err| tracing::warn!(%err, "warm-feed airspace count failed"))
        .ok()?;
    if !warm_feed_allowed(count) {
        tracing::info!(
            airspaces = count,
            max = WARM_FEED_MAX_AIRSPACES,
            "warm feed skipped — store over the airspace threshold, keeping bounded viewport feeds"
        );
        return None;
    }
    tracing::info!(
        airspaces = count,
        max = WARM_FEED_MAX_AIRSPACES,
        "warm feed loading — store within the airspace threshold, going for global coverage"
    );

    let airspaces = convert_airspaces(fetch("airspaces", store.airspaces_in_bbox(bbox)));
    let mut points: Vec<RenderPointFeature> = Vec::new();
    points.extend(
        fetch("airports", store.airports_in_bbox(bbox))
            .iter()
            .filter_map(convert::airport),
    );
    points.extend(
        fetch("navaids", store.navaids_in_bbox(bbox))
            .iter()
            .map(convert::navaid),
    );

    tracing::debug!(
        airspaces = airspaces.len(),
        points = points.len(),
        query_ms = started.elapsed().as_millis() as u64,
        "warm feed queried"
    );
    Some(WarmFeed { airspaces, points })
}

#[cfg(test)]
mod tests {
    use strata_data::domain::{
        Airport, AirportKind, Airspace, AirspaceClass, AirspaceKind, LatLon, MetersAmsl, Navaid,
        NavaidKind, Polygon, VerticalLimit,
    };

    use super::*;

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    fn airspace(name: &str) -> Feature {
        Feature::Airspace(Airspace {
            name: name.to_owned(),
            class: AirspaceClass::D,
            kind: AirspaceKind::Ctr,
            lower: VerticalLimit::gnd(),
            upper: VerticalLimit::amsl(MetersAmsl::from_feet(3500.0)),
            geometry: Polygon::new(
                vec![
                    ll(49.0, 11.0),
                    ll(49.0, 11.2),
                    ll(49.2, 11.2),
                    ll(49.2, 11.0),
                ],
                vec![],
            )
            .unwrap(),
            airac: None,
        })
    }

    fn airport(name: &str) -> Feature {
        Feature::Airport(Airport {
            ident: None,
            name: name.to_owned(),
            kind: AirportKind::Airfield,
            position: ll(49.5, 11.1),
            elevation: MetersAmsl::from_feet(1046.0),
            runways: vec![],
            frequencies: vec![],
        })
    }

    fn navaid(name: &str) -> Feature {
        Feature::Navaid(Navaid {
            ident: "NUB".to_owned(),
            name: name.to_owned(),
            kind: NavaidKind::VorDme,
            frequency: None,
            channel: None,
            position: ll(49.5, 11.1),
            elevation: MetersAmsl(300.0),
        })
    }

    fn names(features: &[Feature]) -> Vec<&str> {
        features.iter().map(|f| f.name()).collect()
    }

    /// Store order is airspace stack first; the panel must lead with the
    /// deliberately clicked point features instead, preserving each group's
    /// internal (distance) order.
    #[test]
    fn point_features_move_ahead_of_airspaces() {
        let reordered = prioritize_point_features(vec![
            airspace("CTR Nuernberg"),
            airspace("TMA Nuernberg"),
            airport("Nuernberg"),
            navaid("Nuernberg VOR"),
        ]);
        assert_eq!(
            names(&reordered),
            [
                "Nuernberg",
                "Nuernberg VOR",
                "CTR Nuernberg",
                "TMA Nuernberg"
            ]
        );
    }

    #[test]
    fn airspace_only_hits_are_untouched() {
        let reordered =
            prioritize_point_features(vec![airspace("CTR Nuernberg"), airspace("TMA Nuernberg")]);
        assert_eq!(names(&reordered), ["CTR Nuernberg", "TMA Nuernberg"]);
    }

    // --- feed coverage skip -------------------------------------------------

    fn snapshot(south: f64, west: f64, north: f64, east: f64, zoom: f64) -> CameraSnapshot {
        CameraSnapshot {
            center: strata_render::LatLon::new((south + north) / 2.0, (west + east) / 2.0),
            zoom,
            bounds: (
                strata_render::LatLon::new(south, west),
                strata_render::LatLon::new(north, east),
            ),
        }
    }

    fn coverage(zoom: f64) -> FeedCoverage {
        FeedCoverage {
            global: false,
            bbox: Some(BboxCoverage {
                bbox: BoundingBox::new(9.0, 48.0, 12.0, 51.0).unwrap(),
                gates: FeedGates::at_zoom(zoom),
            }),
        }
    }

    #[test]
    fn settle_inside_fed_bbox_with_same_gates_is_covered() {
        let fed = coverage(7.5);
        let inside = snapshot(48.5, 9.5, 50.5, 11.5, 7.4);
        assert!(feed_covered(&fed, &inside));
    }

    #[test]
    fn settle_with_a_corner_outside_the_fed_bbox_refeeds() {
        let fed = coverage(7.5);
        let east_of = snapshot(48.5, 11.5, 50.5, 13.5, 7.5);
        assert!(!feed_covered(&fed, &east_of));
    }

    /// Crossing a feed zoom gate (reporting points at 8.0 here) selects more
    /// feature kinds, so a contained bbox must still refeed.
    #[test]
    fn settle_across_a_zoom_gate_refeeds() {
        let fed = coverage(7.5);
        let zoomed_in = snapshot(48.5, 9.5, 50.5, 11.5, 8.5);
        assert!(!feed_covered(&fed, &zoomed_in));
    }

    // --- global (warm-fed) coverage ------------------------------------------

    /// Below every point gate, global coverage means *nothing* is left to
    /// query — any camera move anywhere is covered, with or without a
    /// previous bbox feed.
    #[test]
    fn global_coverage_covers_airspace_only_zooms_everywhere() {
        let global = FeedCoverage {
            global: true,
            bbox: None,
        };
        assert!(feed_covered(&global, &snapshot(48.5, 9.5, 50.5, 11.5, 6.5)));
        // Far outside any bbox a viewport feed would ever have queried.
        assert!(feed_covered(
            &global,
            &snapshot(53.0, 13.0, 54.5, 14.5, 7.9)
        ));
    }

    /// Gated point kinds are the only thing global coverage does NOT span:
    /// past their zoom gates a bbox feed is still required.
    #[test]
    fn global_coverage_still_refeeds_gated_point_kinds() {
        let global = FeedCoverage {
            global: true,
            bbox: None,
        };
        assert!(!feed_covered(
            &global,
            &snapshot(48.5, 9.5, 50.5, 11.5, 8.5)
        ));

        // Once the gated kinds were bbox-fed at matching gates, settles
        // inside that envelope are covered again …
        let with_bbox = FeedCoverage {
            bbox: coverage(8.5).bbox,
            global: true,
        };
        assert!(feed_covered(
            &with_bbox,
            &snapshot(48.5, 9.5, 50.5, 11.5, 8.5)
        ));
        // … but opening the next gate (obstacles at 9.0) refeeds.
        assert!(!feed_covered(
            &with_bbox,
            &snapshot(48.5, 9.5, 50.5, 11.5, 9.5)
        ));
    }

    /// Data reload / renderer recreation voids all coverage, including the
    /// warm feed's global flag — the next feed must hit the store again.
    #[test]
    fn reload_clears_global_coverage() {
        let mut cov = FeedCoverage {
            bbox: coverage(8.5).bbox,
            global: true,
        };
        let view = snapshot(48.5, 9.5, 50.5, 11.5, 6.5);
        assert!(feed_covered(&cov, &view));
        cov.clear();
        assert!(!cov.global);
        assert!(!feed_covered(&cov, &view));
    }

    // --- warm-feed threshold --------------------------------------------------

    /// The warm-feed decision over the store's airspace count. Germany is
    /// ~750 airspaces and a handful of enabled countries stays within the
    /// threshold (still warm-fed whole); a store grown past it — e.g. a
    /// synthetic 25k multi-country count — falls back to bounded viewport
    /// feeding, with the renderer-side mesh LRU bounding the GPU side
    /// independently.
    #[test]
    fn warm_feed_threshold() {
        assert!(warm_feed_allowed(0));
        assert!(warm_feed_allowed(750)); // Germany
        assert!(warm_feed_allowed(5_000)); // several countries
        assert!(warm_feed_allowed(WARM_FEED_MAX_AIRSPACES));
        assert!(!warm_feed_allowed(WARM_FEED_MAX_AIRSPACES + 1));
        // Many-country store: over threshold → plain viewport feeding.
        assert!(!warm_feed_allowed(25_000));
    }

    // --- eager-feed constants ---------------------------------------------------

    #[test]
    fn eager_feed_constants() {
        assert_eq!(FEED_DEBOUNCE, Duration::from_millis(50));
        assert!((FEED_BBOX_MARGIN - 0.6).abs() < 1e-12);
        assert_eq!(WARM_FEED_MAX_AIRSPACES, 20_000);
        // Streaming must not outpace its own cadence guard.
        assert!(FEED_STREAM_INTERVAL >= FEED_DEBOUNCE);
        assert!(FEED_STREAM_AFTER >= FEED_DEBOUNCE);
    }

    // --- fly-to target snapshot --------------------------------------------------

    /// The predicted landing bbox must match the camera math: centered on
    /// the target, `viewport_px / (256 · 2^zoom) · 360°` of longitude wide.
    #[test]
    fn fly_target_snapshot_matches_camera_math() {
        let viewport = DVec2::new(1024.0, 768.0);
        let (lat, lon, zoom) = (50.0379, 8.5622, 11.0);
        let target = fly_target_snapshot(lat, lon, zoom, viewport);

        assert_eq!(target.zoom, zoom);
        assert!((target.center.lat_deg() - lat).abs() < 1e-9);
        assert!((target.center.lon_deg() - lon).abs() < 1e-9);

        let (sw, ne) = target.bounds;
        // Ordering: a real (south_west, north_east) pair.
        assert!(sw.lat_deg() < ne.lat_deg());
        assert!(sw.lon_deg() < ne.lon_deg());
        // Longitude span is exact in Web-Mercator (x is linear in lon).
        let want_lon_span = viewport.x / (256.0 * zoom.exp2()) * 360.0;
        assert!((ne.lon_deg() - sw.lon_deg() - want_lon_span).abs() < 1e-9);
        // The bbox is centered on the target.
        assert!(((sw.lon_deg() + ne.lon_deg()) / 2.0 - lon).abs() < 1e-9);
        assert!(((sw.lat_deg() + ne.lat_deg()) / 2.0 - lat).abs() < 0.05);
    }

    /// The camera clamps fly-to zooms; the prediction must agree or the
    /// landing viewport would differ from the predicted one.
    #[test]
    fn fly_target_snapshot_clamps_zoom_like_the_camera() {
        let viewport = DVec2::new(800.0, 600.0);
        assert_eq!(
            fly_target_snapshot(50.0, 8.5, 99.0, viewport).zoom,
            strata_render::MAX_ZOOM
        );
        assert_eq!(
            fly_target_snapshot(50.0, 8.5, 0.5, viewport).zoom,
            strata_render::MIN_ZOOM
        );
    }

    /// The predicted bbox must cover what the camera reports after landing,
    /// so the settle feed finds itself covered (with margin to spare).
    #[test]
    fn fly_target_bbox_covers_the_settled_camera() {
        let viewport = DVec2::new(1280.0, 800.0);
        let target = fly_target_snapshot(48.3537, 11.7751, 11.0, viewport);
        let fed = FeedCoverage {
            global: false,
            bbox: Some(BboxCoverage {
                bbox: feed_bbox(&target).unwrap(),
                gates: FeedGates::at_zoom(target.zoom),
            }),
        };
        // The settled camera reports (sub-pixel) the same pose.
        assert!(feed_covered(&fed, &target));
    }

    // --- route fit (opening a flight frames its route) ---------------------------

    #[test]
    fn route_fit_frames_the_bbox_with_margin() {
        let viewport = DVec2::new(1600.0, 900.0);
        // EDFE → EDQN, roughly west–east across Franconia.
        let route = [
            GeoLatLon::new(49.96, 8.64).unwrap(),
            GeoLatLon::new(49.49, 9.92).unwrap(),
            GeoLatLon::new(49.61, 11.21).unwrap(),
        ];
        let (lat, lon, zoom) = route_fit_view(&route, viewport).expect("non-empty route fits");
        assert!(zoom <= ROUTE_FIT_MAX_ZOOM);
        assert!(zoom >= strata_render::MIN_ZOOM);

        // The settled camera at the fit pose must contain every waypoint
        // (that is what "fits with margin" means).
        let target = fly_target_snapshot(lat, lon, zoom, viewport);
        let (sw, ne) = target.bounds;
        for p in &route {
            assert!(
                sw.lat_deg() <= p.lat() && p.lat() <= ne.lat_deg(),
                "lat {} outside {}..{}",
                p.lat(),
                sw.lat_deg(),
                ne.lat_deg()
            );
            assert!(
                sw.lon_deg() <= p.lon() && p.lon() <= ne.lon_deg(),
                "lon {} outside {}..{}",
                p.lon(),
                sw.lon_deg(),
                ne.lon_deg()
            );
        }
        // …and not drown it: one more zoom step must overflow the viewport
        // on at least one axis (the fit is tight up to the margin).
        let closer = fly_target_snapshot(lat, lon, zoom + 1.0, viewport);
        let (csw, cne) = closer.bounds;
        let contained = route.iter().all(|p| {
            csw.lat_deg() <= p.lat()
                && p.lat() <= cne.lat_deg()
                && csw.lon_deg() <= p.lon()
                && p.lon() <= cne.lon_deg()
        });
        assert!(!contained, "fit zoom is not maximal");
    }

    #[test]
    fn route_fit_caps_zoom_and_skips_empty_routes() {
        let viewport = DVec2::new(1600.0, 900.0);
        assert_eq!(route_fit_view(&[], viewport), None, "empty route");

        // A single waypoint (degenerate bbox) lands exactly at the cap,
        // centered on the point.
        let solo = [GeoLatLon::new(49.4987, 11.078).unwrap()];
        let (lat, lon, zoom) = route_fit_view(&solo, viewport).expect("one point");
        assert_eq!(zoom, ROUTE_FIT_MAX_ZOOM);
        assert!((lat - 49.4987).abs() < 1e-9);
        assert!((lon - 11.078).abs() < 1e-9);

        // A tiny two-point hop also hits the cap (never closer).
        let hop = [
            GeoLatLon::new(49.50, 11.07).unwrap(),
            GeoLatLon::new(49.51, 11.09).unwrap(),
        ];
        let (_, _, zoom) = route_fit_view(&hop, viewport).expect("hop fits");
        assert_eq!(zoom, ROUTE_FIT_MAX_ZOOM);
    }

    // --- GPU device-loss decision -------------------------------------------

    #[test]
    fn device_recreation_decision() {
        // Lost: always stop, regardless of the swap check.
        assert!(device_needs_recreation(Some(true), false));
        assert!(device_needs_recreation(Some(true), true));
        // Recovered behind our back: the swapped device forces recreation.
        assert!(device_needs_recreation(Some(false), true));
        assert!(!device_needs_recreation(Some(false), false));
        // Backends that cannot report device loss keep painting.
        assert!(!device_needs_recreation(None, false));
    }
}

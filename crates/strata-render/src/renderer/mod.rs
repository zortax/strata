//! The map renderer: owns the camera, layers, workers and the
//! double-buffered offscreen target. gpui-free; the app embeds the returned
//! texture via its surface element.

mod config;
mod target;

pub use config::{RendererConfig, clear_color_from_palette, clear_color_from_srgb8};

use self::target::RenderTargets;
use crate::basemap::BasemapLayer;
use crate::camera::{Camera, Viewport};
use crate::error::RenderError;
use crate::features::{
    GriddedField, RenderAirspace, RenderPointFeature, RenderRoute, RenderSigmet, WeatherGridFrame,
};
use crate::geo::{self, LatLon};
use crate::gpu::shader::ShaderLibrary;
use crate::gpu::{GLOBALS_BIND_GROUP_INDEX, Globals};
use crate::input::MapInput;
use crate::layer::{DrawCtx, FrameInfo, LayerId, LayerToggles, MapLayer, PrepareCtx};
use crate::layers::{AirspaceLayer, GriddedWeatherLayer, PointLayer, RouteLayer, WeatherLayer};
use crate::map_theme::MapTheme;
use crate::terrain::TerrainLayer;
use crate::text::TextSystem;
use crate::workers::WorkerPool;

use glam::{DVec2, UVec2};
use std::sync::Arc;
use std::time::Duration;

/// Whether the app should schedule another frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redraw {
    Needed,
    Idle,
}

/// Camera pose in geographic terms, for app-side store queries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraSnapshot {
    pub center: LatLon,
    pub zoom: f64,
    /// Visible bbox as `(south_west, north_east)`.
    pub bounds: (LatLon, LatLon),
}

/// The wgpu map renderer. All methods are render-thread-cheap; IO and
/// tessellation run on the internal worker pool.
pub struct MapRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    clear_color: wgpu::Color,
    camera: Camera,
    toggles: LayerToggles,
    globals: Globals,
    shaders: ShaderLibrary,
    workers: WorkerPool,
    targets: RenderTargets,
    terrain: TerrainLayer,
    basemap: BasemapLayer,
    /// Kept for [`set_basemap_source`](Self::set_basemap_source) /
    /// [`set_terrain_source`](Self::set_terrain_source) (late installs after
    /// `strata-ingest` runs while the app is open). The basemap value is the
    /// *fallback* — a source reporting [`crate::tiles::TileSource::max_zoom`]
    /// wins.
    max_basemap_zoom: u8,
    max_terrain_zoom: u8,
    basemap_detail_bias: f64,
    /// Active map color theme, shared with the layers (they clone the `Arc`
    /// into their worker jobs).
    theme: Arc<MapTheme>,
    /// Bumped on every effective [`set_map_theme`](Self::set_map_theme):
    /// everything color-dependent regenerates under the new generation.
    style_generation: u64,
    airspace: AirspaceLayer,
    points: PointLayer,
    route: RouteLayer,
    weather: WeatherLayer,
    weather_grid: GriddedWeatherLayer,
    text: TextSystem,
    cursor_px: Option<DVec2>,
    frame_index: u64,
    last_dt: Duration,
    dirty: bool,
}

impl MapRenderer {
    /// Targets start at 1×1 — call [`resize`](Self::resize) with the real
    /// surface size before the first frame.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        config: RendererConfig,
    ) -> Result<Self, RenderError> {
        let shaders = ShaderLibrary::embedded();
        // Resolve everything up front so broken includes fail at init, not
        // mid-frame at first pipeline creation.
        for name in shaders.names().collect::<Vec<_>>() {
            shaders.resolve(name)?;
        }
        let globals = Globals::new(&device);
        let workers = WorkerPool::new(config.worker_threads.unwrap_or_else(default_worker_threads));
        let targets = RenderTargets::new(&device, config.format, UVec2::ONE);
        let mut text = TextSystem::new(&device, &queue);
        let theme = Arc::new(config.theme);
        text.set_halo_color(theme.labels.halo);
        let mut terrain = TerrainLayer::new(config.terrain_source, config.max_terrain_zoom);
        terrain.set_style(theme.terrain);
        tracing::debug!(
            workers = workers.thread_count(),
            format = ?config.format,
            theme = theme.id,
            "map renderer created"
        );
        Ok(Self {
            camera: Camera::new(Viewport::default()),
            // Everything on except the gridded weather overlays (opt-in).
            toggles: LayerToggles::standard(),
            terrain,
            basemap: BasemapLayer::new(
                config.basemap_source,
                config.max_basemap_zoom,
                config.basemap_detail_bias,
                Arc::clone(&theme),
            ),
            max_basemap_zoom: config.max_basemap_zoom,
            max_terrain_zoom: config.max_terrain_zoom,
            basemap_detail_bias: config.basemap_detail_bias,
            airspace: AirspaceLayer::with_cache_budget(
                Arc::clone(&theme),
                config.airspace_mesh_cache_bytes,
            ),
            points: PointLayer::new(Arc::clone(&theme)),
            route: RouteLayer::new(Arc::clone(&theme)),
            weather: WeatherLayer::new(Arc::clone(&theme)),
            weather_grid: GriddedWeatherLayer::new(Arc::clone(&theme)),
            text,
            format: config.format,
            // Matches the basemap land fill so a not-yet-loaded tile is
            // indistinguishable from land — no hard edge while tiles fade in.
            clear_color: clear_color_from_palette(theme.clear_color),
            theme,
            style_generation: 0,
            globals,
            shaders,
            workers,
            targets,
            device,
            queue,
            cursor_px: None,
            frame_index: 0,
            last_dt: Duration::ZERO,
            dirty: true,
        })
    }

    /// Resize the offscreen target (physical pixels) and viewport.
    pub fn resize(&mut self, size_px: UVec2, scale_factor: f32) {
        self.camera
            .set_viewport(Viewport::new(size_px, scale_factor));
        self.targets.resize(&self.device, size_px.max(UVec2::ONE));
        self.dirty = true;
    }

    /// Feed a user-interaction event.
    pub fn input(&mut self, event: MapInput) {
        match event {
            MapInput::PanBy { delta_px } => {
                self.camera.pan_by(delta_px);
                self.dirty = true;
            }
            MapInput::PanEnd => {
                // Reserved for release inertia.
            }
            MapInput::ZoomAbout {
                anchor_px,
                zoom_delta,
            } => {
                self.camera.zoom_about(anchor_px, zoom_delta);
                self.dirty = true;
            }
            MapInput::FlyTo { lat_lon, zoom } => {
                self.camera.fly_to(lat_lon, zoom);
                self.dirty = true;
            }
            MapInput::CursorMoved { px } => {
                self.cursor_px = Some(px);
            }
        }
    }

    pub fn set_layer_enabled(&mut self, layer: LayerId, enabled: bool) {
        if self.toggles.enabled(layer) != enabled {
            self.toggles.set(layer, enabled);
            self.dirty = true;
        }
    }

    pub fn layer_enabled(&self, layer: LayerId) -> bool {
        self.toggles.enabled(layer)
    }

    /// Replace the airspace set. Identical data keeps the renderer idle.
    pub fn set_airspaces(&mut self, airspaces: Vec<RenderAirspace>) {
        if self.airspace.set_airspaces(airspaces) {
            self.dirty = true;
        }
    }

    /// Replace the point-feature set (airports, navaids, reporting points,
    /// obstacles, METAR stations). Identical data keeps the renderer idle.
    pub fn set_points(&mut self, points: Vec<RenderPointFeature>) {
        if self.points.set_points(points) {
            self.dirty = true;
        }
    }

    /// Replace the SIGMET set. Identical data keeps the renderer idle.
    pub fn set_sigmets(&mut self, sigmets: Vec<RenderSigmet>) {
        if self.weather.set_sigmets(sigmets) {
            self.dirty = true;
        }
    }

    /// Set (or clear with `None`) the planned flight route. Identical data
    /// keeps the renderer idle; a change in `scrub_along_m` alone
    /// repositions the scrub marker without re-tessellating the polyline.
    /// With no route set the layer draws nothing — the explorer map is
    /// untouched.
    pub fn set_route(&mut self, route: Option<RenderRoute>) {
        if self.route.set_route(route) {
            self.dirty = true;
        }
    }

    /// The currently set route, if any.
    pub fn route(&self) -> Option<&RenderRoute> {
        self.route.route()
    }

    /// Replace the gridded-weather working set for one field (the frames
    /// the time slider scrubs through). Identical data keeps the renderer
    /// idle; invalid frames are dropped with a warning (see
    /// [`WeatherGridFrame`] for the grid contract). Textures upload lazily
    /// — only the frames bracketing the current weather time go to the GPU.
    pub fn set_weather_frames(&mut self, field: GriddedField, frames: Vec<WeatherGridFrame>) {
        if self.weather_grid.set_frames(field, frames) && self.toggles.enabled(field.layer()) {
            self.dirty = true;
        }
    }

    /// Move the weather time slider (unix seconds). The bracketing frames
    /// are blended in the shader, so scrubbing renders continuously.
    /// Redraws only happen when a toggled-on weather field actually holds
    /// frames — an idle map stays idle.
    pub fn set_weather_time(&mut self, unix_seconds: i64) {
        if self.weather_grid.set_time(unix_seconds)
            && self.weather_grid.any_visible_field(&self.toggles)
        {
            self.dirty = true;
        }
    }

    /// The current weather-time slider position (unix seconds).
    pub fn weather_time(&self) -> i64 {
        self.weather_grid.time()
    }

    /// Install (or replace) the basemap tile source — e.g. when
    /// `strata-ingest basemap` finishes while the app is running. Rebuilding
    /// the layer also discards any cached tiles and negative misses.
    pub fn set_basemap_source(&mut self, source: Option<Arc<dyn crate::tiles::TileSource>>) {
        self.basemap = BasemapLayer::new(
            source,
            self.max_basemap_zoom,
            self.basemap_detail_bias,
            Arc::clone(&self.theme),
        );
        self.dirty = true;
    }

    /// Switch the map color theme at runtime. Everything color-dependent
    /// regenerates without a restart: cached basemap tiles are dropped and
    /// re-tessellated, airspace/point/weather artifacts rebuild, the symbol
    /// atlas re-uploads, terrain tint uniforms and the clear color update,
    /// and the label halo follows the theme. An identical theme is a no-op.
    pub fn set_map_theme(&mut self, theme: MapTheme) {
        if *self.theme == theme {
            return;
        }
        let theme = Arc::new(theme);
        self.theme = Arc::clone(&theme);
        self.style_generation += 1;
        self.clear_color = clear_color_from_palette(theme.clear_color);
        self.basemap.set_theme(Arc::clone(&theme));
        self.airspace.set_theme(Arc::clone(&theme));
        self.points.set_theme(Arc::clone(&theme));
        self.route.set_theme(Arc::clone(&theme));
        self.weather.set_theme(Arc::clone(&theme));
        self.weather_grid.set_theme(Arc::clone(&theme));
        self.terrain.set_style(theme.terrain);
        self.text.set_halo_color(theme.labels.halo);
        self.dirty = true;
        tracing::info!(theme = theme.id, generation = self.style_generation, "map theme switched");
    }

    /// The active map color theme.
    pub fn map_theme(&self) -> &MapTheme {
        &self.theme
    }

    /// Monotonic counter bumped by every effective
    /// [`set_map_theme`](Self::set_map_theme) — lets the app (and tests)
    /// observe that a style regeneration happened.
    pub fn style_generation(&self) -> u64 {
        self.style_generation
    }

    /// Change the basemap detail bias at runtime (see
    /// [`RendererConfig::basemap_detail_bias`]) — wired so it can become a
    /// user setting later.
    pub fn set_basemap_detail_bias(&mut self, bias: f64) {
        if self.basemap_detail_bias != bias {
            self.basemap_detail_bias = bias;
            self.basemap.set_detail_bias(bias);
            self.dirty = true;
        }
    }

    pub fn basemap_detail_bias(&self) -> f64 {
        self.basemap_detail_bias
    }

    /// Install (or replace) the terrain tile source — e.g. when the store
    /// only became available after startup.
    pub fn set_terrain_source(&mut self, source: Option<Arc<dyn crate::tiles::TileSource>>) {
        self.terrain = TerrainLayer::new(source, self.max_terrain_zoom);
        self.terrain.set_style(self.theme.terrain);
        self.dirty = true;
    }

    /// Advance animations. While this returns [`Redraw::Needed`] the app
    /// should call [`render`](Self::render) and schedule another tick.
    pub fn tick(&mut self, dt: Duration) -> Redraw {
        self.last_dt = dt;
        if self.camera.tick(dt) {
            self.dirty = true;
        }
        let layers_want = self.terrain.wants_redraw()
            || self.basemap.wants_redraw()
            || self.airspace.wants_redraw()
            || self.points.wants_redraw()
            || self.route.wants_redraw()
            || self.weather.wants_redraw()
            || self.weather_grid.wants_redraw();
        if self.dirty || layers_want {
            Redraw::Needed
        } else {
            Redraw::Idle
        }
    }

    /// Render one frame into the back buffer and return the just-completed
    /// texture (`RENDER_ATTACHMENT | TEXTURE_BINDING`). Double-buffered: the
    /// texture returned by the *previous* call stays valid for sampling
    /// while the next frame is drawn.
    pub fn render(&mut self) -> &wgpu::Texture {
        self.globals.update(&self.queue, &self.camera);

        let frame = FrameInfo {
            dt: self.last_dt,
            frame_index: self.frame_index,
        };
        {
            let mut ctx = PrepareCtx {
                device: &self.device,
                queue: &self.queue,
                camera: &self.camera,
                workers: &self.workers,
                layers: &self.toggles,
                frame,
                target_format: self.format,
                globals_layout: &self.globals.layout,
                shaders: &self.shaders,
            };
            if ctx.layers.enabled(LayerId::Terrain) {
                self.terrain.prepare(&mut ctx);
            }
            if ctx.layers.enabled(LayerId::Basemap) {
                self.basemap.prepare(&mut ctx);
            }
            if ctx.layers.any_enabled(&GriddedWeatherLayer::CATEGORIES) {
                self.weather_grid.prepare(&mut ctx);
            }
            if ctx.layers.enabled(LayerId::Airspace) {
                self.airspace.prepare(&mut ctx);
            }
            if ctx.layers.any_enabled(&PointLayer::CATEGORIES)
                || ctx.layers.enabled(LayerId::Weather)
            {
                self.points.prepare(&mut ctx);
            }
            if ctx.layers.enabled(LayerId::Route) {
                self.route.prepare(&mut ctx);
            }
            if ctx.layers.enabled(LayerId::Weather) {
                self.weather.prepare(&mut ctx);
            }
            // Forward this frame's labels into the text overlay: after the
            // layer prepares (which compute the label sets) and before the
            // text prepare (which consumes the queue). Labels below their
            // zoom gate are filtered here, before the per-frame clone —
            // `place_labels` culls again for labels pushed via `LabelQueue`
            // or by the app.
            let zoom = self.camera.zoom() as f32;
            if ctx.layers.enabled(LayerId::Basemap) {
                for label in self.basemap.pending_labels() {
                    if zoom >= label.min_zoom {
                        self.text.queue_label(label.clone());
                    }
                }
            }
            if ctx.layers.enabled(LayerId::Airspace) {
                for label in self.airspace.labels() {
                    if zoom >= label.min_zoom {
                        self.text.queue_label(label.clone());
                    }
                }
            }
            for label in self.points.visible_labels(ctx.layers) {
                if zoom >= label.min_zoom {
                    self.text.queue_label(label.clone());
                }
            }
            if ctx.layers.enabled(LayerId::Route) {
                for label in self.route.labels() {
                    if zoom >= label.min_zoom {
                        self.text.queue_label(label.clone());
                    }
                }
            }
            if ctx.layers.enabled(LayerId::Weather) {
                for label in self.weather.labels() {
                    if zoom >= label.min_zoom {
                        self.text.queue_label(label.clone());
                    }
                }
            }
            self.text.prepare(&mut ctx);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("strata map encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("strata map pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.targets.back_view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(GLOBALS_BIND_GROUP_INDEX, &self.globals.bind_group, &[]);

            let ctx = DrawCtx {
                camera: &self.camera,
                layers: &self.toggles,
                globals: &self.globals.bind_group,
            };
            // Z-order: terrain → basemap → gridded weather (cloud cover,
            // precipitation, thunderstorms) → airspace → points (obstacles,
            // navaids, reporting points, airports) → route → weather → text
            // overlay.
            if ctx.layers.enabled(LayerId::Terrain) {
                self.terrain.draw(&mut pass, &ctx);
            }
            if ctx.layers.enabled(LayerId::Basemap) {
                self.basemap.draw(&mut pass, &ctx);
            }
            if ctx.layers.any_enabled(&GriddedWeatherLayer::CATEGORIES) {
                self.weather_grid.draw(&mut pass, &ctx);
            }
            if ctx.layers.enabled(LayerId::Airspace) {
                self.airspace.draw(&mut pass, &ctx);
            }
            if ctx.layers.any_enabled(&PointLayer::CATEGORIES)
                || ctx.layers.enabled(LayerId::Weather)
            {
                self.points.draw(&mut pass, &ctx);
            }
            if ctx.layers.enabled(LayerId::Route) {
                self.route.draw(&mut pass, &ctx);
            }
            if ctx.layers.enabled(LayerId::Weather) {
                self.weather.draw(&mut pass, &ctx);
            }
            self.text.draw(&mut pass, &ctx);
        }
        self.queue.submit([encoder.finish()]);

        self.frame_index += 1;
        self.dirty = false;
        self.targets.swap()
    }

    /// The most recently completed frame (what the app should sample).
    pub fn texture(&self) -> &wgpu::Texture {
        self.targets.front_texture()
    }

    /// Geographic camera pose for app-side store queries.
    pub fn camera(&self) -> CameraSnapshot {
        let (min, max) = self.camera.visible_world_bounds();
        CameraSnapshot {
            center: geo::lat_lon_from_world(self.camera.center()),
            zoom: self.camera.zoom(),
            bounds: (
                geo::lat_lon_from_world(DVec2::new(min.x, max.y)),
                geo::lat_lon_from_world(DVec2::new(max.x, min.y)),
            ),
        }
    }

    /// Screen (logical px) → geographic, for app-side hit testing.
    pub fn pick(&self, px: DVec2) -> LatLon {
        self.camera.pick(px)
    }

    /// Last position reported via [`MapInput::CursorMoved`].
    pub fn cursor_px(&self) -> Option<DVec2> {
        self.cursor_px
    }
}

fn default_worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2))
        .unwrap_or(2)
        .clamp(2, 8)
}

//! # strata-render
//!
//! wgpu map renderer for Strata. **No gpui dependency** — the app obtains
//! `Arc<wgpu::Device>` / `Arc<wgpu::Queue>` from its windowing stack, drives
//! [`MapRenderer`] with [`MapInput`] events and `tick`/`render` calls, and
//! embeds the returned offscreen texture.
//!
//! Camera math is f64 Web-Mercator over a normalized `[0, 1]^2` world; see
//! [`camera`]. Layers follow the [`layer::MapLayer`] prepare/draw split with
//! all IO and tessellation on the internal worker pool ([`workers`]).

pub mod basemap;
pub mod camera;
pub mod error;
pub mod features;
pub mod geo;
pub mod gpu;
pub mod input;
pub mod layer;
pub mod layers;
pub mod map_theme;
pub mod renderer;
pub mod terrain;
pub mod text;
pub mod tiles;
pub mod workers;

pub use camera::{Camera, CameraState, MAX_ZOOM, MIN_ZOOM, Viewport};
pub use error::RenderError;
pub use features::{
    AirspaceStyleKey, FlightCategoryColor, GriddedField, IcaoClass, PointKind, RenderAirspace,
    RenderPointFeature, RenderRoute, RenderSigmet, RoutePointKind, RouteVertex, WeatherGridFrame,
};
pub use geo::LatLon;
pub use input::MapInput;
pub use layer::{DrawCtx, LayerId, LayerToggles, MapLayer, PrepareCtx};
pub use map_theme::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme,
};
pub use renderer::{CameraSnapshot, MapRenderer, Redraw, RendererConfig};
pub use tiles::{TileId, TileSource};

// Re-exported so the app and strata-render agree on math/GPU types by
// construction (wgpu must stay the zed fork shared with gpui-ce).
pub use glam;
pub use wgpu;

//! Layer architecture: z-ordered toggleable layers with a prepare/draw split.
//!
//! `prepare` runs on the render thread but must stay cheap: drain worker
//! results, upload buffers, kick new jobs. `draw` only binds and draws.

use crate::camera::Camera;
use crate::gpu::shader::ShaderLibrary;
use crate::workers::WorkerPool;

use std::time::Duration;

/// Identity and z-order of the toggleable map layers (bottom to top).
///
/// The gridded weather overlays (cloud cover, precipitation, thunderstorms)
/// sit above the ground reference (terrain + basemap) but below the
/// airspace structure and every symbol/label, so weather never obscures
/// chart information. The flight route draws above the airports it connects
/// but below the live weather symbols. Declaration order is z-order; `Ord`
/// follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LayerId {
    Terrain,
    Basemap,
    /// Gridded cloud-cover overlay (default off).
    CloudCover,
    /// Gridded precipitation overlay (default off).
    Precipitation,
    /// Gridded thunderstorm-potential overlay (default off).
    Thunderstorms,
    Airspace,
    Obstacles,
    Navaids,
    ReportingPoints,
    Airports,
    /// The planned flight route (polyline, handles, markers). Not a
    /// user-facing toggle — it stays enabled and simply draws nothing while
    /// no route is set ([`crate::renderer::MapRenderer::set_route`]); the
    /// app's layers panel deliberately does not list it.
    Route,
    Weather,
}

impl LayerId {
    pub const COUNT: usize = 12;

    /// All layers in z-order (bottom first).
    pub const ALL: [LayerId; Self::COUNT] = [
        LayerId::Terrain,
        LayerId::Basemap,
        LayerId::CloudCover,
        LayerId::Precipitation,
        LayerId::Thunderstorms,
        LayerId::Airspace,
        LayerId::Obstacles,
        LayerId::Navaids,
        LayerId::ReportingPoints,
        LayerId::Airports,
        LayerId::Route,
        LayerId::Weather,
    ];

    /// Stable dense index (z-order position).
    pub fn index(self) -> usize {
        match self {
            LayerId::Terrain => 0,
            LayerId::Basemap => 1,
            LayerId::CloudCover => 2,
            LayerId::Precipitation => 3,
            LayerId::Thunderstorms => 4,
            LayerId::Airspace => 5,
            LayerId::Obstacles => 6,
            LayerId::Navaids => 7,
            LayerId::ReportingPoints => 8,
            LayerId::Airports => 9,
            LayerId::Route => 10,
            LayerId::Weather => 11,
        }
    }

    /// True for the gridded weather overlays, which start disabled (see
    /// [`LayerToggles::standard`]).
    pub fn default_off(self) -> bool {
        matches!(
            self,
            LayerId::CloudCover | LayerId::Precipitation | LayerId::Thunderstorms
        )
    }
}

/// Per-layer enabled flags. The default ([`Self::standard`]) enables
/// everything except the gridded weather overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerToggles {
    enabled: [bool; LayerId::COUNT],
}

impl LayerToggles {
    pub fn all_enabled() -> Self {
        Self {
            enabled: [true; LayerId::COUNT],
        }
    }

    /// The startup defaults: every layer on except the gridded weather
    /// overlays (cloud cover, precipitation, thunderstorms), which are
    /// opt-in.
    pub fn standard() -> Self {
        let mut toggles = Self::all_enabled();
        for id in LayerId::ALL {
            if id.default_off() {
                toggles.set(id, false);
            }
        }
        toggles
    }

    pub fn enabled(&self, id: LayerId) -> bool {
        self.enabled[id.index()]
    }

    pub fn set(&mut self, id: LayerId, on: bool) {
        self.enabled[id.index()] = on;
    }

    /// True if any of `ids` is enabled (used by layers that render several
    /// toggle categories, e.g. the point layer).
    pub fn any_enabled(&self, ids: &[LayerId]) -> bool {
        ids.iter().any(|id| self.enabled(*id))
    }
}

impl Default for LayerToggles {
    fn default() -> Self {
        Self::standard()
    }
}

/// Per-frame timing info.
#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    /// `dt` of the most recent `tick`.
    pub dt: Duration,
    /// Monotonic frame counter (one per `render`).
    pub frame_index: u64,
}

/// Everything a layer needs during `prepare` (render thread, keep it cheap).
pub struct PrepareCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub camera: &'a Camera,
    /// Thread pool for decode / tessellation jobs; results come back through
    /// the layer's own [`crate::workers::JobQueue`].
    pub workers: &'a WorkerPool,
    pub layers: &'a LayerToggles,
    pub frame: FrameInfo,
    /// Color format of the offscreen target — needed for pipeline creation.
    pub target_format: wgpu::TextureFormat,
    /// Bind group layout of the shared globals (group 0 in every shader).
    pub globals_layout: &'a wgpu::BindGroupLayout,
    /// Embedded WGSL library with `//#include` resolution.
    pub shaders: &'a ShaderLibrary,
}

/// Everything a layer needs during `draw`.
pub struct DrawCtx<'a> {
    pub camera: &'a Camera,
    pub layers: &'a LayerToggles,
    /// Shared globals bind group, already updated for this frame. Bind at
    /// group 0 (see `shaders/common.wgsl`).
    pub globals: &'a wgpu::BindGroup,
}

/// A z-ordered map layer.
pub trait MapLayer {
    /// CPU-side work and GPU uploads for the coming frame: drain worker
    /// results, submit new jobs, write buffers. Must not block.
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>);

    /// Record draw commands. No resource creation here.
    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ctx: &DrawCtx<'_>);

    /// True if the layer has pending animations or in-flight worker results
    /// and wants another frame even with an idle camera.
    fn wants_redraw(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` is the z-order; `index()` must be its dense position and `Ord`
    /// (declaration order) must agree.
    #[test]
    fn layer_indices_are_dense_and_match_z_order() {
        assert_eq!(LayerId::ALL.len(), LayerId::COUNT);
        for (position, id) in LayerId::ALL.iter().enumerate() {
            assert_eq!(id.index(), position, "{id:?}");
        }
        for pair in LayerId::ALL.windows(2) {
            assert!(pair[0] < pair[1], "Ord must follow z-order: {pair:?}");
        }
    }

    /// The gridded weather overlays sit above the ground reference
    /// (terrain + basemap) and below the airspace structure.
    #[test]
    fn gridded_weather_layers_sit_between_basemap_and_airspace() {
        for id in [
            LayerId::CloudCover,
            LayerId::Precipitation,
            LayerId::Thunderstorms,
        ] {
            assert!(id.index() > LayerId::Terrain.index());
            assert!(id.index() > LayerId::Basemap.index());
            assert!(id.index() < LayerId::Airspace.index());
        }
    }

    /// The flight route draws above the airports it connects but below the
    /// live weather symbols, and it is enabled by default (it is not a
    /// user-facing toggle — without a route it simply draws nothing).
    #[test]
    fn route_sits_between_airports_and_weather_and_defaults_on() {
        assert!(LayerId::Route.index() > LayerId::Airports.index());
        assert!(LayerId::Route.index() < LayerId::Weather.index());
        assert!(!LayerId::Route.default_off());
        assert!(LayerToggles::standard().enabled(LayerId::Route));
    }

    /// Startup defaults: gridded weather overlays are opt-in, everything
    /// else starts enabled.
    #[test]
    fn standard_toggles_disable_only_the_gridded_weather_overlays() {
        let toggles = LayerToggles::default();
        assert_eq!(toggles, LayerToggles::standard());
        for id in LayerId::ALL {
            assert_eq!(toggles.enabled(id), !id.default_off(), "{id:?}");
        }
        let all = LayerToggles::all_enabled();
        for id in LayerId::ALL {
            assert!(all.enabled(id), "{id:?}");
        }
    }
}

//! Web-Mercator camera with smooth, anchor-stable zoom animation.
//!
//! All camera math is `f64`. Screen space is **logical pixels** (y-down,
//! origin top-left); physical pixels = logical × `scale_factor`. The
//! world→screen scale is `256 · 2^zoom` logical px per world unit.
//!
//! The hard invariant: while a zoom animation runs, the world point captured
//! under the zoom anchor stays under that exact screen point. Every tick
//! first advances `zoom`, then recomputes `center` from the stored
//! `(anchor_world, anchor_screen)` pair — the anchor is honored by
//! construction, not by approximation. See the property test below.

mod viewport;

pub use viewport::Viewport;

use crate::geo::{self, LatLon};

use glam::DVec2;
use std::time::Duration;

/// Minimum continuous zoom level.
pub const MIN_ZOOM: f64 = 4.0;
/// Maximum continuous zoom level.
pub const MAX_ZOOM: f64 = 19.0;
/// Logical pixels per world unit at zoom 0.
pub const TILE_SIZE_PX: f64 = 256.0;
/// Time constant of the exponential zoom / fly-to smoothing.
pub const ANIMATION_TAU_SECONDS: f64 = 0.16;

/// Zoom is considered settled when within this many zoom units of the target.
const ZOOM_SETTLE_EPS: f64 = 1e-4;
/// Fly-to center is settled when the remaining distance is below this many
/// logical pixels on screen.
const CENTER_SETTLE_EPS_PX: f64 = 0.01;

/// The instantaneous camera pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraState {
    /// World-space center in normalized Web-Mercator `[0, 1]^2`.
    pub center: DVec2,
    /// Continuous zoom level.
    pub zoom: f64,
}

#[derive(Debug, Clone, Copy)]
struct ZoomAnchor {
    world: DVec2,
    screen_px: DVec2,
}

#[derive(Debug, Clone, Copy)]
struct FlyTarget {
    center: DVec2,
    zoom: f64,
}

/// Camera with viewport, zoom-target accumulator and animation state.
#[derive(Debug, Clone)]
pub struct Camera {
    state: CameraState,
    viewport: Viewport,
    target_zoom: f64,
    zoom_anchor: Option<ZoomAnchor>,
    fly: Option<FlyTarget>,
}

impl Camera {
    /// Camera over Germany at a country-wide zoom.
    pub fn new(viewport: Viewport) -> Self {
        let center = geo::world_from_lat_lon(LatLon::new(51.1657, 10.4515));
        let zoom = 6.0;
        Self {
            state: CameraState { center, zoom },
            viewport,
            target_zoom: zoom,
            zoom_anchor: None,
            fly: None,
        }
    }

    pub fn state(&self) -> CameraState {
        self.state
    }

    pub fn center(&self) -> DVec2 {
        self.state.center
    }

    pub fn zoom(&self) -> f64 {
        self.state.zoom
    }

    /// Where the zoom animation is heading (equals `zoom()` when idle).
    pub fn target_zoom(&self) -> f64 {
        self.target_zoom
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    /// Logical pixels per world unit: `256 · 2^zoom`.
    pub fn world_scale(&self) -> f64 {
        TILE_SIZE_PX * self.state.zoom.exp2()
    }

    /// World space → logical screen pixels.
    pub fn project(&self, world: DVec2) -> DVec2 {
        (world - self.state.center) * self.world_scale() + self.viewport.logical_size() * 0.5
    }

    /// Logical screen pixels → world space.
    pub fn unproject(&self, screen_px: DVec2) -> DVec2 {
        (screen_px - self.viewport.logical_size() * 0.5) / self.world_scale() + self.state.center
    }

    /// Logical screen pixels → WGS84.
    pub fn pick(&self, screen_px: DVec2) -> LatLon {
        geo::lat_lon_from_world(self.unproject(screen_px))
    }

    /// Visible world-space rectangle `(min, max)`, clamped to `[0, 1]^2`.
    pub fn visible_world_bounds(&self) -> (DVec2, DVec2) {
        let min = self.unproject(DVec2::ZERO).clamp(DVec2::ZERO, DVec2::ONE);
        let max = self
            .unproject(self.viewport.logical_size())
            .clamp(DVec2::ZERO, DVec2::ONE);
        (min, max)
    }

    /// Direct 1:1 pan: the map content follows the cursor by `delta_px`
    /// logical pixels. Cancels a fly-to (user takeover) but composes with a
    /// running zoom animation by re-deriving the anchor's world point.
    pub fn pan_by(&mut self, delta_px: DVec2) {
        self.fly = None;
        self.state.center -= delta_px / self.world_scale();
        self.clamp_center();
        if let Some(screen_px) = self.zoom_anchor.as_ref().map(|a| a.screen_px) {
            let world = self.unproject(screen_px);
            if let Some(anchor) = self.zoom_anchor.as_mut() {
                anchor.world = world;
            }
        }
    }

    /// Accumulate `zoom_delta` into the zoom target and (re-)anchor the
    /// animation at `anchor_px`: the world point currently under `anchor_px`
    /// will stay under it for the rest of the animation.
    pub fn zoom_about(&mut self, anchor_px: DVec2, zoom_delta: f64) {
        self.fly = None;
        self.target_zoom = (self.target_zoom + zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
        self.zoom_anchor = Some(ZoomAnchor {
            world: self.unproject(anchor_px),
            screen_px: anchor_px,
        });
    }

    /// Smooth animated transition of both center and zoom (same exponential
    /// smoothing as wheel zoom). Cancels any zoom anchor.
    pub fn fly_to(&mut self, target: LatLon, zoom: f64) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let center = geo::world_from_lat_lon(target).clamp(DVec2::ZERO, DVec2::ONE);
        self.zoom_anchor = None;
        self.target_zoom = zoom;
        self.fly = Some(FlyTarget { center, zoom });
    }

    pub fn is_animating(&self) -> bool {
        self.fly.is_some()
            || (self.target_zoom - self.state.zoom).abs() > ZOOM_SETTLE_EPS
            || self.zoom_anchor.is_some()
    }

    /// Advance animations by `dt`. Returns `true` if the camera changed.
    ///
    /// dt-independent: the smoothing factor is `1 − exp(−dt/τ)`, so N small
    /// ticks equal one big tick of the same total duration exactly.
    pub fn tick(&mut self, dt: Duration) -> bool {
        if !self.is_animating() {
            return false;
        }
        let alpha = 1.0 - (-dt.as_secs_f64() / ANIMATION_TAU_SECONDS).exp();

        if let Some(fly) = self.fly {
            self.state.zoom += (fly.zoom - self.state.zoom) * alpha;
            self.state.center += (fly.center - self.state.center) * alpha;
            let zoom_settled = (fly.zoom - self.state.zoom).abs() <= ZOOM_SETTLE_EPS;
            let center_settled = (fly.center - self.state.center).length() * self.world_scale()
                <= CENTER_SETTLE_EPS_PX;
            if zoom_settled && center_settled {
                self.state.zoom = fly.zoom;
                self.state.center = fly.center;
                self.fly = None;
            }
            self.clamp_center();
            return true;
        }

        // Wheel-zoom animation: advance zoom, then re-derive center from the
        // anchor so the anchored world point is under anchor_px by
        // construction.
        let remaining = self.target_zoom - self.state.zoom;
        if remaining.abs() > ZOOM_SETTLE_EPS {
            self.state.zoom += remaining * alpha;
        }
        let settled = (self.target_zoom - self.state.zoom).abs() <= ZOOM_SETTLE_EPS;
        if settled {
            self.state.zoom = self.target_zoom;
        }
        if let Some(anchor) = self.zoom_anchor {
            self.state.center = anchor.world
                - (anchor.screen_px - self.viewport.logical_size() * 0.5) / self.world_scale();
        }
        self.clamp_center();
        if settled {
            self.zoom_anchor = None;
        }
        true
    }

    // Keeping the center inside [0,1]^2 can override the anchor invariant at
    // the world edges — that is intended product behavior (the map must not
    // scroll off the world), and irrelevant for a Germany-focused viewport.
    fn clamp_center(&mut self) {
        self.state.center = self.state.center.clamp(DVec2::ZERO, DVec2::ONE);
    }
}

#[cfg(test)]
mod tests;

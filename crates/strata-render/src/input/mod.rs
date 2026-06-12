//! Input vocabulary the application translates platform events into.
//!
//! Screen coordinates are **logical pixels** (same space as
//! [`crate::camera::Camera::project`]).

use crate::geo::LatLon;

use glam::DVec2;

/// A user-interaction event for the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapInput {
    /// Direct 1:1 drag pan; the map content moves by `delta_px`.
    PanBy { delta_px: DVec2 },
    /// Drag released. Reserved for release inertia — currently a no-op.
    PanEnd,
    /// Wheel / pinch zoom anchored at `anchor_px`: accumulates `zoom_delta`
    /// into the zoom target and starts/re-anchors the smooth zoom animation.
    ZoomAbout { anchor_px: DVec2, zoom_delta: f64 },
    /// Animated transition to a location (e.g. search result selection).
    FlyTo { lat_lon: LatLon, zoom: f64 },
    /// Cursor hover position (kept for picking and future hover effects).
    CursorMoved { px: DVec2 },
}

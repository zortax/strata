//#include common.wgsl

// Flight-route strokes: the route polyline (with direction chevrons), the
// dashed alternate links and the terrain-corridor band — one pipeline, the
// per-vertex style fields select the behaviour.
//
// Width is the sum of two extrusions so one vertex format serves both
// stroke families:
//   * `width_px`     — logical pixels, zoom-independent (legs, links);
//   * `width_world`  — world units, ground-fixed (the corridor; the CPU
//     derives it from the corridor half-width in meters per vertex, so the
//     band follows the zoom and the local Mercator scale).
//
// Direction chevrons (`ticks > 0.5`) darken a periodic band whose phase
// leads at the centerline (`across = 0`) and trails at the edges, forming
// a "›" pointing along increasing arc length — the direction of flight.
// `dash_px` is (on, off) in logical px; (0, 0) = solid.
//
// World y grows south (= screen down), so a world-space normal direction is
// already a screen-space direction.

struct RouteLocals {
    origin_rel: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var<uniform> locals: RouteLocals;

// Chevron geometry (logical px) and the tick shade factor.
const TICK_PERIOD_PX: f32 = 26.0;
const TICK_LENGTH_PX: f32 = 4.0;
const TICK_SKEW_PX: f32 = 3.5;
const TICK_DARKEN: f32 = 0.45;

struct RouteVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) along_px: f32,
    // -1..1 across the stroke (interpolated from the edge vertices).
    @location(2) across: f32,
    @location(3) dash_px: vec2<f32>,
    @location(4) ticks: f32,
}

@vertex
fn vs_main(
    @location(0) pos_local: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) width_px: f32,
    @location(4) width_world: f32,
    @location(5) along_world: f32,
    @location(6) side: f32,
    @location(7) dash_px: vec2<f32>,
    @location(8) ticks: f32,
) -> RouteVaryings {
    var out: RouteVaryings;
    let center = world_rel_to_clip(locals.origin_rel + pos_local);
    // Logical px per world unit, recovered from the globals (same recipe as
    // line_dash.wgsl) — converts the world-fixed width component and the
    // arc length into screen-stable logical pixels.
    let px_per_world = globals.camera_to_clip.x * globals.viewport_size_px.x * 0.5
        / globals.scale_factor;
    let half_px = width_px * 0.5 + width_world * px_per_world * 0.5;
    let extrude = logical_px_to_clip(normal * half_px);
    out.clip = vec4<f32>(center.xy + extrude, center.zw);
    out.color = color;
    out.along_px = along_world * px_per_world;
    out.across = side;
    out.dash_px = dash_px;
    out.ticks = ticks;
    return out;
}

@fragment
fn fs_main(in: RouteVaryings) -> @location(0) vec4<f32> {
    let period = in.dash_px.x + in.dash_px.y;
    if (period > 0.0) {
        let t = in.along_px - floor(in.along_px / period) * period;
        if (t > in.dash_px.x) {
            discard;
        }
    }
    var color = in.color;
    if (in.ticks > 0.5) {
        // Chevron band: the phase coordinate leads at the centerline, so
        // the band's tip points toward increasing arc length.
        let u = in.along_px + TICK_SKEW_PX * abs(in.across);
        let t = u - floor(u / TICK_PERIOD_PX) * TICK_PERIOD_PX;
        if (t < TICK_LENGTH_PX) {
            // Darkening a premultiplied color keeps it premultiplied.
            color = vec4<f32>(color.rgb * TICK_DARKEN, color.a);
        }
    }
    return color;
}

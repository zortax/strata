//#include common.wgsl

// Screen-stable stroked outlines (airspace borders, SIGMET outlines).
//
// The CPU tessellates centerline strokes (lyon) carrying a per-vertex
// extrusion normal and the arc length along the path in world units. The
// vertex stage extrudes in logical pixels (zoom-independent width) and
// converts arc length to logical pixels, so the fragment stage can cut
// dashes with screen-stable spacing. `dash_px` is (on, off); (0, 0) = solid.
//
// World y grows south (= screen down), so a world-space normal direction is
// already a screen-space direction.

struct LineLocals {
    origin_rel: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var<uniform> locals: LineLocals;

struct LineVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) along_px: f32,
    @location(2) dash_px: vec2<f32>,
}

@vertex
fn vs_main(
    @location(0) pos_local: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) width_px: f32,
    @location(4) along_world: f32,
    @location(5) dash_px: vec2<f32>,
) -> LineVaryings {
    var out: LineVaryings;
    let center = world_rel_to_clip(locals.origin_rel + pos_local);
    let extrude = logical_px_to_clip(normal * (width_px * 0.5));
    out.clip = vec4<f32>(center.xy + extrude, center.zw);
    out.color = color;
    // Logical px per world unit, recovered from the globals so dash spacing
    // stays fixed on screen at any zoom.
    let px_per_world = globals.camera_to_clip.x * globals.viewport_size_px.x * 0.5
        / globals.scale_factor;
    out.along_px = along_world * px_per_world;
    out.dash_px = dash_px;
    return out;
}

@fragment
fn fs_main(in: LineVaryings) -> @location(0) vec4<f32> {
    let period = in.dash_px.x + in.dash_px.y;
    if (period > 0.0) {
        let t = in.along_px - floor(in.along_px / period) * period;
        if (t > in.dash_px.x) {
            discard;
        }
    }
    return in.color;
}

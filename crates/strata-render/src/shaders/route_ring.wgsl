//#include common.wgsl

// Snap-indication ring: a single screen-sized annulus pulsing around the
// feature a waypoint drag currently snaps onto. One quad generated from
// the vertex index (no vertex buffers); the fragment shader draws an
// anti-aliased ring band by signed distance from the pulse radius. The
// CPU animates the pulse (radius + fade baked into `color`) and re-writes
// the uniform every frame while a ring is shown.

struct RingLocals {
    // Ring center in camera-relative world units (f64 subtraction on the
    // CPU, the local-origin discipline of the other layers).
    center_rel: vec2<f32>,
    // Current pulse radius in logical px.
    radius_px: f32,
    // Ring band thickness in logical px.
    thickness_px: f32,
    // Premultiplied ring color; the pulse fade is pre-applied.
    color: vec4<f32>,
}

@group(1) @binding(0) var<uniform> ring: RingLocals;

struct RingVaryings {
    @builtin(position) clip: vec4<f32>,
    // Logical-px offset from the ring center.
    @location(0) offset_px: vec2<f32>,
}

// Anti-alias feather on each band edge, logical px.
const FEATHER_PX: f32 = 0.75;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> RingVaryings {
    // Two triangles spanning the quad, corners in {-1, 1}.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let extent = ring.radius_px + ring.thickness_px * 0.5 + FEATHER_PX;
    let offset = corners[index] * extent;
    let center = world_rel_to_clip(ring.center_rel);
    var out: RingVaryings;
    out.clip = vec4<f32>(center.xy + logical_px_to_clip(offset), center.zw);
    out.offset_px = offset;
    return out;
}

@fragment
fn fs_main(in: RingVaryings) -> @location(0) vec4<f32> {
    // Distance from the ring centerline; 0 inside the band.
    let d = abs(length(in.offset_px) - ring.radius_px);
    let half_band = ring.thickness_px * 0.5;
    let coverage = 1.0 - smoothstep(half_band - FEATHER_PX, half_band + FEATHER_PX, d);
    // Premultiplied color scales whole by coverage.
    return ring.color * coverage;
}

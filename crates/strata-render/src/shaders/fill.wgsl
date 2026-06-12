//#include common.wgsl

// Polygon fill (airspace fills, basemap polygons, SIGMET overlays).
// Vertices come pre-tessellated (lyon) with a per-vertex premultiplied color.
// TODO(next-phase): dash/hatch patterns for SIGMET move to a variant or
// a pattern flag.

struct FillVertex {
    @location(0) world_rel: vec2<f32>,
    @location(1) color: vec4<f32>,
}

struct FillVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(v: FillVertex) -> FillVaryings {
    var out: FillVaryings;
    out.clip = world_rel_to_clip(v.world_rel);
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: FillVaryings) -> @location(0) vec4<f32> {
    return in.color;
}

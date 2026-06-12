//#include common.wgsl

// Airspace polygon fills, pre-tessellated with lyon on the worker pool.
// Vertex positions are world units relative to a per-dataset origin; the
// origin itself is camera-relative (`locals.origin_rel`, f64 subtraction on
// the CPU each frame) so f32 only ever holds small quantities.

struct AirspaceLocals {
    origin_rel: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var<uniform> locals: AirspaceLocals;

struct FillVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @location(0) pos_local: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> FillVaryings {
    var out: FillVaryings;
    out.clip = world_rel_to_clip(locals.origin_rel + pos_local);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: FillVaryings) -> @location(0) vec4<f32> {
    return in.color;
}

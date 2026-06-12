//#include common.wgsl

// Point-feature symbols (airports, navaids, reporting points, obstacles,
// METAR dots): small flat-colored meshes built once per kind at unit scale
// on the CPU, instanced per feature and sized in logical pixels at draw
// time, so symbols stay screen-sized at any zoom.
//
// Mesh vertices are in unit symbol space (x right, y down — matching screen
// pixels). Instance anchors are world units relative to the dataset origin;
// the origin itself is camera-relative (`locals.origin_rel`, f64 subtraction
// on the CPU each frame).

struct SymbolLocals {
    origin_rel: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var<uniform> locals: SymbolLocals;

struct SymbolVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    // Per-vertex (mesh, buffer 0).
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Per-instance (buffer 1).
    @location(2) anchor_local: vec2<f32>,
    @location(3) offset_px: vec2<f32>,
    @location(4) size_px: f32,
    @location(5) color_mul: vec4<f32>,
    @location(6) rotation_rad: f32,
) -> SymbolVaryings {
    var out: SymbolVaryings;
    let anchor = world_rel_to_clip(locals.origin_rel + anchor_local);
    // Rotate the unit mesh clockwise on screen (y is down, so the standard
    // rotation matrix turns clockwise = true heading with north up). The
    // per-instance screen offset (METAR dots) is applied after and does NOT
    // rotate with the mesh.
    let s = sin(rotation_rad);
    let c = cos(rotation_rad);
    let rotated = vec2<f32>(pos.x * c - pos.y * s, pos.x * s + pos.y * c);
    let offset = logical_px_to_clip(offset_px + rotated * size_px);
    out.clip = vec4<f32>(anchor.xy + offset, anchor.zw);
    // Both colors are premultiplied; a componentwise product stays premultiplied.
    out.color = color * color_mul;
    return out;
}

@fragment
fn fs_main(in: SymbolVaryings) -> @location(0) vec4<f32> {
    return in.color;
}

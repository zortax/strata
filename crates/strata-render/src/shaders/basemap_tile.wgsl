//#include common.wgsl

// Basemap vector-tile mesh (fills + extruded strokes in one buffer).
//
// Vertices are in tile-local units (0..1 across the tile; the MVT buffer may
// reach slightly outside). The per-draw `BasemapTile` uniform places the tile
// in camera-relative world space — the f64 `tile_origin − camera_center`
// subtraction happens on the CPU, so f32 only ever holds small quantities.
//
// Strokes carry a screen-space extrusion normal (lyon stroke tessellation
// with `position_on_path`), so line widths are constant in logical pixels at
// any fractional zoom / overzoom. Fill vertices have normal == (0, 0).
//
// `alpha` drives the ~150 ms fade-in of freshly loaded tiles; colors are
// premultiplied, so all four components are scaled.

struct BasemapTile {
    // Tile origin in world units relative to the camera center.
    origin_rel: vec2<f32>,
    // Tile side length in world units (1 / 2^z of the source tile).
    scale: f32,
    // Fade-in multiplier in [0, 1].
    alpha: f32,
}

@group(1) @binding(0) var<uniform> tile: BasemapTile;

struct BasemapVertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) width_px: f32,
    @location(3) color: vec4<f32>,
}

struct BasemapVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(v: BasemapVertexIn) -> BasemapVaryings {
    var out: BasemapVaryings;
    let world_rel = tile.origin_rel + v.pos * tile.scale;
    let center = world_rel_to_clip(world_rel);
    let extrude = logical_px_to_clip(v.normal * (v.width_px * 0.5));
    out.clip = vec4<f32>(center.xy + extrude, center.zw);
    out.color = v.color * tile.alpha;
    return out;
}

@fragment
fn fs_main(in: BasemapVaryings) -> @location(0) vec4<f32> {
    return in.color;
}

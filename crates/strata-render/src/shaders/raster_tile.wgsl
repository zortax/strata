//#include common.wgsl

// Raster terrain tile quad. The texture is a two-channel hillshade
// (Rg8Unorm): r is the shade (0 = full shadow, 1 = full light), g is the
// DEM-coverage alpha (0 where the source has no data — sea squares, areas
// outside the ingested region). The fragment maps the shade into the theme
// palette: mix(shadow_tint, light_tint, value), scaled by coverage, the
// style opacity and the per-tile fade-in. Output is premultiplied alpha, so
// zero-coverage fragments are exactly vec4(0) — invisible, never a tinted
// block.
//
// Ancestor fallback reuses this pipeline: overzoomed tiles are drawn with a
// sub-window UV rect computed on the CPU.

struct TerrainStyle {
    shadow_tint: vec4<f32>,
    light_tint: vec4<f32>,
    opacity: f32,
}

@group(1) @binding(0) var<uniform> style: TerrainStyle;
@group(2) @binding(0) var tile_texture: texture_2d<f32>;
@group(2) @binding(1) var tile_sampler: sampler;

struct RasterVertex {
    @location(0) world_rel: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fade: f32,
}

struct RasterVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fade: f32,
}

@vertex
fn vs_main(v: RasterVertex) -> RasterVaryings {
    var out: RasterVaryings;
    out.clip = world_rel_to_clip(v.world_rel);
    out.uv = v.uv;
    out.fade = v.fade;
    return out;
}

@fragment
fn fs_main(in: RasterVaryings) -> @location(0) vec4<f32> {
    let texel = textureSample(tile_texture, tile_sampler, in.uv);
    let value = texel.r;
    let coverage = texel.g;
    let alpha = clamp(style.opacity * in.fade, 0.0, 1.0) * coverage;
    return mix(style.shadow_tint, style.light_tint, value) * alpha;
}

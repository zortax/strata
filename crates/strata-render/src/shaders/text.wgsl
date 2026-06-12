//#include common.wgsl

// Text glyphs: one instanced quad per glyph, sampling the R8Unorm coverage
// atlas filled by cosmic-text/swash rasterization (etagere allocation).
// Instances are fully screen-space (logical px, origin top-left, y-down);
// the CPU side projects world anchors and resolves collisions, so the
// vertex stage only has to scale into clip space. Colors arrive
// premultiplied; the pipeline blends with (One, OneMinusSrcAlpha).

@group(1) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(1) @binding(1) var glyph_sampler: sampler;

struct GlyphInstance {
    // Quad top-left in logical px (screen space, y-down).
    @location(1) pos_px: vec2<f32>,
    // Quad size in logical px.
    @location(2) size_px: vec2<f32>,
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
    // Premultiplied linear RGBA.
    @location(5) color: vec4<f32>,
}

struct GlyphVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

// Absolute logical-px screen position (origin top-left, y-down) -> clip space.
fn screen_px_to_clip(pos_px: vec2<f32>) -> vec2<f32> {
    let phys = pos_px * globals.scale_factor;
    return vec2<f32>(
        2.0 * phys.x / globals.viewport_size_px.x - 1.0,
        1.0 - 2.0 * phys.y / globals.viewport_size_px.y,
    );
}

@vertex
fn vs_main(
    // Unit quad corner in [0, 1]^2 (vertex buffer 0, triangle strip).
    @location(0) corner: vec2<f32>,
    instance: GlyphInstance,
) -> GlyphVaryings {
    var out: GlyphVaryings;
    let pos = instance.pos_px + corner * instance.size_px;
    out.clip = vec4<f32>(screen_px_to_clip(pos), 0.0, 1.0);
    out.uv = mix(instance.uv_min, instance.uv_max, corner);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: GlyphVaryings) -> @location(0) vec4<f32> {
    let coverage = textureSample(glyph_atlas, glyph_sampler, in.uv).r;
    return in.color * coverage;
}

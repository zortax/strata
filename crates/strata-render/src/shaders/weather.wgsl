//#include common.wgsl

// SIGMET overlay polygons: translucent fill with diagonal hatching computed
// in the fragment stage from framebuffer coordinates, so the stripes are
// screen-stable under pan/zoom.

struct WeatherLocals {
    origin_rel: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var<uniform> locals: WeatherLocals;

// Hatch geometry in logical px.
const HATCH_PERIOD_PX: f32 = 9.0;
const HATCH_LINE_PX: f32 = 2.0;
// Fill translucency between the hatch lines, as a fraction of the vertex color.
const HATCH_BASE: f32 = 0.18;

struct WeatherVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @location(0) pos_local: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> WeatherVaryings {
    var out: WeatherVaryings;
    out.clip = world_rel_to_clip(locals.origin_rel + pos_local);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: WeatherVaryings) -> @location(0) vec4<f32> {
    // `clip` in the fragment stage is the framebuffer position in physical px.
    let diag = (in.clip.x + in.clip.y) / globals.scale_factor;
    let t = diag - floor(diag / HATCH_PERIOD_PX) * HATCH_PERIOD_PX;
    let factor = select(HATCH_BASE, 1.0, t < HATCH_LINE_PX);
    // Premultiplied color: scaling all channels scales coverage uniformly.
    return in.color * factor;
}

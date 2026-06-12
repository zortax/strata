//#include common.wgsl

// Stroked lines (airspace borders, basemap roads/boundaries).
// Centerline vertices carry a screen-space extrusion normal so stroke width
// is zoom-independent. `dash` holds (dash_px, gap_px); (0, 0) = solid.
// `along_px` is the accumulated distance along the line in logical px.
// TODO(next-phase): proper dash phase + joins/caps from the tessellator.

struct LineVertex {
    @location(0) world_rel: vec2<f32>,
    @location(1) normal: vec2<f32>,      // unit extrusion direction, screen space
    @location(2) color: vec4<f32>,
    @location(3) width_px: f32,          // full stroke width, logical px
    @location(4) along_px: f32,
    @location(5) dash: vec2<f32>,
}

struct LineVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) along_px: f32,
    @location(2) dash: vec2<f32>,
}

@vertex
fn vs_main(v: LineVertex) -> LineVaryings {
    var out: LineVaryings;
    let center = world_rel_to_clip(v.world_rel);
    let extrude = logical_px_to_clip(v.normal * (v.width_px * 0.5));
    out.clip = vec4<f32>(center.xy + extrude, center.zw);
    out.color = v.color;
    out.along_px = v.along_px;
    out.dash = v.dash;
    return out;
}

@fragment
fn fs_main(in: LineVaryings) -> @location(0) vec4<f32> {
    let period = in.dash.x + in.dash.y;
    if (period > 0.0) {
        let t = in.along_px - floor(in.along_px / period) * period;
        if (t > in.dash.x) {
            discard;
        }
    }
    return in.color;
}

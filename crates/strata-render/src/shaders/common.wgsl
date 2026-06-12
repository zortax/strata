// Shared prelude, pulled into every pipeline shader via `//#include common.wgsl`.
//
// Coordinate convention: vertex positions arrive in world units RELATIVE TO
// THE CAMERA CENTER. The f64 subtraction (world − camera_center) happens on
// the CPU per tile origin / upload; f32 is only ever asked to hold small
// relative quantities, so deep zoom stays precise.

struct Globals {
    // Camera-relative world units -> clip space. y is negated here:
    // world y grows southward (screen down), clip y grows up.
    camera_to_clip: vec2<f32>,
    // Render target size in physical pixels.
    viewport_size_px: vec2<f32>,
    // Continuous camera zoom.
    zoom: f32,
    // Physical pixels per logical pixel.
    scale_factor: f32,
    pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

// Camera-relative world position -> clip space.
fn world_rel_to_clip(world_rel: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(world_rel * globals.camera_to_clip, 0.0, 1.0);
}

// A physical-pixel offset expressed in clip-space units (for screen-space
// extrusion: line widths, symbol quads, text glyphs).
fn px_to_clip(offset_px: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        2.0 * offset_px.x / globals.viewport_size_px.x,
        -2.0 * offset_px.y / globals.viewport_size_px.y,
    );
}

// Logical-pixel offset -> clip-space units.
fn logical_px_to_clip(offset_px: vec2<f32>) -> vec2<f32> {
    return px_to_clip(offset_px * globals.scale_factor);
}

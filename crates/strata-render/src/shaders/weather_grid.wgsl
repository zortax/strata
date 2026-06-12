//#include common.wgsl

// Gridded weather overlay: one quad over the frame extent, two Rg16Float
// frame textures (r = value · coverage, g = coverage; NaN source points
// carry coverage 0) blended by the temporal fraction, then colormapped by a
// piecewise-linear stop ramp from the uniform.
//
// Mercator mapping: latitude is nonlinear in Web-Mercator, so a single
// linearly-interpolated V coordinate would misplace rows. Instead the
// fragment stage recovers the geographic latitude from the interpolated
// world position (inverse Mercator: lat = atan(sinh((0.5 − y)·2π))) and
// derives each texture's V from it. This was chosen over a row-subdivided
// mesh because it is exact at every zoom (no piecewise-linear residual)
// and keeps the geometry at one quad. Precision: the varying is the
// quad-relative position (small values, exact in f32); only the absolute
// north-edge world y (~0.33 for Germany, f32 error ≈ 2e-8 world units
// ≈ 5 m on the ground) enters the latitude math — far below one grid cell
// (ICON-D2: ~2.2 km). The CPU mirror of this math lives in
// `layers/weather_grid/grid.rs` and is tested at three latitudes.
//
// Each texture carries its own lat/lon window (`grid_a` / `grid_b`): the
// two blended frames may come from different sources (radar past vs model
// forecast) with different extents/resolutions. Outside a texture's window
// its sample contributes zero coverage, so mixed extents fade smoothly.

struct GridLocals {
    // Quad north-west corner, world units relative to the camera center.
    origin_rel: vec2<f32>,
    // Quad size in world units (x east, y south).
    size_world: vec2<f32>,
    // Absolute world coordinates of the quad's north-west corner.
    nw_abs: vec2<f32>,
    // Temporal blend fraction between texture a (0.0) and b (1.0).
    frac: f32,
    // Screen-space hatch strength (0 = none; thunderstorm overlay only).
    hatch: f32,
    // Per-texture grid window: (lat_min, 1/lat_span, lon_min, 1/lon_span).
    grid_a: vec4<f32>,
    // Per-texture grid points: (ni, nj, unused, unused).
    dims_a: vec4<f32>,
    grid_b: vec4<f32>,
    dims_b: vec4<f32>,
    // Colormap: `stop_count` ascending stops; values below the first stop
    // clamp to its (transparent) color. Mirrors map_theme::Colormap.
    stop_count: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    stop_pos: array<vec4<f32>, 2>,
    stop_color: array<vec4<f32>, 8>,
}

@group(1) @binding(0) var<uniform> locals: GridLocals;
@group(2) @binding(0) var frame_a: texture_2d<f32>;
@group(2) @binding(1) var frame_b: texture_2d<f32>;
@group(2) @binding(2) var frame_sampler: sampler;

const TAU: f32 = 6.283185307179586;
// Coverage below this renders nothing: keeps bilinear edges of the data
// domain (GRIB bitmap holes, grid borders) from smearing half-valid texels.
const MIN_COVERAGE: f32 = 0.05;
// Hatch geometry in logical px (same diagonal language as the SIGMET fill).
const HATCH_PERIOD_PX: f32 = 9.0;
const HATCH_LINE_PX: f32 = 2.0;
// How much the hatch dims the fill between the stripe lines at strength 1.
const HATCH_DIM: f32 = 0.45;

struct GridVaryings {
    @builtin(position) clip: vec4<f32>,
    // Position on the quad in [0,1]^2 (x: west→east, y: north→south).
    @location(0) pos01: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> GridVaryings {
    // One quad, two triangles; no vertex buffer (local var: dynamically
    // indexable).
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    var out: GridVaryings;
    let pos01 = quad[index];
    out.clip = world_rel_to_clip(locals.origin_rel + pos01 * locals.size_world);
    out.pos01 = pos01;
    return out;
}

// Window tolerance in grid-span fractions: f32 rounding must not mask the
// exact edge rows/columns of a frame that spans the whole quad.
const WINDOW_EPS: f32 = 1e-4;

// Half-texel UV for a (lat, lon) inside a grid window; z = 1 inside, else 0.
// Row 0 of the texture is the southernmost row, so v grows with latitude
// fraction directly (the quad's y axis points south, which only affects
// pos01, not this mapping).
fn grid_uv(lat: f32, lon: f32, grid: vec4<f32>, dims: vec4<f32>) -> vec3<f32> {
    let t = vec2<f32>((lon - grid.z) * grid.w, (lat - grid.x) * grid.y);
    let inside = t.x >= -WINDOW_EPS && t.x <= 1.0 + WINDOW_EPS
        && t.y >= -WINDOW_EPS && t.y <= 1.0 + WINDOW_EPS;
    let tc = clamp(t, vec2<f32>(0.0), vec2<f32>(1.0));
    let uv = (vec2<f32>(0.5) + tc * (dims.xy - vec2<f32>(1.0))) / dims.xy;
    return vec3<f32>(uv, select(0.0, 1.0, inside));
}

fn stop_pos(i: u32) -> f32 {
    return locals.stop_pos[i / 4u][i % 4u];
}

// Piecewise-linear colormap — CPU mirror: map_theme::Colormap::sample.
fn colormap_sample(value: f32) -> vec4<f32> {
    let n = locals.stop_count;
    if (n == 0u) {
        return vec4<f32>(0.0);
    }
    if (value <= stop_pos(0u)) {
        return locals.stop_color[0];
    }
    for (var i = 1u; i < n; i = i + 1u) {
        let hi = stop_pos(i);
        if (value <= hi) {
            let lo = stop_pos(i - 1u);
            let t = clamp((value - lo) / max(hi - lo, 1e-6), 0.0, 1.0);
            return mix(locals.stop_color[i - 1u], locals.stop_color[i], t);
        }
    }
    return locals.stop_color[n - 1u];
}

@fragment
fn fs_main(in: GridVaryings) -> @location(0) vec4<f32> {
    // Geographic position of this fragment (see header for precision).
    let world_abs = locals.nw_abs + in.pos01 * locals.size_world;
    let lon = (world_abs.x - 0.5) * 360.0;
    let lat = degrees(atan(sinh((0.5 - world_abs.y) * TAU)));

    let uv_a = grid_uv(lat, lon, locals.grid_a, locals.dims_a);
    let uv_b = grid_uv(lat, lon, locals.grid_b, locals.dims_b);
    // Samples first (uniform control flow), masked after.
    let texel_a = textureSample(frame_a, frame_sampler, uv_a.xy).rg * uv_a.z;
    let texel_b = textureSample(frame_b, frame_sampler, uv_b.xy).rg * uv_b.z;

    // Temporal blend of the premultiplied (value·coverage, coverage) pairs.
    let blended = mix(texel_a, texel_b, locals.frac);
    let coverage = blended.y;
    if (coverage < MIN_COVERAGE) {
        return vec4<f32>(0.0);
    }
    var color = colormap_sample(blended.x / coverage);
    // Fade out toward no-data regions (premultiplied: scales all channels).
    color = color * clamp(coverage, 0.0, 1.0);

    // Subtle screen-stable diagonal hatch (thunderstorm overlay): dim the
    // fill between stripe lines, keep it full on them.
    let diag = (in.clip.x + in.clip.y) / globals.scale_factor;
    let t = diag - floor(diag / HATCH_PERIOD_PX) * HATCH_PERIOD_PX;
    let off_line = select(1.0, 0.0, t < HATCH_LINE_PX);
    let dim = 1.0 - locals.hatch * HATCH_DIM * off_line;
    return color * dim;
}

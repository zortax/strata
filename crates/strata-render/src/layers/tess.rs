//! Worker-side lyon tessellation for the aero layers: polygon fills (with
//! holes) and ring strokes carrying an arc-length attribute for shader-side
//! dashing.
//!
//! ## Precision
//!
//! Vertices are stored in world units **relative to a dataset origin** (f64
//! subtraction here, on the worker), so f32 only holds small quantities; the
//! per-frame `origin − camera_center` completes the camera-relative shader
//! convention. Lyon itself runs on the local coordinates scaled by
//! [`TESS_SCALE`] — Germany-sized geometry is ~0.03 world units across, far
//! below lyon's comfortable f32 range, so we tessellate at ×65536 and scale
//! positions/arc-lengths back down.

use crate::geo::{LatLon, world_from_lat_lon};

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex as LyonFillVertex,
    StrokeOptions, StrokeTessellator, StrokeVertex as LyonStrokeVertex, VertexBuffers,
};

/// Uniform scale applied to local coordinates before tessellation.
pub const TESS_SCALE: f32 = 65536.0;

/// Vertex of a tessellated fill (`fill_airspace.wgsl` / `weather.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FillVertex {
    /// World units relative to the dataset origin.
    pub pos: [f32; 2],
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
}

impl FillVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Vertex of a stroked centerline (`line_dash.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    /// Centerline position, world units relative to the dataset origin.
    pub pos: [f32; 2],
    /// Extrusion direction (lyon stroke normal; unit-ish, longer at miters).
    pub normal: [f32; 2],
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
    /// Full stroke width in logical px.
    pub width_px: f32,
    /// Arc length along the ring in world units.
    pub along_world: f32,
    /// Dash pattern (on, off) in logical px; (0, 0) = solid.
    pub dash_px: [f32; 2],
}

impl LineVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x4,
        3 => Float32,
        4 => Float32,
        5 => Float32x2,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// CPU-side accumulated fill geometry.
#[derive(Debug, Clone, Default)]
pub struct FillMesh {
    pub vertices: Vec<FillVertex>,
    pub indices: Vec<u32>,
}

/// CPU-side accumulated stroke geometry.
#[derive(Debug, Clone, Default)]
pub struct LineMesh {
    pub vertices: Vec<LineVertex>,
    pub indices: Vec<u32>,
}

/// Stroke styling carried into the vertices.
#[derive(Debug, Clone, Copy)]
pub struct StrokeStyleSpec {
    pub color: [f32; 4],
    pub width_px: f32,
    pub dash_px: Option<(f32, f32)>,
}

/// `[lon, lat]` degree ring → normalized Web-Mercator world coordinates.
pub fn ring_to_world(ring: &[[f64; 2]]) -> Vec<DVec2> {
    ring.iter()
        .map(|&[lon, lat]| world_from_lat_lon(LatLon::new(lat, lon)))
        .collect()
}

/// Tessellate `exterior` minus `holes` (world coordinates) into `out`,
/// relative to `origin`. Errors are logged and the polygon skipped — one bad
/// geometry must not take down the whole dataset.
pub fn tessellate_fill(
    exterior: &[DVec2],
    holes: &[Vec<DVec2>],
    origin: DVec2,
    color: [f32; 4],
    out: &mut FillMesh,
) {
    if exterior.len() < 3 {
        return;
    }
    let mut builder = Path::builder();
    for ring in std::iter::once(exterior).chain(holes.iter().map(Vec::as_slice)) {
        if ring.len() < 3 {
            continue;
        }
        let mut points = ring.iter().map(|&w| to_local(w, origin));
        // `ring.len() >= 3` guarantees a first point.
        let Some(first) = points.next() else {
            continue;
        };
        builder.begin(first);
        for p in points {
            builder.line_to(p);
        }
        builder.close();
    }
    let path = builder.build();

    let mut buffers: VertexBuffers<FillVertex, u32> = VertexBuffers::new();
    let options = FillOptions::default().with_fill_rule(FillRule::EvenOdd);
    let result = FillTessellator::new().tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |v: LyonFillVertex| FillVertex {
            pos: from_local(v.position()),
            color,
        }),
    );
    match result {
        Ok(_) => append_fill(out, buffers),
        Err(e) => tracing::warn!(error = %e, "fill tessellation failed; polygon skipped"),
    }
}

/// Stroke a closed ring (world coordinates) into `out`, relative to
/// `origin`, carrying arc length for shader-side dashing.
pub fn tessellate_ring_stroke(
    ring: &[DVec2],
    origin: DVec2,
    style: StrokeStyleSpec,
    out: &mut LineMesh,
) {
    if ring.len() < 3 {
        return;
    }
    let mut builder = Path::builder();
    let mut points = ring.iter().map(|&w| to_local(w, origin));
    let Some(first) = points.next() else {
        return;
    };
    builder.begin(first);
    for p in points {
        builder.line_to(p);
    }
    builder.close();
    let path = builder.build();

    let dash_px = style.dash_px.map_or([0.0, 0.0], |(on, off)| [on, off]);
    let mut buffers: VertexBuffers<LineVertex, u32> = VertexBuffers::new();
    // Width 1.0 is a placeholder: the shader extrudes `normal` by the real
    // width in logical px, so the tessellated width never shows.
    let options = StrokeOptions::default().with_line_width(1.0);
    let result = StrokeTessellator::new().tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |v: LyonStrokeVertex| LineVertex {
            pos: from_local(v.position_on_path()),
            normal: [v.normal().x, v.normal().y],
            color: style.color,
            width_px: style.width_px,
            along_world: v.advancement() / TESS_SCALE,
            dash_px,
        }),
    );
    match result {
        Ok(_) => append_line(out, buffers),
        Err(e) => tracing::warn!(error = %e, "stroke tessellation failed; ring skipped"),
    }
}

fn to_local(world: DVec2, origin: DVec2) -> lyon::math::Point {
    let local = world - origin; // f64 subtraction keeps deep-zoom precision
    point(local.x as f32 * TESS_SCALE, local.y as f32 * TESS_SCALE)
}

fn from_local(p: lyon::math::Point) -> [f32; 2] {
    [p.x / TESS_SCALE, p.y / TESS_SCALE]
}

fn append_fill(out: &mut FillMesh, buffers: VertexBuffers<FillVertex, u32>) {
    let base = out.vertices.len() as u32;
    out.vertices.extend(buffers.vertices);
    out.indices.extend(buffers.indices.iter().map(|i| i + base));
}

fn append_line(out: &mut LineMesh, buffers: VertexBuffers<LineVertex, u32>) {
    let base = out.vertices.len() as u32;
    out.vertices.extend(buffers.vertices);
    out.indices.extend(buffers.indices.iter().map(|i| i + base));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(center: DVec2, half: f64) -> Vec<DVec2> {
        vec![
            center + DVec2::new(-half, -half),
            center + DVec2::new(half, -half),
            center + DVec2::new(half, half),
            center + DVec2::new(-half, half),
        ]
    }

    #[test]
    fn fill_tessellation_produces_triangles_near_origin() {
        let origin = DVec2::new(0.53, 0.34); // ~Germany in world space
        let exterior = square(origin, 5e-4);
        let mut mesh = FillMesh::default();
        tessellate_fill(&exterior, &[], origin, [0.1, 0.2, 0.3, 0.4], &mut mesh);
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        for &i in &mesh.indices {
            assert!((i as usize) < mesh.vertices.len());
        }
        for v in &mesh.vertices {
            assert!(v.pos[0].abs() <= 5.1e-4, "local coords stay small");
            assert!(v.pos[1].abs() <= 5.1e-4);
            assert_eq!(v.color, [0.1, 0.2, 0.3, 0.4]);
        }
    }

    #[test]
    fn fill_with_hole_keeps_the_hole_empty() {
        let origin = DVec2::new(0.5, 0.5);
        let exterior = square(origin, 4e-4);
        let holes = vec![square(origin, 2e-4)];
        let mut mesh = FillMesh::default();
        tessellate_fill(&exterior, &holes, origin, [1.0; 4], &mut mesh);
        // No triangle's centroid may fall inside the hole.
        for tri in mesh.indices.chunks_exact(3) {
            let c = tri.iter().fold([0.0f32; 2], |acc, &i| {
                let p = mesh.vertices[i as usize].pos;
                [acc[0] + p[0] / 3.0, acc[1] + p[1] / 3.0]
            });
            let inside_hole = c[0].abs() < 1.9e-4 && c[1].abs() < 1.9e-4;
            assert!(!inside_hole, "triangle centroid {c:?} inside the hole");
        }
    }

    #[test]
    fn ring_stroke_carries_arc_length_and_style() {
        let origin = DVec2::new(0.5, 0.5);
        let ring = square(origin, 1e-3);
        let mut mesh = LineMesh::default();
        let style = StrokeStyleSpec {
            color: [0.5, 0.0, 0.0, 0.5],
            width_px: 2.0,
            dash_px: Some((6.0, 3.0)),
        };
        tessellate_ring_stroke(&ring, origin, style, &mut mesh);
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        let perimeter = 8.0e-3; // 4 sides × 2e-3
        let max_along = mesh
            .vertices
            .iter()
            .map(|v| v.along_world)
            .fold(0.0f32, f32::max);
        assert!(
            (max_along - perimeter as f32).abs() < 0.2 * perimeter as f32,
            "arc length {max_along} should approximate the perimeter {perimeter}"
        );
        for v in &mesh.vertices {
            assert_eq!(v.width_px, 2.0);
            assert_eq!(v.dash_px, [6.0, 3.0]);
            let n = (v.normal[0].powi(2) + v.normal[1].powi(2)).sqrt();
            assert!(n > 0.5 && n < 2.0, "normal length {n} out of miter range");
        }
    }

    #[test]
    fn degenerate_rings_are_skipped() {
        let mut fill = FillMesh::default();
        tessellate_fill(&[DVec2::ZERO, DVec2::ONE], &[], DVec2::ZERO, [1.0; 4], &mut fill);
        assert!(fill.vertices.is_empty());
        let mut line = LineMesh::default();
        let style = StrokeStyleSpec {
            color: [1.0; 4],
            width_px: 1.0,
            dash_px: None,
        };
        tessellate_ring_stroke(&[DVec2::ZERO, DVec2::ONE], DVec2::ZERO, style, &mut line);
        assert!(line.vertices.is_empty());
    }

    #[test]
    fn ring_to_world_maps_lon_lat_pairs() {
        let world = ring_to_world(&[[10.4515, 51.1657]]);
        assert_eq!(world.len(), 1);
        assert!((world[0].x - (10.4515 / 360.0 + 0.5)).abs() < 1e-12);
    }
}

//! Lyon tessellation of decoded MVT features into one interleaved
//! vertex/index buffer per tile. Runs on the worker pool only.
//!
//! Coordinates are tile-local (0..1 across the tile; MVT buffer geometry may
//! reach slightly outside — the layer clips with a per-tile scissor rect).
//! Strokes keep their centerline position and carry lyon's extrusion normal
//! so `basemap_tile.wgsl` can extrude in screen space (constant logical-px
//! width at any fractional zoom / overzoom).

use crate::basemap::style::StrokeStyle;
use crate::camera::TILE_SIZE_PX;

use bytemuck::{Pod, Zeroable};
use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, FillVertexConstructor,
    LineJoin, StrokeOptions, StrokeTessellator, StrokeVertex, StrokeVertexConstructor,
    VertexBuffers,
};

/// Tessellation tolerance in tile units (~0.5 px at the nominal zoom).
const TOLERANCE: f32 = 0.002;

/// One basemap mesh vertex. Fills have `normal == (0, 0)`, `width_px == 0`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BasemapVertex {
    /// Tile-local position (0..1 across the tile).
    pub pos: [f32; 2],
    /// Screen-space extrusion direction (lyon stroke normal; miter-scaled).
    pub normal: [f32; 2],
    /// Full stroke width in logical px.
    pub width_px: f32,
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
}

impl BasemapVertex {
    pub const STRIDE: wgpu::BufferAddress = std::mem::size_of::<BasemapVertex>() as u64;

    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32,
        3 => Float32x4,
    ];
}

/// CPU-side tile mesh, ready for GPU upload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<BasemapVertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

struct FillCtor {
    color: [f32; 4],
}

impl FillVertexConstructor<BasemapVertex> for FillCtor {
    fn new_vertex(&mut self, vertex: FillVertex) -> BasemapVertex {
        let p = vertex.position();
        BasemapVertex {
            pos: [p.x, p.y],
            normal: [0.0, 0.0],
            width_px: 0.0,
            color: self.color,
        }
    }
}

struct StrokeCtor {
    color: [f32; 4],
    width_px: f32,
}

impl StrokeVertexConstructor<BasemapVertex> for StrokeCtor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> BasemapVertex {
        let p = vertex.position_on_path();
        let n = vertex.normal();
        BasemapVertex {
            pos: [p.x, p.y],
            normal: [n.x, n.y],
            width_px: self.width_px,
            color: self.color,
        }
    }
}

/// Accumulates tessellated features into one tile mesh.
pub struct MeshBuilder {
    buffers: VertexBuffers<BasemapVertex, u32>,
    fill: FillTessellator,
    stroke: StrokeTessellator,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self {
            buffers: VertexBuffers::new(),
            fill: FillTessellator::new(),
            stroke: StrokeTessellator::new(),
        }
    }

    /// Tessellate a polygon (all rings of all sub-polygons as one non-zero
    /// fill, which resolves MVT winding-encoded holes).
    pub fn add_fill(&mut self, rings: &[Vec<[f32; 2]>], color: [f32; 4]) {
        let mut builder = Path::builder();
        let mut any = false;
        for ring in rings {
            if ring.len() < 3 {
                continue;
            }
            builder.begin(point(ring[0][0], ring[0][1]));
            for p in &ring[1..] {
                builder.line_to(point(p[0], p[1]));
            }
            builder.end(true);
            any = true;
        }
        if !any {
            return;
        }
        let path = builder.build();
        let options = FillOptions::tolerance(TOLERANCE).with_fill_rule(FillRule::NonZero);
        let result = self.fill.tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut self.buffers, FillCtor { color }),
        );
        if let Err(e) = result {
            tracing::warn!("basemap fill tessellation failed: {e:?}");
        }
    }

    /// Tessellate polylines as screen-extruded strokes. Dashes are split
    /// geometrically (dash lengths are logical px at the nominal tile zoom).
    pub fn add_stroke(&mut self, paths: &[Vec<[f32; 2]>], style: &StrokeStyle) {
        let dashed_storage;
        let paths: &[Vec<[f32; 2]>] = match style.dash {
            Some([dash_px, gap_px]) if dash_px > 0.0 && gap_px > 0.0 => {
                let to_tile = 1.0 / TILE_SIZE_PX as f32;
                dashed_storage = paths
                    .iter()
                    .flat_map(|path| dash_segments(path, dash_px * to_tile, gap_px * to_tile))
                    .collect::<Vec<_>>();
                &dashed_storage
            }
            _ => paths,
        };

        let mut builder = Path::builder();
        let mut any = false;
        for path in paths {
            if path.len() < 2 {
                continue;
            }
            builder.begin(point(path[0][0], path[0][1]));
            for p in &path[1..] {
                builder.line_to(point(p[0], p[1]));
            }
            builder.end(false);
            any = true;
        }
        if !any {
            return;
        }
        let path = builder.build();
        // Unit line width: the real width is applied in the shader along the
        // (miter-scaled) normal, in logical pixels.
        let options = StrokeOptions::tolerance(TOLERANCE)
            .with_line_width(1.0)
            .with_line_join(LineJoin::Miter);
        let result = self.stroke.tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(
                &mut self.buffers,
                StrokeCtor {
                    color: style.color,
                    width_px: style.width_px,
                },
            ),
        );
        if let Err(e) = result {
            tracing::warn!("basemap stroke tessellation failed: {e:?}");
        }
    }

    pub fn finish(self) -> MeshData {
        MeshData {
            vertices: self.buffers.vertices,
            indices: self.buffers.indices,
        }
    }
}

impl Default for MeshBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Split a polyline into "on" segments of a dash pattern (lengths in tile
/// units). Each returned polyline is one dash.
fn dash_segments(path: &[[f32; 2]], dash_len: f32, gap_len: f32) -> Vec<Vec<[f32; 2]>> {
    let mut out = Vec::new();
    if path.len() < 2 {
        return out;
    }
    let mut on = true;
    let mut remaining = dash_len;
    let mut current: Vec<[f32; 2]> = vec![path[0]];
    for pair in path.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len <= f32::EPSILON {
            continue;
        }
        let mut t0 = 0.0_f32;
        while t0 < seg_len {
            let step = (seg_len - t0).min(remaining);
            let t1 = t0 + step;
            let p1 = [a[0] + dx * t1 / seg_len, a[1] + dy * t1 / seg_len];
            if on {
                current.push(p1);
            }
            remaining -= step;
            if remaining <= f32::EPSILON {
                if on {
                    if current.len() >= 2 {
                        out.push(std::mem::take(&mut current));
                    } else {
                        current.clear();
                    }
                    remaining = gap_len;
                } else {
                    current = vec![p1];
                    remaining = dash_len;
                }
                on = !on;
            }
            t0 = t1;
        }
    }
    if on && current.len() >= 2 {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::basemap::style::StrokeStyle;

    #[test]
    fn fill_with_hole_produces_triangles_avoiding_the_hole() {
        let mut builder = MeshBuilder::new();
        // Outer square (CW in y-down screen space) with an inner square wound
        // the opposite way — non-zero rule must cut the hole out.
        let outer = vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9], [0.1, 0.1]];
        let inner = vec![[0.4, 0.4], [0.4, 0.6], [0.6, 0.6], [0.6, 0.4], [0.4, 0.4]];
        builder.add_fill(&[outer, inner], [1.0, 0.0, 0.0, 1.0]);
        let mesh = builder.finish();
        assert!(!mesh.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        // No triangle centroid may land inside the hole.
        for tri in mesh.indices.chunks(3) {
            let c = tri.iter().fold([0.0_f32; 2], |acc, &i| {
                let p = mesh.vertices[i as usize].pos;
                [acc[0] + p[0] / 3.0, acc[1] + p[1] / 3.0]
            });
            let in_hole = c[0] > 0.42 && c[0] < 0.58 && c[1] > 0.42 && c[1] < 0.58;
            assert!(!in_hole, "triangle centroid {c:?} inside the hole");
        }
    }

    #[test]
    fn stroke_vertices_carry_normals_and_width() {
        let mut builder = MeshBuilder::new();
        builder.add_stroke(
            &[vec![[0.0, 0.5], [1.0, 0.5]]],
            &StrokeStyle {
                color: [0.0, 1.0, 0.0, 1.0],
                width_px: 3.0,
                dash: None,
            },
        );
        let mesh = builder.finish();
        assert!(!mesh.is_empty());
        for v in &mesh.vertices {
            assert_eq!(v.width_px, 3.0);
            let n = (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1]).sqrt();
            assert!(n > 0.5, "stroke normal must be roughly unit, got {n}");
            // Horizontal line: normals extrude vertically.
            assert!(v.normal[1].abs() > 0.9);
        }
    }

    #[test]
    fn dashes_split_into_alternating_segments() {
        let segments = dash_segments(&[[0.0, 0.0], [1.0, 0.0]], 0.2, 0.1);
        // 1.0 / (0.2 + 0.3) → dashes at [0,.2], [.3,.5], [.6,.8], [.9,1.0]
        assert_eq!(segments.len(), 4);
        let total: f32 = segments
            .iter()
            .map(|s| (s.last().unwrap()[0] - s.first().unwrap()[0]).abs())
            .sum();
        assert!((total - 0.7).abs() < 1e-4, "on-coverage was {total}");
    }

    #[test]
    fn degenerate_inputs_are_skipped() {
        let mut builder = MeshBuilder::new();
        builder.add_fill(&[vec![[0.0, 0.0], [1.0, 1.0]]], [1.0; 4]); // < 3 pts
        builder.add_stroke(
            &[vec![[0.5, 0.5]]], // < 2 pts
            &StrokeStyle {
                color: [1.0; 4],
                width_px: 1.0,
                dash: None,
            },
        );
        assert!(builder.finish().is_empty());
    }
}

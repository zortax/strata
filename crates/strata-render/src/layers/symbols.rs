//! Procedural symbol meshes for point features: one small flat-colored mesh
//! per kind, built once at unit scale (coordinates ≈ [-1, 1], x right,
//! y **down** to match screen pixels) and instanced/scaled at draw time.
//! Also home of the per-kind declutter (min zoom) and draw-order tables.

use crate::features::PointKind;
use crate::layer::LayerId;
use crate::map_theme::SymbolTheme;

use bytemuck::{Pod, Zeroable};

use std::ops::Range;

/// Mesh selector — [`PointKind`] with the METAR category color erased
/// (weather stations share one mesh and tint per instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolMeshKey {
    AirportIntl,
    AirportRegional,
    Airfield,
    GliderSite,
    Heliport,
    UltraLight,
    Vor,
    VorDme,
    Dme,
    Ndb,
    Tacan,
    ReportingPointMandatory,
    ReportingPointVoluntary,
    Obstacle,
    WeatherStation,
}

impl SymbolMeshKey {
    pub const COUNT: usize = 15;

    pub const ALL: [Self; Self::COUNT] = [
        Self::AirportIntl,
        Self::AirportRegional,
        Self::Airfield,
        Self::GliderSite,
        Self::Heliport,
        Self::UltraLight,
        Self::Vor,
        Self::VorDme,
        Self::Dme,
        Self::Ndb,
        Self::Tacan,
        Self::ReportingPointMandatory,
        Self::ReportingPointVoluntary,
        Self::Obstacle,
        Self::WeatherStation,
    ];

    pub fn from_kind(kind: PointKind) -> Self {
        match kind {
            PointKind::AirportIntl => Self::AirportIntl,
            PointKind::AirportRegional => Self::AirportRegional,
            PointKind::Airfield => Self::Airfield,
            PointKind::GliderSite => Self::GliderSite,
            PointKind::Heliport => Self::Heliport,
            PointKind::UltraLight => Self::UltraLight,
            PointKind::Vor => Self::Vor,
            PointKind::VorDme => Self::VorDme,
            PointKind::Dme => Self::Dme,
            PointKind::Ndb => Self::Ndb,
            PointKind::Tacan => Self::Tacan,
            PointKind::ReportingPointMandatory => Self::ReportingPointMandatory,
            PointKind::ReportingPointVoluntary => Self::ReportingPointVoluntary,
            PointKind::Obstacle => Self::Obstacle,
            PointKind::WeatherStation(_) => Self::WeatherStation,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::AirportIntl => 0,
            Self::AirportRegional => 1,
            Self::Airfield => 2,
            Self::GliderSite => 3,
            Self::Heliport => 4,
            Self::UltraLight => 5,
            Self::Vor => 6,
            Self::VorDme => 7,
            Self::Dme => 8,
            Self::Ndb => 9,
            Self::Tacan => 10,
            Self::ReportingPointMandatory => 11,
            Self::ReportingPointVoluntary => 12,
            Self::Obstacle => 13,
            Self::WeatherStation => 14,
        }
    }

    /// The toggleable layer category whose z-slot this symbol draws in.
    pub fn category(self) -> LayerId {
        match self {
            Self::AirportIntl
            | Self::AirportRegional
            | Self::Airfield
            | Self::GliderSite
            | Self::Heliport
            | Self::UltraLight => LayerId::Airports,
            Self::Vor | Self::VorDme | Self::Dme | Self::Ndb | Self::Tacan => LayerId::Navaids,
            Self::ReportingPointMandatory | Self::ReportingPointVoluntary => {
                LayerId::ReportingPoints
            }
            Self::Obstacle => LayerId::Obstacles,
            Self::WeatherStation => LayerId::Weather,
        }
    }

    /// Declutter gate: the symbol is hidden below this camera zoom.
    /// Obstacles and reporting points only appear at high zoom.
    pub fn min_zoom(self) -> f32 {
        match self {
            Self::AirportIntl => 5.0,
            Self::AirportRegional => 6.5,
            Self::Airfield => 8.0,
            Self::GliderSite => 8.5,
            Self::Heliport | Self::UltraLight => 9.0,
            Self::Vor | Self::VorDme => 6.5,
            Self::Tacan => 7.0,
            Self::Dme | Self::Ndb => 7.5,
            Self::ReportingPointMandatory | Self::ReportingPointVoluntary => 10.0,
            Self::Obstacle => 10.5,
            Self::WeatherStation => 6.0,
        }
    }

    /// Screen size in logical px that the unit mesh is scaled by.
    pub fn size_px(self) -> f32 {
        match self {
            Self::AirportIntl => 9.0,
            Self::AirportRegional => 7.0,
            Self::Airfield => 5.5,
            Self::GliderSite => 6.0,
            Self::Heliport => 6.0,
            Self::UltraLight => 5.0,
            Self::Vor => 7.0,
            Self::VorDme => 8.0,
            Self::Dme => 6.0,
            Self::Ndb => 7.0,
            Self::Tacan => 8.0,
            Self::ReportingPointMandatory | Self::ReportingPointVoluntary => 5.0,
            Self::Obstacle => 7.0,
            Self::WeatherStation => 4.0,
        }
    }

    /// Draw order within the point layer (low draws first). Weather dots sit
    /// above airports by design.
    pub fn draw_order(self) -> u8 {
        match self.category() {
            LayerId::Obstacles => 0,
            LayerId::Navaids => 1,
            LayerId::ReportingPoints => 2,
            LayerId::Airports => 3,
            _ => 4, // weather stations on top
        }
    }
}

/// Flat-colored mesh vertex in unit symbol space.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SymbolVertex {
    pub pos: [f32; 2],
    /// Premultiplied linear RGBA; multiplied by the instance tint.
    pub color: [f32; 4],
}

impl SymbolVertex {
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

/// Per-feature symbol instance (`symbol.wgsl` buffer 1).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SymbolInstance {
    /// World units relative to the dataset origin.
    pub anchor_local: [f32; 2],
    /// Logical-px offset from the anchor (weather dots sit right-top).
    /// Applied *after* rotation — it never rotates with the mesh.
    pub offset_px: [f32; 2],
    /// Scale of the unit mesh in logical px.
    pub size_px: f32,
    /// Mesh rotation in radians, clockwise on screen (map north is up, so
    /// this equals true heading for north-south canonical meshes). 0 = none.
    pub rotation_rad: f32,
    /// Premultiplied tint multiplied into the mesh colors.
    pub color_mul: [f32; 4],
}

impl SymbolInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 16,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 20,
            shader_location: 6,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 24,
            shader_location: 5,
        },
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Rotate a unit-mesh position clockwise on screen (x right, y **down**) by
/// `rotation_rad` — the CPU mirror of the vertex rotation in `symbol.wgsl`.
/// With north-up screen space, a north-pointing vertex `(0, -1)` rotated by
/// 90° (east) lands at `(1, 0)`.
pub fn rotate_screen_pos(pos: [f32; 2], rotation_rad: f32) -> [f32; 2] {
    let (sin, cos) = rotation_rad.sin_cos();
    [pos[0] * cos - pos[1] * sin, pos[0] * sin + pos[1] * cos]
}

/// A built unit-scale mesh.
#[derive(Debug, Clone, Default)]
pub struct SymbolMesh {
    pub vertices: Vec<SymbolVertex>,
    pub indices: Vec<u32>,
}

/// All per-kind meshes concatenated for a single vertex/index buffer pair.
pub struct SymbolAtlas {
    pub vertices: Vec<SymbolVertex>,
    pub indices: Vec<u32>,
    ranges: [MeshRange; SymbolMeshKey::COUNT],
}

/// Draw range of one mesh inside the atlas.
#[derive(Debug, Clone)]
pub struct MeshRange {
    pub indices: Range<u32>,
    pub base_vertex: i32,
}

impl SymbolAtlas {
    /// Build every kind's mesh with the theme's symbol colors.
    pub fn build(theme: &SymbolTheme) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut ranges: Vec<MeshRange> = Vec::with_capacity(SymbolMeshKey::COUNT);
        for key in SymbolMeshKey::ALL {
            let mesh = build_mesh(theme, key);
            let start = indices.len() as u32;
            let base_vertex = vertices.len() as i32;
            indices.extend(&mesh.indices);
            vertices.extend(&mesh.vertices);
            ranges.push(MeshRange {
                indices: start..indices.len() as u32,
                base_vertex,
            });
        }
        // `ranges` has exactly COUNT entries by construction.
        let ranges: [MeshRange; SymbolMeshKey::COUNT] = match ranges.try_into() {
            Ok(r) => r,
            Err(_) => unreachable!("one range per SymbolMeshKey::ALL entry"),
        };
        Self {
            vertices,
            indices,
            ranges,
        }
    }

    pub fn range(&self, key: SymbolMeshKey) -> &MeshRange {
        &self.ranges[key.index()]
    }
}

const CIRCLE_SEGS: u32 = 24;

/// Build the unit-scale mesh for one symbol kind, colored by `theme`.
pub fn build_mesh(theme: &SymbolTheme, key: SymbolMeshKey) -> SymbolMesh {
    let mut m = SymbolMesh::default();
    match key {
        SymbolMeshKey::AirportIntl => {
            let c = theme.airport;
            // Runway tick, canonical orientation north-south (vertical):
            // the per-instance rotation is the runway's true heading.
            quad(&mut m, [0.0, 0.0], 0.16, 0.95, 0.0, c);
            filled_circle(&mut m, [0.0, 0.0], 0.6, CIRCLE_SEGS, c);
        }
        SymbolMeshKey::AirportRegional => {
            let c = theme.airport;
            quad(&mut m, [0.0, 0.0], 0.12, 0.85, 0.0, c); // runway tick, north-south
            ring(&mut m, [0.0, 0.0], 0.72, 0.16, CIRCLE_SEGS, 0.0, c);
        }
        SymbolMeshKey::Airfield => {
            ring(
                &mut m,
                [0.0, 0.0],
                0.7,
                0.18,
                CIRCLE_SEGS,
                0.0,
                theme.airport,
            );
        }
        SymbolMeshKey::GliderSite => {
            let c = theme.glider;
            ring(&mut m, [0.0, 0.0], 0.85, 0.14, CIRCLE_SEGS, 0.0, c);
            ring(&mut m, [0.0, 0.0], 0.48, 0.14, CIRCLE_SEGS, 0.0, c);
        }
        SymbolMeshKey::Heliport => {
            let c = theme.airport;
            ring(&mut m, [0.0, 0.0], 0.8, 0.15, CIRCLE_SEGS, 0.0, c);
            filled_circle(&mut m, [0.0, 0.0], 0.28, CIRCLE_SEGS, c);
        }
        SymbolMeshKey::UltraLight => {
            filled_regular(&mut m, [0.0, 0.0], 0.85, 3, -90.0, theme.airport);
        }
        SymbolMeshKey::Vor => {
            let c = theme.navaid;
            ring(&mut m, [0.0, 0.0], 0.85, 0.14, 6, 0.0, c); // hexagon
            filled_circle(&mut m, [0.0, 0.0], 0.18, 12, c);
        }
        SymbolMeshKey::VorDme => {
            let c = theme.navaid;
            ring(&mut m, [0.0, 0.0], 1.0, 0.12, 4, 45.0, c); // square
            ring(&mut m, [0.0, 0.0], 0.68, 0.12, 6, 0.0, c); // hexagon inside
            filled_circle(&mut m, [0.0, 0.0], 0.16, 12, c);
        }
        SymbolMeshKey::Dme => {
            ring(&mut m, [0.0, 0.0], 0.9, 0.15, 4, 45.0, theme.navaid);
        }
        SymbolMeshKey::Ndb => {
            // Stippled circle: a ring of small dots around a center dot.
            let c = theme.navaid;
            let dots = 12;
            for k in 0..dots {
                let a = (k as f32 / dots as f32) * std::f32::consts::TAU;
                filled_circle(&mut m, [0.8 * a.cos(), 0.8 * a.sin()], 0.12, 6, c);
            }
            filled_circle(&mut m, [0.0, 0.0], 0.2, 12, c);
        }
        SymbolMeshKey::Tacan => {
            let c = theme.navaid;
            ring(&mut m, [0.0, 0.0], 0.8, 0.14, 6, 30.0, c);
            filled_regular(&mut m, [0.0, 0.0], 0.32, 6, 30.0, c);
        }
        SymbolMeshKey::ReportingPointMandatory => {
            filled_regular(&mut m, [0.0, 0.0], 0.9, 3, -90.0, theme.reporting);
        }
        SymbolMeshKey::ReportingPointVoluntary => {
            ring(&mut m, [0.0, 0.0], 0.9, 0.22, 3, -90.0, theme.reporting);
        }
        SymbolMeshKey::Obstacle => {
            // Tower: tall thin triangle, tip up (negative y), dot on the tip.
            let c = theme.obstacle;
            push_triangle(&mut m, [0.0, -0.75], [-0.45, 1.0], [0.45, 1.0], c);
            filled_circle(&mut m, [0.0, -0.78], 0.16, 10, c);
        }
        SymbolMeshKey::WeatherStation => {
            ring(
                &mut m,
                [0.0, 0.0],
                1.0,
                0.18,
                CIRCLE_SEGS,
                0.0,
                theme.weather_outline,
            );
            filled_circle(&mut m, [0.0, 0.0], 0.85, CIRCLE_SEGS, theme.weather_dot);
        }
    }
    m
}

/// Append a triangle, fixing winding so its signed area is positive.
pub(crate) fn push_triangle(
    m: &mut SymbolMesh,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
) {
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let (b, c) = if area >= 0.0 { (b, c) } else { (c, b) };
    let base = m.vertices.len() as u32;
    for pos in [a, b, c] {
        m.vertices.push(SymbolVertex { pos, color });
    }
    m.indices.extend([base, base + 1, base + 2]);
}

pub(crate) fn polar(center: [f32; 2], radius: f32, angle_deg: f32) -> [f32; 2] {
    let a = angle_deg.to_radians();
    [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
}

pub(crate) fn filled_circle(
    m: &mut SymbolMesh,
    center: [f32; 2],
    r: f32,
    segs: u32,
    color: [f32; 4],
) {
    filled_regular(m, center, r, segs, 0.0, color);
}

/// Filled regular polygon (fan around the center).
pub(crate) fn filled_regular(
    m: &mut SymbolMesh,
    center: [f32; 2],
    r: f32,
    corners: u32,
    rotation_deg: f32,
    color: [f32; 4],
) {
    let corners = corners.max(3);
    for k in 0..corners {
        let a0 = rotation_deg + 360.0 * k as f32 / corners as f32;
        let a1 = rotation_deg + 360.0 * (k + 1) as f32 / corners as f32;
        push_triangle(
            m,
            center,
            polar(center, r, a0),
            polar(center, r, a1),
            color,
        );
    }
}

/// Regular polygon outline (annulus between `r` and `r − thickness`);
/// `corners` ≥ ~16 reads as a circle, low counts give hexagons, squares,
/// triangles.
pub(crate) fn ring(
    m: &mut SymbolMesh,
    center: [f32; 2],
    r: f32,
    thickness: f32,
    corners: u32,
    rotation_deg: f32,
    color: [f32; 4],
) {
    let corners = corners.max(3);
    let inner = (r - thickness).max(0.0);
    for k in 0..corners {
        let a0 = rotation_deg + 360.0 * k as f32 / corners as f32;
        let a1 = rotation_deg + 360.0 * (k + 1) as f32 / corners as f32;
        let o0 = polar(center, r, a0);
        let o1 = polar(center, r, a1);
        let i0 = polar(center, inner, a0);
        let i1 = polar(center, inner, a1);
        push_triangle(m, o0, o1, i1, color);
        push_triangle(m, o0, i1, i0, color);
    }
}

/// Axis-aligned rectangle rotated by `rotation_deg`, half-extents `(hw, hh)`.
pub(crate) fn quad(
    m: &mut SymbolMesh,
    center: [f32; 2],
    hw: f32,
    hh: f32,
    rotation_deg: f32,
    color: [f32; 4],
) {
    let a = rotation_deg.to_radians();
    let (sin, cos) = a.sin_cos();
    let rot = |x: f32, y: f32| [center[0] + x * cos - y * sin, center[1] + x * sin + y * cos];
    let c0 = rot(-hw, -hh);
    let c1 = rot(hw, -hh);
    let c2 = rot(hw, hh);
    let c3 = rot(-hw, hh);
    push_triangle(m, c0, c1, c2, color);
    push_triangle(m, c0, c2, c3, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::FlightCategoryColor;
    use crate::map_theme::MapTheme;

    #[test]
    fn every_mesh_is_non_empty_and_consistently_wound() {
        let theme = MapTheme::oldworld();
        for key in SymbolMeshKey::ALL {
            let mesh = build_mesh(&theme.symbols, key);
            assert!(!mesh.vertices.is_empty(), "{key:?}: empty mesh");
            assert!(!mesh.indices.is_empty(), "{key:?}: no indices");
            assert_eq!(mesh.indices.len() % 3, 0, "{key:?}: broken triangle list");
            for tri in mesh.indices.chunks_exact(3) {
                let [a, b, c] =
                    [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize].pos);
                let area =
                    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                assert!(
                    area > 1e-6,
                    "{key:?}: degenerate or miswound triangle (area {area})"
                );
            }
            for v in &mesh.vertices {
                assert!(
                    v.pos[0].abs() <= 1.5 && v.pos[1].abs() <= 1.5,
                    "{key:?}: vertex outside unit symbol space"
                );
            }
        }
    }

    /// The screen rotation (CPU mirror of `symbol.wgsl`) turns clockwise
    /// with north up: a north-pointing unit vertex rotated by a 90° heading
    /// points east, 180° points south, 270° points west.
    #[test]
    fn screen_rotation_is_clockwise_from_north() {
        let north = [0.0, -1.0]; // y is down in symbol space
        let cases = [
            (0.0_f32, [0.0, -1.0]), // north stays north
            (90.0, [1.0, 0.0]),     // east
            (180.0, [0.0, 1.0]),    // south
            (270.0, [-1.0, 0.0]),   // west
        ];
        for (heading_deg, expected) in cases {
            let got = rotate_screen_pos(north, heading_deg.to_radians());
            assert!(
                (got[0] - expected[0]).abs() < 1e-6 && (got[1] - expected[1]).abs() < 1e-6,
                "heading {heading_deg}: got {got:?}, expected {expected:?}"
            );
        }
        // Rotation preserves length (pure rotation, no scale/shear).
        let p = rotate_screen_pos([0.3, 0.7], 1.234);
        assert!((p[0].hypot(p[1]) - 0.3_f32.hypot(0.7)).abs() < 1e-6);
    }

    /// Canonical (un-rotated) orientation of the airport runway tick is
    /// north-south: per-instance `rotation_deg` equals the runway's true
    /// heading. The tick quad is the first 6 vertices of the mesh — tall in
    /// y, thin in x.
    #[test]
    fn airport_runway_tick_is_vertical_pre_rotation() {
        let theme = MapTheme::oldworld();
        for key in [SymbolMeshKey::AirportIntl, SymbolMeshKey::AirportRegional] {
            let mesh = build_mesh(&theme.symbols, key);
            let tick = &mesh.vertices[..6]; // quad = 2 triangles, pushed first
            let max_x = tick.iter().map(|v| v.pos[0].abs()).fold(0.0, f32::max);
            let max_y = tick.iter().map(|v| v.pos[1].abs()).fold(0.0, f32::max);
            assert!(
                max_y > 0.8 && max_x < 0.2,
                "{key:?}: tick must be a vertical (north-south) line, \
                 got extents x ±{max_x}, y ±{max_y}"
            );
        }
    }

    #[test]
    fn atlas_ranges_cover_every_key() {
        let atlas = SymbolAtlas::build(&MapTheme::oldworld().symbols);
        assert!(!atlas.vertices.is_empty());
        let mut total = 0;
        for key in SymbolMeshKey::ALL {
            let range = atlas.range(key);
            assert!(range.indices.start < range.indices.end, "{key:?}");
            total += range.indices.len();
            // Indices plus base vertex stay within the atlas.
            for i in range.indices.clone() {
                let v = atlas.indices[i as usize] as i64 + range.base_vertex as i64;
                assert!(v >= 0 && (v as usize) < atlas.vertices.len(), "{key:?}");
            }
        }
        assert_eq!(total, atlas.indices.len());
    }

    #[test]
    fn zoom_gating_declutters_low_priority_kinds() {
        use crate::camera::{MAX_ZOOM, MIN_ZOOM};
        for key in SymbolMeshKey::ALL {
            let z = key.min_zoom();
            assert!(
                (MIN_ZOOM as f32..MAX_ZOOM as f32).contains(&z),
                "{key:?}: min zoom {z} outside camera range"
            );
        }
        // Obstacles and reporting points only at high zoom.
        assert!(SymbolMeshKey::Obstacle.min_zoom() >= 10.0);
        assert!(SymbolMeshKey::ReportingPointMandatory.min_zoom() >= 10.0);
        assert!(SymbolMeshKey::ReportingPointVoluntary.min_zoom() >= 10.0);
        // Major airports and navaids show early.
        assert!(SymbolMeshKey::AirportIntl.min_zoom() <= 6.0);
        assert!(SymbolMeshKey::Vor.min_zoom() <= 7.0);
        // Every kind un-gates strictly below an obstacle-revealing zoom of 11.
        assert!(
            SymbolMeshKey::ALL.iter().all(|k| k.min_zoom() <= 11.0),
            "nothing may stay hidden at full zoom"
        );
    }

    #[test]
    fn mesh_key_tables_are_consistent() {
        for (i, key) in SymbolMeshKey::ALL.iter().enumerate() {
            assert_eq!(key.index(), i, "{key:?}: ALL order must match index()");
            assert!(key.size_px() > 0.0);
        }
        // Every PointKind maps to a key in its own layer category.
        let kinds = [
            PointKind::AirportIntl,
            PointKind::AirportRegional,
            PointKind::Airfield,
            PointKind::GliderSite,
            PointKind::Heliport,
            PointKind::UltraLight,
            PointKind::Vor,
            PointKind::VorDme,
            PointKind::Dme,
            PointKind::Ndb,
            PointKind::Tacan,
            PointKind::ReportingPointMandatory,
            PointKind::ReportingPointVoluntary,
            PointKind::Obstacle,
            PointKind::WeatherStation(FlightCategoryColor::Vfr),
        ];
        for kind in kinds {
            assert_eq!(SymbolMeshKey::from_kind(kind).category(), kind.layer());
        }
        // Weather dots draw above airports.
        assert!(
            SymbolMeshKey::WeatherStation.draw_order()
                > SymbolMeshKey::AirportIntl.draw_order()
        );
    }
}

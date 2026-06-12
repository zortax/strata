//! Procedural meshes for the route markers: waypoint handles (departure
//! square, waypoint circle, destination square-flag, alternate hollow),
//! TOC/TOD markers and the emphasized profile-scrub marker.
//!
//! Same conventions as [`crate::layers::symbols`]: unit symbol space
//! (x right, y **down**), instanced and sized in logical px at draw time
//! through the shared `symbol.wgsl` pipeline. Colors come from
//! [`RouteTheme`]: fills in `handle_fill`, casings in `handle_outline`
//! (drawn first, underneath, so every handle pops off the route line).

use crate::layers::symbols::{
    MeshRange, SymbolMesh, filled_circle, filled_regular, push_triangle, quad, ring,
};
use crate::map_theme::RouteTheme;

/// Mesh selector for the route markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteSymbolKey {
    Departure,
    Waypoint,
    Destination,
    Alternate,
    Toc,
    Tod,
    Scrub,
}

impl RouteSymbolKey {
    pub const COUNT: usize = 7;

    pub const ALL: [Self; Self::COUNT] = [
        Self::Departure,
        Self::Waypoint,
        Self::Destination,
        Self::Alternate,
        Self::Toc,
        Self::Tod,
        Self::Scrub,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Departure => 0,
            Self::Waypoint => 1,
            Self::Destination => 2,
            Self::Alternate => 3,
            Self::Toc => 4,
            Self::Tod => 5,
            Self::Scrub => 6,
        }
    }

    /// Screen size in logical px that the unit mesh is scaled by. TOC/TOD
    /// stay small; the scrub marker is deliberately emphasized.
    pub fn size_px(self) -> f32 {
        match self {
            Self::Departure => 8.0,
            Self::Waypoint => 6.5,
            Self::Destination => 9.0,
            Self::Alternate => 7.0,
            Self::Toc | Self::Tod => 5.5,
            Self::Scrub => 9.5,
        }
    }
}

/// Scale applied to a handle's [`RouteSymbolKey::size_px`] while its
/// vertex is hover-highlighted ([`crate::features::RenderRoute::highlight`])
/// — a per-instance size change from the retained artifacts, never a mesh
/// rebuild.
pub const HIGHLIGHT_SCALE: f32 = 1.4;

const CIRCLE_SEGS: u32 = 24;

/// Build the unit-scale mesh for one route marker, colored by `theme`.
pub fn build_route_mesh(theme: &RouteTheme, key: RouteSymbolKey) -> SymbolMesh {
    let fill = theme.handle_fill;
    let casing = theme.handle_outline;
    let mut m = SymbolMesh::default();
    match key {
        RouteSymbolKey::Departure => {
            // Filled square on a casing square.
            quad(&mut m, [0.0, 0.0], 0.85, 0.85, 0.0, casing);
            quad(&mut m, [0.0, 0.0], 0.58, 0.58, 0.0, fill);
        }
        RouteSymbolKey::Waypoint => {
            // Filled circle on a casing disc.
            filled_circle(&mut m, [0.0, 0.0], 0.8, CIRCLE_SEGS, casing);
            filled_circle(&mut m, [0.0, 0.0], 0.52, CIRCLE_SEGS, fill);
        }
        RouteSymbolKey::Destination => {
            // Square-flag: the departure square plus a pole and pennant.
            quad(&mut m, [0.0, 0.1], 0.75, 0.75, 0.0, casing);
            quad(&mut m, [0.0, 0.1], 0.5, 0.5, 0.0, fill);
            quad(&mut m, [-0.5, -1.0], 0.09, 0.36, 0.0, casing);
            push_triangle(&mut m, [-0.41, -1.36], [0.55, -1.08], [-0.41, -0.8], fill);
        }
        RouteSymbolKey::Alternate => {
            // Hollow circle (annulus) with a casing annulus around it; the
            // open center keeps the alternate visually "not on the route".
            ring(&mut m, [0.0, 0.0], 1.0, 0.5, CIRCLE_SEGS, 0.0, casing);
            ring(&mut m, [0.0, 0.0], 0.92, 0.34, CIRCLE_SEGS, 0.0, fill);
        }
        RouteSymbolKey::Toc => {
            // Small triangle pointing up (top of climb).
            filled_regular(&mut m, [0.0, 0.0], 1.0, 3, -90.0, casing);
            filled_regular(&mut m, [0.0, 0.0], 0.62, 3, -90.0, fill);
        }
        RouteSymbolKey::Tod => {
            // Small triangle pointing down (top of descent).
            filled_regular(&mut m, [0.0, 0.0], 1.0, 3, 90.0, casing);
            filled_regular(&mut m, [0.0, 0.0], 0.62, 3, 90.0, fill);
        }
        RouteSymbolKey::Scrub => {
            // Emphasized: casing disc, accent ring, accent center dot.
            filled_circle(&mut m, [0.0, 0.0], 1.0, CIRCLE_SEGS, casing);
            ring(&mut m, [0.0, 0.0], 0.85, 0.28, CIRCLE_SEGS, 0.0, fill);
            filled_circle(&mut m, [0.0, 0.0], 0.3, 12, fill);
        }
    }
    m
}

/// All route-marker meshes concatenated for one vertex/index buffer pair
/// (mirrors [`crate::layers::symbols::SymbolAtlas`]).
pub struct RouteSymbolAtlas {
    pub vertices: Vec<crate::layers::symbols::SymbolVertex>,
    pub indices: Vec<u32>,
    ranges: [MeshRange; RouteSymbolKey::COUNT],
}

impl RouteSymbolAtlas {
    /// Build every marker's mesh with the theme's route colors.
    pub fn build(theme: &RouteTheme) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut ranges: Vec<MeshRange> = Vec::with_capacity(RouteSymbolKey::COUNT);
        for key in RouteSymbolKey::ALL {
            let mesh = build_route_mesh(theme, key);
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
        let ranges: [MeshRange; RouteSymbolKey::COUNT] = match ranges.try_into() {
            Ok(r) => r,
            Err(_) => unreachable!("one range per RouteSymbolKey::ALL entry"),
        };
        Self {
            vertices,
            indices,
            ranges,
        }
    }

    pub fn range(&self, key: RouteSymbolKey) -> &MeshRange {
        &self.ranges[key.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_theme::MapTheme;

    #[test]
    fn every_route_mesh_is_non_empty_and_consistently_wound() {
        let theme = MapTheme::oldworld().route;
        for key in RouteSymbolKey::ALL {
            let mesh = build_route_mesh(&theme, key);
            assert!(!mesh.vertices.is_empty(), "{key:?}: empty mesh");
            assert_eq!(mesh.indices.len() % 3, 0, "{key:?}: broken triangle list");
            for tri in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize].pos);
                let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
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

    /// Handles must read against the line: every mesh starts with casing
    /// triangles (`handle_outline`) drawn under the fill.
    #[test]
    fn every_route_mesh_is_cased() {
        let theme = MapTheme::oldworld().route;
        for key in RouteSymbolKey::ALL {
            let mesh = build_route_mesh(&theme, key);
            assert_eq!(
                mesh.vertices[0].color, theme.handle_outline,
                "{key:?}: casing must be drawn first (underneath)"
            );
            assert!(
                mesh.vertices.iter().any(|v| v.color == theme.handle_fill),
                "{key:?}: no fill-colored geometry"
            );
        }
    }

    /// TOC points up, TOD points down (y is down in symbol space).
    #[test]
    fn toc_and_tod_point_apart() {
        let theme = MapTheme::oldworld().route;
        let top = |key| {
            build_route_mesh(&theme, key)
                .vertices
                .iter()
                .map(|v| v.pos[1])
                .fold(f32::INFINITY, f32::min)
        };
        let bottom = |key| {
            build_route_mesh(&theme, key)
                .vertices
                .iter()
                .map(|v| v.pos[1])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        // Up-pointing triangle: its apex (min y) reaches further than its base.
        assert!(top(RouteSymbolKey::Toc) < -0.9);
        assert!(bottom(RouteSymbolKey::Toc) < 0.9);
        assert!(bottom(RouteSymbolKey::Tod) > 0.9);
        assert!(top(RouteSymbolKey::Tod) > -0.9);
    }

    #[test]
    fn atlas_ranges_cover_every_key() {
        let atlas = RouteSymbolAtlas::build(&MapTheme::oldworld().route);
        assert!(!atlas.vertices.is_empty());
        let mut total = 0;
        for key in RouteSymbolKey::ALL {
            let range = atlas.range(key);
            assert!(range.indices.start < range.indices.end, "{key:?}");
            total += range.indices.len();
            for i in range.indices.clone() {
                let v = atlas.indices[i as usize] as i64 + range.base_vertex as i64;
                assert!(v >= 0 && (v as usize) < atlas.vertices.len(), "{key:?}");
            }
        }
        assert_eq!(total, atlas.indices.len());
    }

    #[test]
    fn key_tables_are_consistent() {
        for (i, key) in RouteSymbolKey::ALL.iter().enumerate() {
            assert_eq!(key.index(), i, "{key:?}: ALL order must match index()");
            assert!(key.size_px() > 0.0);
        }
        // The scrub marker is the emphasized one; TOC/TOD stay small.
        for key in RouteSymbolKey::ALL {
            assert!(RouteSymbolKey::Scrub.size_px() >= key.size_px());
        }
        assert!(RouteSymbolKey::Toc.size_px() < RouteSymbolKey::Waypoint.size_px());
    }
}

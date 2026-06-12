//! Progressive draw-buffer assembly: concatenate cached per-feature meshes
//! into one fill/border mesh relative to a common set origin.
//!
//! ## Rebase precision
//!
//! Cached vertices are f32 locals around each feature's own f64 origin. The
//! per-feature rebase offset `feature_origin − set_origin` is computed in
//! f64 and only then narrowed to f32: both origins sit on/near the data, so
//! the offset is small (viewport-to-continent scale, ≪ 1 world unit) and
//! the f32 addition loses well under 1e-7 world units (~5 mm of Mercator
//! ground distance) — asserted by the reconstruction test below.

use super::cache::FeatureMesh;
use crate::features::RenderAirspace;
use crate::geo::{LatLon, world_from_lat_lon};
use crate::layers::tess::{FillMesh, FillVertex, LineMesh, LineVertex};
use crate::text::LabelRequest;

use glam::DVec2;

/// Concatenated draw data for the currently available subset of a set.
pub struct Assembled {
    pub fill: FillMesh,
    pub border: LineMesh,
    pub labels: Vec<LabelRequest>,
    /// How many feature meshes went in (≤ the set size while batches are
    /// still tessellating).
    pub features: usize,
}

/// A stable reference point for a feature set: bbox center over each
/// feature's first exterior vertex. Only ever used as a rebase/uniform
/// origin, so "near the data" is the only requirement; using first vertices
/// keeps it independent of tessellation results (computable at plan time)
/// and stable across progressive re-assemblies.
pub fn set_origin(airspaces: &[RenderAirspace]) -> DVec2 {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    let mut any = false;
    for airspace in airspaces {
        if let Some(&[lon, lat]) = airspace.polygon.first() {
            let p = world_from_lat_lon(LatLon::new(lat, lon));
            min = min.min(p);
            max = max.max(p);
            any = true;
        }
    }
    if any { (min + max) / 2.0 } else { DVec2::ZERO }
}

/// Concatenate `meshes` (set order) into one fill + border mesh relative to
/// `origin`, collecting the cached band labels. Pure CPU concat — no
/// tessellation happens here, so partial sets assemble in well under a
/// frame while batches are still in flight.
pub fn assemble(origin: DVec2, meshes: &[&FeatureMesh]) -> Assembled {
    let mut fill = FillMesh::default();
    let mut border = LineMesh::default();
    fill.vertices
        .reserve(meshes.iter().map(|m| m.fill.vertices.len()).sum());
    fill.indices
        .reserve(meshes.iter().map(|m| m.fill.indices.len()).sum());
    border
        .vertices
        .reserve(meshes.iter().map(|m| m.border.vertices.len()).sum());
    border
        .indices
        .reserve(meshes.iter().map(|m| m.border.indices.len()).sum());
    let mut labels = Vec::new();

    for mesh in meshes {
        // f64 subtraction; the offset is small because both origins are on
        // the data, so the f32 narrowing is safe (module docs).
        let offset = mesh.origin - origin;
        let off = [offset.x as f32, offset.y as f32];

        let fill_base = fill.vertices.len() as u32;
        fill.vertices
            .extend(mesh.fill.vertices.iter().map(|v| FillVertex {
                pos: [v.pos[0] + off[0], v.pos[1] + off[1]],
                color: v.color,
            }));
        fill.indices
            .extend(mesh.fill.indices.iter().map(|i| i + fill_base));

        let border_base = border.vertices.len() as u32;
        border
            .vertices
            .extend(mesh.border.vertices.iter().map(|v| LineVertex {
                pos: [v.pos[0] + off[0], v.pos[1] + off[1]],
                ..*v
            }));
        border
            .indices
            .extend(mesh.border.indices.iter().map(|i| i + border_base));

        if let Some(label) = &mesh.label {
            labels.push(label.clone());
        }
    }

    Assembled {
        fill,
        border,
        labels,
        features: meshes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::build::build_feature_mesh;
    use super::super::tests::ctr;
    use super::*;
    use crate::map_theme::MapTheme;

    /// Two CTRs ~5° of longitude apart: tessellated around their own
    /// origins, rebased to the common set origin, every reconstructed world
    /// position must match the direct projection of the input ring within
    /// 1e-7 world units.
    #[test]
    fn rebase_reconstructs_world_positions_within_1e_7() {
        let theme = MapTheme::oldworld();
        let mut west = ctr(1);
        west.polygon = vec![[6.0, 50.0], [6.2, 50.0], [6.2, 50.1], [6.0, 50.1]];
        let mut east = ctr(2);
        east.polygon = vec![[11.0, 48.0], [11.2, 48.0], [11.2, 48.1], [11.0, 48.1]];

        let set = [west.clone(), east.clone()];
        let origin = set_origin(&set);
        let meshes = [
            build_feature_mesh(&west, &theme),
            build_feature_mesh(&east, &theme),
        ];
        let assembled = assemble(origin, &[&meshes[0], &meshes[1]]);
        assert!(!assembled.fill.vertices.is_empty());

        // Every input ring corner, directly projected.
        let expected: Vec<DVec2> = set
            .iter()
            .flat_map(|a| a.polygon.iter())
            .map(|&[lon, lat]| world_from_lat_lon(LatLon::new(lat, lon)))
            .collect();

        // Convex quads: lyon emits fill vertices exactly at the corners, so
        // each reconstructed vertex must coincide with some projected corner.
        for v in &assembled.fill.vertices {
            let world = origin + DVec2::new(v.pos[0] as f64, v.pos[1] as f64);
            let nearest = expected
                .iter()
                .map(|e| (world - *e).length())
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest < 1e-7,
                "reconstructed vertex {world:?} is {nearest:e} world units \
                 from the nearest direct projection"
            );
        }
        // Border centerlines reconstruct the same way.
        for v in &assembled.border.vertices {
            let world = origin + DVec2::new(v.pos[0] as f64, v.pos[1] as f64);
            let nearest = expected
                .iter()
                .map(|e| (world - *e).length())
                .fold(f64::INFINITY, f64::min);
            assert!(nearest < 1e-7, "border vertex off by {nearest:e}");
        }
    }

    /// Assembly preserves set order, remaps indices into one namespace and
    /// passes the cached labels through untouched (no label recomputation).
    #[test]
    fn assembly_concatenates_in_order_with_cached_labels() {
        let theme = MapTheme::oldworld();
        let a = build_feature_mesh(&ctr(1), &theme);
        let b = build_feature_mesh(&ctr(2), &theme);
        let assembled = assemble(set_origin(&[ctr(1), ctr(2)]), &[&a, &b]);

        assert_eq!(assembled.features, 2);
        assert_eq!(
            assembled.fill.vertices.len(),
            a.fill.vertices.len() + b.fill.vertices.len()
        );
        assert_eq!(
            assembled.fill.indices.len(),
            a.fill.indices.len() + b.fill.indices.len()
        );
        // b's indices were rebased past a's vertices.
        let max_index = *assembled.fill.indices.iter().max().expect("indices");
        assert!((max_index as usize) < assembled.fill.vertices.len());
        let b_min = assembled.fill.indices[a.fill.indices.len()..]
            .iter()
            .min()
            .copied()
            .expect("b indices");
        assert!(b_min as usize >= a.fill.vertices.len());

        // Labels are the cached ones, in set order.
        let cached: Vec<_> = [&a, &b].iter().filter_map(|m| m.label.clone()).collect();
        assert_eq!(assembled.labels, cached);

        // Style payloads survive the rebase untouched.
        assert_eq!(
            assembled.border.vertices[0].width_px,
            a.border.vertices[0].width_px
        );
        assert_eq!(
            assembled.border.vertices[0].dash_px,
            a.border.vertices[0].dash_px
        );
    }

    #[test]
    fn empty_set_assembles_to_nothing_at_origin_zero() {
        assert_eq!(set_origin(&[]), DVec2::ZERO);
        let assembled = assemble(DVec2::ZERO, &[]);
        assert!(assembled.fill.vertices.is_empty());
        assert!(assembled.border.vertices.is_empty());
        assert!(assembled.labels.is_empty());
        assert_eq!(assembled.features, 0);
    }
}

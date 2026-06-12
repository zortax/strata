//! Worker-side per-feature tessellation: one [`FeatureMesh`] per airspace
//! (lyon fill + ring strokes + pole-of-inaccessibility band label), built in
//! small batches so a cold set spreads across all worker threads.

use super::cache::FeatureMesh;
use super::{AIRSPACE_LABEL_MIN_ZOOM, LABEL_ID_NAMESPACE};
use crate::features::RenderAirspace;
use crate::layers::polylabel::pole_of_inaccessibility;
use crate::layers::style::{AirspaceStyle, airspace_style, label_color_from_border, priority};
use crate::layers::tess::{
    FillMesh, LineMesh, StrokeStyleSpec, ring_to_world, tessellate_fill, tessellate_ring_stroke,
};
use crate::map_theme::MapTheme;
use crate::text::{LabelAnchor, LabelPlacement, LabelRequest};

use glam::{DVec2, Vec2};
use rustc_hash::FxHasher;

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

/// One tessellated feature, addressed back into the set being assembled.
pub struct BuiltFeature {
    /// Index into the set this batch was planned for (guarded by the job
    /// generation — a new set invalidates in-flight batches).
    pub set_index: usize,
    pub id: u64,
    pub mesh: Arc<FeatureMesh>,
}

/// Result of one tessellation batch job.
pub struct TessBatch {
    pub built: Vec<BuiltFeature>,
}

/// Pure worker job: tessellate one batch of cache-missing features.
pub fn build_batch(features: &[(usize, RenderAirspace)], theme: &MapTheme) -> TessBatch {
    let span = tracing::debug_span!("airspace_tess_batch", features = features.len());
    let _enter = span.enter();
    let started = Instant::now();
    let built = features
        .iter()
        .map(|(set_index, airspace)| BuiltFeature {
            set_index: *set_index,
            id: airspace.id,
            mesh: Arc::new(build_feature_mesh(airspace, theme)),
        })
        .collect();
    tracing::debug!(
        features = features.len(),
        ms = started.elapsed().as_secs_f64() * 1e3,
        "airspace tessellation batch built"
    );
    TessBatch { built }
}

/// Tessellate one airspace relative to its own origin (exterior bbox
/// center): fill with holes, ring strokes, and the band label.
pub fn build_feature_mesh(airspace: &RenderAirspace, theme: &MapTheme) -> FeatureMesh {
    let exterior = ring_to_world(&airspace.polygon);
    let holes: Vec<Vec<DVec2>> = airspace.holes.iter().map(|h| ring_to_world(h)).collect();
    let origin = bbox_center(&exterior);
    let style = airspace_style(&theme.airspace, airspace.style);

    let mut fill = FillMesh::default();
    tessellate_fill(&exterior, &holes, origin, style.fill, &mut fill);

    let stroke = StrokeStyleSpec {
        color: style.border,
        width_px: style.border_width_px,
        dash_px: style.dash_px,
    };
    let mut border = LineMesh::default();
    tessellate_ring_stroke(&exterior, origin, stroke, &mut border);
    for hole in &holes {
        tessellate_ring_stroke(hole, origin, stroke, &mut border);
    }

    FeatureMesh {
        origin,
        fill,
        border,
        label: band_label(airspace, &style, &exterior, &holes),
        fingerprint: fingerprint(airspace),
    }
}

/// Content fingerprint over everything a [`FeatureMesh`] derives from
/// (geometry, style key, labels) — guards cache hits against a feature id
/// being re-fed with different content.
pub fn fingerprint(airspace: &RenderAirspace) -> u64 {
    let mut hasher = FxHasher::default();
    airspace.style.hash(&mut hasher);
    hash_ring(&airspace.polygon, &mut hasher);
    airspace.holes.len().hash(&mut hasher);
    for hole in &airspace.holes {
        hash_ring(hole, &mut hasher);
    }
    airspace.lower_label.hash(&mut hasher);
    airspace.upper_label.hash(&mut hasher);
    airspace.name.hash(&mut hasher);
    hasher.finish()
}

fn hash_ring(ring: &[[f64; 2]], hasher: &mut FxHasher) {
    ring.len().hash(hasher);
    for &[x, y] in ring {
        x.to_bits().hash(hasher);
        y.to_bits().hash(hasher);
    }
}

fn bbox_center(points: &[DVec2]) -> DVec2 {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    if points.is_empty() {
        DVec2::ZERO
    } else {
        (min + max) / 2.0
    }
}

/// Two-line "NAME\nUPPER / LOWER" label at the polygon's visual center.
fn band_label(
    airspace: &RenderAirspace,
    style: &AirspaceStyle,
    exterior: &[DVec2],
    holes: &[Vec<DVec2>],
) -> Option<LabelRequest> {
    let text = match (
        airspace.name.is_empty(),
        airspace.upper_label.is_empty() && airspace.lower_label.is_empty(),
    ) {
        (true, true) => return None,
        (false, true) => airspace.name.clone(),
        (true, false) => format!("{} / {}", airspace.upper_label, airspace.lower_label),
        (false, false) => format!(
            "{}\n{} / {}",
            airspace.name, airspace.upper_label, airspace.lower_label
        ),
    };
    let anchor = pole_of_inaccessibility(exterior, holes, label_precision(exterior))
        .or_else(|| vertex_mean(exterior))?;
    Some(LabelRequest {
        text: text.into(),
        anchor: LabelAnchor::World(anchor),
        offset_px: Vec2::ZERO,
        placement: LabelPlacement::Center,
        size_px: 11.0,
        color: label_color_from_border(style.border),
        priority: priority::AIRSPACE_BAND,
        min_zoom: AIRSPACE_LABEL_MIN_ZOOM,
        id: airspace.id | LABEL_ID_NAMESPACE,
    })
}

fn label_precision(exterior: &[DVec2]) -> f64 {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in exterior {
        min = min.min(*p);
        max = max.max(*p);
    }
    let size = max - min;
    (size.x.min(size.y) / 64.0).max(1e-9)
}

fn vertex_mean(points: &[DVec2]) -> Option<DVec2> {
    (!points.is_empty()).then(|| points.iter().copied().sum::<DVec2>() / points.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::super::tests::ctr;
    use super::*;
    use crate::features::{AirspaceStyleKey, IcaoClass};
    use crate::layers::polylabel::signed_distance;

    fn build(airspace: &RenderAirspace) -> FeatureMesh {
        build_feature_mesh(airspace, &MapTheme::oldworld())
    }

    #[test]
    fn feature_mesh_contains_fill_border_and_label() {
        let mesh = build(&ctr(7));
        assert!(!mesh.fill.indices.is_empty());
        assert!(!mesh.border.indices.is_empty());
        let label = mesh.label.as_ref().expect("band label cached in the mesh");
        assert_eq!(&*label.text, "FRANKFURT CTR\n2500 MSL / GND");
        assert_eq!(label.priority, priority::AIRSPACE_BAND);
        assert_eq!(label.min_zoom, AIRSPACE_LABEL_MIN_ZOOM);
        assert_eq!(label.id, 7 | LABEL_ID_NAMESPACE);
        // Label anchor lies inside the polygon.
        let LabelAnchor::World(anchor) = label.anchor else {
            panic!("airspace labels are world-anchored");
        };
        let exterior = ring_to_world(&ctr(7).polygon);
        assert!(signed_distance(anchor, &exterior, &[]) > 0.0);
    }

    /// Local vertices stay near the per-feature origin (the rebase-precision
    /// foundation: f32 only holds feature-sized quantities).
    #[test]
    fn vertices_are_local_to_the_feature_origin() {
        let mesh = build(&ctr(1));
        for v in &mesh.fill.vertices {
            assert!(v.pos[0].abs() < 1e-3 && v.pos[1].abs() < 1e-3);
        }
        for v in &mesh.border.vertices {
            assert!(v.pos[0].abs() < 1e-3 && v.pos[1].abs() < 1e-3);
        }
    }

    #[test]
    fn dashed_styles_reach_the_vertices() {
        let mut airspace = ctr(1);
        airspace.style = AirspaceStyleKey::Tmz;
        let mesh = build(&airspace);
        let style = airspace_style(&MapTheme::oldworld().airspace, AirspaceStyleKey::Tmz);
        let (on, off) = style.dash_px.expect("TMZ is dashed");
        assert!(mesh.border.vertices.iter().all(|v| v.dash_px == [on, off]));
    }

    #[test]
    fn solid_class_c_has_zero_dash() {
        let mut airspace = ctr(1);
        airspace.style = AirspaceStyleKey::IcaoClass(IcaoClass::C);
        let mesh = build(&airspace);
        assert!(mesh.border.vertices.iter().all(|v| v.dash_px == [0.0, 0.0]));
    }

    #[test]
    fn nameless_airspace_still_gets_a_band_label() {
        let mut airspace = ctr(1);
        airspace.name = String::new();
        let mesh = build(&airspace);
        assert_eq!(&*mesh.label.expect("label").text, "2500 MSL / GND");
    }

    /// The fingerprint tracks everything the mesh derives from.
    #[test]
    fn fingerprint_changes_with_geometry_style_and_labels() {
        let base = fingerprint(&ctr(1));
        assert_eq!(base, fingerprint(&ctr(1)), "deterministic");

        let mut moved = ctr(1);
        moved.polygon[0] = [10.001, 50.0];
        assert_ne!(base, fingerprint(&moved));

        let mut restyled = ctr(1);
        restyled.style = AirspaceStyleKey::Danger;
        assert_ne!(base, fingerprint(&restyled));

        let mut relabeled = ctr(1);
        relabeled.upper_label = "FL 100".into();
        assert_ne!(base, fingerprint(&relabeled));

        let mut holed = ctr(1);
        holed.holes = vec![vec![[10.05, 50.02], [10.1, 50.02], [10.1, 50.05]]];
        assert_ne!(base, fingerprint(&holed));
    }
}

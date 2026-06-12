//! Label extraction from basemap tiles: place names (`places` layer) and
//! waterway names. The basemap never renders text itself — specs are stored
//! with the tile and queued to the shared [`crate::text::TextSystem`] as
//! [`crate::text::LabelRequest`]s each frame.

use crate::basemap::style::{self, FeatureProperties};
use crate::map_theme::BasemapTheme;
use crate::text::{LabelAnchor, LabelPlacement, LabelRequest};

use glam::{DVec2, Vec2};
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A label extracted from a tile, anchored in world space. Computed once on
/// the worker at decode time.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelSpec {
    /// Refcounted so the per-frame [`LabelRequest`] conversion is
    /// allocation-free.
    pub text: Arc<str>,
    /// Normalized Web-Mercator world anchor.
    pub world: DVec2,
    pub size_px: f32,
    pub color: [f32; 4],
    pub priority: u8,
    pub min_zoom: f32,
    /// Stable across tiles: the same place duplicated into a neighboring
    /// tile's buffer hashes to the same id.
    pub id: u64,
}

impl LabelSpec {
    pub fn to_request(&self) -> LabelRequest {
        LabelRequest {
            text: Arc::clone(&self.text),
            anchor: LabelAnchor::World(self.world),
            offset_px: Vec2::ZERO,
            placement: LabelPlacement::Center,
            size_px: self.size_px,
            color: self.color,
            priority: self.priority,
            min_zoom: self.min_zoom,
            id: self.id,
        }
    }
}

/// A label for a `places` feature, or `None` if unnamed / an unlabeled kind.
pub fn place_label(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    world: DVec2,
) -> Option<LabelSpec> {
    let name = style::str_prop(properties, &["name", "name:de"])?;
    if name.is_empty() {
        return None;
    }
    // 0 = biggest places; used to scale priority and size within localities.
    let rank = style::i64_prop(properties, &["population_rank", "pmap:population_rank"])
        .unwrap_or(0)
        .clamp(0, 15) as u8;
    let schema_min_zoom = style::f64_prop(properties, &["min_zoom", "pmap:min_zoom"]);

    let (size_px, priority, default_min_zoom, color) = match style::kind(properties)? {
        "country" => (14.0, 220, 4.0, theme.country_label),
        "region" => (11.0, 150, 6.5, theme.place_label),
        "locality" | "city" | "town" | "village" | "hamlet" => (
            (13.0 - 0.25 * rank as f32).max(10.0),
            180_u8.saturating_sub(rank * 6),
            7.0 + 0.35 * rank as f64,
            theme.place_label,
        ),
        "macrohood" | "neighbourhood" | "suburb" => (10.0, 60, 11.5, theme.place_label),
        _ => return None,
    };
    let min_zoom = schema_min_zoom.unwrap_or(default_min_zoom) as f32;
    Some(LabelSpec {
        id: stable_id(name, world),
        text: name.into(),
        world,
        size_px,
        color,
        priority,
        min_zoom,
    })
}

/// A label for a named waterway line (river/canal names), anchored at a
/// representative point on the line.
pub fn waterway_label(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    world: DVec2,
) -> Option<LabelSpec> {
    let name = style::str_prop(properties, &["name", "name:de"])?;
    if name.is_empty() {
        return None;
    }
    match style::kind(properties) {
        Some("river" | "canal" | "lake" | "water" | "stream") | None => {}
        Some(_) => return None,
    }
    Some(LabelSpec {
        id: stable_id(name, world),
        text: name.into(),
        world,
        size_px: 10.0,
        color: theme.water_label,
        priority: 40,
        min_zoom: 9.0,
    })
}

/// Stable label identity: same text near the same world position (quantized
/// to ~1e-7 world units) collapses to one id across tile boundaries.
fn stable_id(text: &str, world: DVec2) -> u64 {
    let mut hasher = FxHasher::default();
    text.hash(&mut hasher);
    ((world.x * 1e7).round() as i64).hash(&mut hasher);
    ((world.y * 1e7).round() as i64).hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::basemap::style::PropertyValue;

    fn theme() -> BasemapTheme {
        crate::map_theme::MapTheme::oldworld().basemap
    }

    fn props(pairs: &[(&str, PropertyValue)]) -> FeatureProperties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn locality_label_prioritized_by_population_rank() {
        let world = DVec2::new(0.53, 0.34);
        let big = place_label(
            &theme(),
            &props(&[
                ("name", PropertyValue::Str("Berlin".into())),
                ("kind", PropertyValue::Str("locality".into())),
                ("population_rank", PropertyValue::I64(1)),
            ]),
            world,
        )
        .expect("city label");
        let small = place_label(
            &theme(),
            &props(&[
                ("name", PropertyValue::Str("Kleinstadt".into())),
                ("kind", PropertyValue::Str("locality".into())),
                ("population_rank", PropertyValue::I64(12)),
            ]),
            world,
        )
        .expect("town label");
        assert!(big.priority > small.priority);
        assert!(big.size_px >= small.size_px);
        assert!(big.min_zoom < small.min_zoom);
    }

    #[test]
    fn schema_min_zoom_wins_over_default() {
        let label = place_label(
            &theme(),
            &props(&[
                ("name", PropertyValue::Str("Hamburg".into())),
                ("kind", PropertyValue::Str("locality".into())),
                ("min_zoom", PropertyValue::F64(5.5)),
            ]),
            DVec2::new(0.5, 0.3),
        )
        .expect("label");
        assert_eq!(label.min_zoom, 5.5);
    }

    #[test]
    fn unnamed_features_have_no_label() {
        assert!(
            place_label(
                &theme(),
                &props(&[("kind", PropertyValue::Str("locality".into()))]),
                DVec2::ZERO
            )
            .is_none()
        );
        assert!(waterway_label(&theme(), &FeatureProperties::default(), DVec2::ZERO).is_none());
    }

    #[test]
    fn duplicate_place_in_neighbor_tile_shares_id() {
        let p = props(&[
            ("name", PropertyValue::Str("Mainz".into())),
            ("kind", PropertyValue::Str("locality".into())),
        ]);
        let a = place_label(&theme(), &p, DVec2::new(0.51234567, 0.33)).expect("a");
        let b = place_label(&theme(), &p, DVec2::new(0.51234567, 0.33)).expect("b");
        assert_eq!(a.id, b.id);
        let c = place_label(&theme(), &p, DVec2::new(0.6, 0.33)).expect("c");
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn waterway_label_is_low_priority_and_zoom_gated() {
        let label = waterway_label(
            &theme(),
            &props(&[
                ("name", PropertyValue::Str("Rhein".into())),
                ("kind", PropertyValue::Str("river".into())),
            ]),
            DVec2::new(0.5, 0.34),
        )
        .expect("river label");
        assert!(label.priority < 100);
        assert!(label.min_zoom >= 8.0);
    }
}

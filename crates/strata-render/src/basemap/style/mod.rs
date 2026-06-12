//! Hand-written basemap style for the Protomaps basemap tile schema
//! (v4 `pmap:`-prefixed and v5 plain property names are both handled).
//! Style = plain Rust data, no DSL: geometry rules (widths, dashes, zoom
//! gates) live here, colors come from the active
//! [`crate::map_theme::BasemapTheme`] (premultiplied display-space RGBA, see
//! [`crate::map_theme::srgb8`]).
//!
//! Unknown layers and unknown kinds simply return `None` (gracefully-missing
//! layers are a schema-version reality, not an error).

use crate::map_theme::BasemapTheme;

use rustc_hash::FxHashMap;

/// A decoded MVT feature property value.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Str(String),
    F64(f64),
    I64(i64),
    Bool(bool),
}

/// Decoded MVT feature properties (tag map).
pub type FeatureProperties = FxHashMap<String, PropertyValue>;

/// Fill paint, premultiplied display-space RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillStyle {
    pub color: [f32; 4],
}

/// Stroke paint, premultiplied display-space RGBA; width in logical px;
/// `dash = (dash_px, gap_px)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    pub color: [f32; 4],
    pub width_px: f32,
    pub dash: Option<[f32; 2]>,
}

/// How to paint one basemap feature. At least one of the parts is `Some`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintStyle {
    pub fill: Option<FillStyle>,
    pub stroke: Option<StrokeStyle>,
}

impl PaintStyle {
    fn fill(color: [f32; 4]) -> Self {
        Self {
            fill: Some(FillStyle { color }),
            stroke: None,
        }
    }

    fn stroke(color: [f32; 4], width_px: f32, dash: Option<[f32; 2]>) -> Self {
        Self {
            fill: None,
            stroke: Some(StrokeStyle {
                color,
                width_px,
                dash,
            }),
        }
    }
}

/// Paint order of the basemap MVT layers (lower paints first). Layers not
/// listed here carry no drawable geometry for us (places, pois, buildings…).
pub fn layer_rank(layer: &str) -> Option<u8> {
    match layer {
        "earth" => Some(0),
        "landcover" => Some(1),
        "natural" => Some(2), // v4
        "landuse" => Some(3),
        "water" => Some(4),
        "physical_line" => Some(5), // v4 waterways
        "transit" => Some(6),
        "roads" => Some(7),
        "boundaries" => Some(8),
        _ => None,
    }
}

/// The paint for a feature of the MVT `layer` with `properties` at `zoom`,
/// or `None` if the feature is not drawn at this zoom. Colors come from
/// `theme`.
pub fn style_for(
    theme: &BasemapTheme,
    layer: &str,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    match layer {
        "earth" => Some(PaintStyle::fill(theme.land)),
        "water" => style_water(theme, properties, zoom),
        "physical_line" => style_waterway(theme, properties, zoom),
        "landcover" | "natural" => style_landcover(theme, properties),
        "landuse" => style_landuse(theme, properties),
        "roads" => style_road(theme, properties, zoom),
        "transit" => style_transit(theme, properties, zoom),
        "boundaries" => style_boundary(theme, properties, zoom),
        _ => None,
    }
}

fn style_water(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    // v5 folds waterway centerlines into `water`; polygons have no line kind.
    match kind(properties) {
        Some("river" | "stream" | "canal" | "drain" | "ditch") => {
            style_waterway(theme, properties, zoom)
        }
        // Polygons: fill + a thin shoreline stroke. The stroke is what keeps
        // narrow river ribbons (Rhine, Elbe at z7–9 they are ~1px polygons)
        // and coastlines traceable — key VFR ground reference.
        _ => Some(PaintStyle {
            fill: Some(FillStyle { color: theme.water }),
            stroke: Some(StrokeStyle {
                color: theme.waterway,
                width_px: width_at(zoom, &[(6.0, 0.5), (10.0, 0.9), (14.0, 1.4)]),
                dash: None,
            }),
        }),
    }
}

fn style_waterway(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    let (min_zoom, stops): (f64, &[(f64, f32)]) = match kind(properties) {
        Some("stream" | "drain" | "ditch") => (11.0, &[(11.0, 0.5), (14.0, 1.2)]),
        _ => (7.0, &[(7.0, 1.0), (10.0, 1.6), (14.0, 3.0)]),
    };
    (zoom >= min_zoom).then(|| PaintStyle::stroke(theme.waterway, width_at(zoom, stops), None))
}

fn style_landcover(theme: &BasemapTheme, properties: &FeatureProperties) -> Option<PaintStyle> {
    let color = match kind(properties)? {
        "forest" | "wood" | "woods" => theme.forest,
        "grassland" | "grass" | "scrub" | "meadow" | "wetland" => theme.grass,
        "farmland" | "orchard" | "allotments" => theme.farmland,
        "barren" | "sand" | "beach" | "bare_rock" | "scree" => theme.barren,
        "glacier" | "snow" => theme.glacier,
        "urban_area" => theme.urban,
        _ => return None,
    };
    Some(PaintStyle::fill(color))
}

fn style_landuse(theme: &BasemapTheme, properties: &FeatureProperties) -> Option<PaintStyle> {
    let color = match kind(properties)? {
        "residential" | "neighbourhood" | "suburb" => theme.urban,
        "industrial" | "commercial" | "retail" | "hospital" | "school" | "university"
        | "college" | "railway" => theme.urban_dense,
        "park" | "cemetery" | "golf_course" | "grass" | "garden" | "pitch" | "playground"
        | "recreation_ground" | "village_green" | "zoo" | "dog_park" => theme.park,
        "forest" | "wood" | "nature_reserve" | "orchard" => theme.forest,
        "farmland" | "farmyard" | "allotments" => theme.farmland,
        "military" | "naval_base" | "airfield" => theme.military,
        "aerodrome" | "runway" | "taxiway" => theme.aerodrome,
        "quarry" | "landfill" | "brownfield" => theme.barren,
        _ => return None,
    };
    Some(PaintStyle::fill(color))
}

fn style_road(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    // v5 buckets roads into coarse kinds; `kind_detail` keeps the OSM class.
    let kind = kind(properties)?;
    let detail = str_prop(properties, &["kind_detail", "pmap:kind_detail"]).unwrap_or("");

    let (min_zoom, color, stops): (f64, [f32; 4], &[(f64, f32)]) = match kind {
        "highway" => (
            5.0,
            theme.road_highway,
            &[(5.0, 0.7), (8.0, 1.4), (12.0, 2.8), (15.0, 5.0)],
        ),
        "major_road" => match detail {
            "trunk" | "trunk_link" => (
                6.0,
                theme.road_major,
                &[(6.0, 0.6), (9.0, 1.3), (12.0, 2.2), (15.0, 4.0)],
            ),
            _ => (
                7.0,
                theme.road_major,
                &[(7.0, 0.4), (9.0, 0.9), (12.0, 1.7), (15.0, 3.4)],
            ),
        },
        // Gates are evaluated at the *tile's* zoom (integer steps): 9.5
        // means "from z10 tiles". Medium/minor roads arrive one tile level
        // later than the data offers them — they otherwise flood the map
        // the moment the next level fades in.
        "medium_road" => (
            9.5,
            theme.road_medium,
            &[(9.5, 0.5), (12.0, 1.2), (15.0, 2.6)],
        ),
        "minor_road" => (
            11.5,
            theme.road_minor,
            &[(11.5, 0.5), (13.0, 1.0), (15.0, 2.0)],
        ),
        "path" => (12.5, theme.path, &[(12.5, 0.4), (15.0, 1.0)]),
        "rail" => (10.0, theme.rail, &[(10.0, 0.5), (14.0, 1.2)]),
        _ => return None,
    };
    (zoom >= min_zoom).then(|| PaintStyle::stroke(color, width_at(zoom, stops), None))
}

fn style_transit(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    match kind(properties)? {
        "rail" | "light_rail" | "subway" | "tram" | "railway" => (zoom >= 10.0).then(|| {
            PaintStyle::stroke(
                theme.rail,
                width_at(zoom, &[(10.0, 0.5), (14.0, 1.2)]),
                None,
            )
        }),
        _ => None,
    }
}

fn style_boundary(
    theme: &BasemapTheme,
    properties: &FeatureProperties,
    zoom: f64,
) -> Option<PaintStyle> {
    // v5 names the levels, v4 carries `pmap:min_admin_level` numbers.
    let admin_level = i64_prop(
        properties,
        &["admin_level", "pmap:min_admin_level", "min_admin_level"],
    );
    let named = kind(properties);
    let is_country = named == Some("country") || admin_level == Some(2);
    let is_region = named == Some("region") || admin_level == Some(4);
    if is_country {
        Some(PaintStyle::stroke(
            theme.boundary_country,
            width_at(zoom, &[(4.0, 1.0), (10.0, 1.7)]),
            Some([6.0, 3.0]),
        ))
    } else if is_region && zoom >= 7.0 {
        Some(PaintStyle::stroke(
            theme.boundary_region,
            width_at(zoom, &[(7.0, 0.7), (12.0, 1.2)]),
            Some([4.0, 3.0]),
        ))
    } else {
        None
    }
}

/// `kind` (v5) falling back to `pmap:kind` (v4).
pub fn kind(properties: &FeatureProperties) -> Option<&str> {
    str_prop(properties, &["kind", "pmap:kind"])
}

/// First string property found under any of `keys`.
pub fn str_prop<'a>(properties: &'a FeatureProperties, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| match properties.get(*key) {
        Some(PropertyValue::Str(s)) => Some(s.as_str()),
        _ => None,
    })
}

/// First numeric property under any of `keys`, as i64.
pub fn i64_prop(properties: &FeatureProperties, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| match properties.get(*key) {
        Some(PropertyValue::I64(v)) => Some(*v),
        Some(PropertyValue::F64(v)) => Some(*v as i64),
        _ => None,
    })
}

/// First numeric property under any of `keys`, as f64.
pub fn f64_prop(properties: &FeatureProperties, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| match properties.get(*key) {
        Some(PropertyValue::F64(v)) => Some(*v),
        Some(PropertyValue::I64(v)) => Some(*v as f64),
        _ => None,
    })
}

/// Piecewise-linear interpolation over `(zoom, width)` stops, clamped at the
/// ends. `stops` must be sorted by zoom and non-empty.
pub fn width_at(zoom: f64, stops: &[(f64, f32)]) -> f32 {
    let Some(&(first_z, first_w)) = stops.first() else {
        return 0.0;
    };
    if zoom <= first_z {
        return first_w;
    }
    for window in stops.windows(2) {
        let [(z0, w0), (z1, w1)] = [window[0], window[1]];
        if zoom <= z1 {
            let t = ((zoom - z0) / (z1 - z0)) as f32;
            return w0 + (w1 - w0) * t;
        }
    }
    stops.last().map(|&(_, w)| w).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_theme::MapTheme;

    fn props(pairs: &[(&str, PropertyValue)]) -> FeatureProperties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn str_props(pairs: &[(&str, &str)]) -> FeatureProperties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), PropertyValue::Str((*v).to_owned())))
            .collect()
    }

    fn theme() -> crate::map_theme::BasemapTheme {
        MapTheme::oldworld().basemap
    }

    fn style(layer: &str, properties: &FeatureProperties, zoom: f64) -> Option<PaintStyle> {
        style_for(&theme(), layer, properties, zoom)
    }

    #[test]
    fn water_polygons_fill_at_z8() {
        let paint =
            style("water", &FeatureProperties::default(), 8.0).expect("water must be styled");
        assert!(paint.fill.is_some(), "water is a fill");
        assert_eq!(paint.fill.map(|f| f.color), Some(theme().water));
    }

    #[test]
    fn water_river_lines_stroke_not_fill() {
        let paint = style("water", &str_props(&[("kind", "river")]), 10.0)
            .expect("rivers are styled at z10");
        assert!(paint.fill.is_none());
        assert!(paint.stroke.is_some());
    }

    #[test]
    fn motorway_width_grows_with_zoom() {
        let props = str_props(&[("kind", "highway"), ("kind_detail", "motorway")]);
        let w8 = style("roads", &props, 8.0)
            .and_then(|s| s.stroke)
            .expect("motorway at z8")
            .width_px;
        let w13 = style("roads", &props, 13.0)
            .and_then(|s| s.stroke)
            .expect("motorway at z13")
            .width_px;
        assert!(
            w13 > w8,
            "motorway width must grow with zoom ({w8} → {w13})"
        );
    }

    #[test]
    fn minor_roads_gated_until_z12_tiles() {
        let props = str_props(&[("kind", "minor_road")]);
        assert!(style("roads", &props, 9.0).is_none());
        assert!(
            style("roads", &props, 11.0).is_none(),
            "z11 tiles must not carry minor roads yet"
        );
        assert!(style("roads", &props, 12.0).is_some());
    }

    #[test]
    fn medium_roads_gated_until_z10_tiles() {
        let props = str_props(&[("kind", "medium_road")]);
        assert!(
            style("roads", &props, 9.0).is_none(),
            "z9 tiles must not carry medium roads yet"
        );
        assert!(style("roads", &props, 10.0).is_some());
    }

    #[test]
    fn country_boundary_is_dashed_for_v4_and_v5_properties() {
        let v5 = style("boundaries", &str_props(&[("kind", "country")]), 8.0)
            .and_then(|s| s.stroke)
            .expect("v5 country boundary");
        assert!(v5.dash.is_some());
        let v4 = style(
            "boundaries",
            &props(&[("pmap:min_admin_level", PropertyValue::I64(2))]),
            8.0,
        )
        .and_then(|s| s.stroke)
        .expect("v4 country boundary");
        assert!(v4.dash.is_some());
        // Region boundary (admin_level 4) also dashed, fainter.
        let region = style(
            "boundaries",
            &props(&[("admin_level", PropertyValue::I64(4))]),
            9.0,
        )
        .and_then(|s| s.stroke)
        .expect("region boundary");
        assert!(region.dash.is_some());
        assert!(region.color[3] < v5.color[3]);
    }

    #[test]
    fn unknown_layers_and_kinds_are_skipped() {
        assert!(style("nonexistent_layer", &FeatureProperties::default(), 10.0).is_none());
        assert!(style("landuse", &str_props(&[("kind", "weird")]), 10.0).is_none());
        assert!(style("landuse", &FeatureProperties::default(), 10.0).is_none());
    }

    #[test]
    fn green_landcover_and_urban_landuse_are_subtle_fills() {
        let forest = style("landcover", &str_props(&[("kind", "forest")]), 9.0)
            .and_then(|s| s.fill)
            .expect("forest fill");
        assert_ne!(forest.color, theme().land);
        let urban = style("landuse", &str_props(&[("kind", "residential")]), 11.0)
            .and_then(|s| s.fill)
            .expect("urban fill");
        assert_ne!(urban.color, theme().land);
        // v4 pmap:kind also resolves.
        assert!(style("landcover", &str_props(&[("pmap:kind", "forest")]), 9.0).is_some());
    }

    #[test]
    fn width_interpolation_is_clamped_and_monotone() {
        let stops = [(5.0, 1.0_f32), (10.0, 3.0)];
        assert_eq!(width_at(2.0, &stops), 1.0);
        assert_eq!(width_at(15.0, &stops), 3.0);
        let mid = width_at(7.5, &stops);
        assert!((mid - 2.0).abs() < 1e-6);
    }

    #[test]
    fn layer_rank_orders_ground_below_roads_below_boundaries() {
        let earth = layer_rank("earth").expect("earth ranked");
        let water = layer_rank("water").expect("water ranked");
        let roads = layer_rank("roads").expect("roads ranked");
        let boundaries = layer_rank("boundaries").expect("boundaries ranked");
        assert!(earth < water && water < roads && roads < boundaries);
        assert!(layer_rank("places").is_none());
    }

    /// The same feature styled under different themes paints with that
    /// theme's colors — the style table itself is theme-independent.
    #[test]
    fn styles_follow_the_given_theme() {
        let light = MapTheme::pastel_light().basemap;
        let dark = theme();
        let water_light = style_for(&light, "water", &FeatureProperties::default(), 8.0)
            .and_then(|s| s.fill)
            .expect("light water");
        let water_dark = style_for(&dark, "water", &FeatureProperties::default(), 8.0)
            .and_then(|s| s.fill)
            .expect("dark water");
        assert_eq!(water_light.color, light.water);
        assert_eq!(water_dark.color, dark.water);
        assert_ne!(water_light.color, water_dark.color);
    }
}

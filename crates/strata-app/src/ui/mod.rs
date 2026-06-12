//! Shell UI pieces rendered by [`crate::app::RootView`]: floating layers
//! panel, search, weather time slider, info panel, status bar, theme
//! controls, and shared helpers.

pub mod aircraft_manager;
pub mod context_tabs;
pub mod flight_menu;
pub mod flight_panel;
pub mod info_panel;
pub mod layers_panel;
pub mod profile_drawer;
pub mod profile_view;
pub mod progress_panel;
pub mod search;
pub mod settings;
pub mod status_bar;
pub mod theme;
pub mod time_slider;

use strata_data::store::Feature;

use crate::assets::IconName;

/// Premultiplied **linear** RGBA (the render crate's style colors) as a
/// gpui sRGB color, with alpha forced opaque — used for legend/color chips
/// that must match the map styling.
pub fn chip_color(premultiplied_linear: [f32; 4]) -> gpui::Rgba {
    let [r, g, b, a] = premultiplied_linear;
    let alpha = if a <= f32::EPSILON { 1.0 } else { a };
    let to_srgb = |linear: f32| {
        let v = (linear / alpha).clamp(0.0, 1.0);
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    gpui::Rgba {
        r: to_srgb(r),
        g: to_srgb(g),
        b: to_srgb(b),
        a: 1.0,
    }
}

/// Icon representing a store feature (search results, info panel headers).
pub fn feature_icon(feature: &Feature) -> IconName {
    match feature {
        Feature::Airspace(_) => IconName::Layers,
        Feature::Airport(_) => IconName::Plane,
        Feature::Navaid(_) => IconName::RadioTower,
        Feature::ReportingPoint(_) => IconName::Waypoints,
        Feature::Obstacle(_) => IconName::TriangleAlert,
    }
}

/// Target zoom for fly-to on selecting a feature.
pub fn feature_fly_zoom(feature: &Feature) -> f64 {
    match feature {
        Feature::Airspace(_) => 8.5,
        Feature::Airport(_) => 11.0,
        Feature::Navaid(_) => 10.0,
        Feature::ReportingPoint(_) => 11.0,
        Feature::Obstacle(_) => 12.5,
    }
}

/// Badge text for an airspace: kind abbreviation plus ICAO class where both
/// say something ("TMA C", "CTR D"); a generic area with a class is just the
/// class ("Class E"); unclassified kinds stand alone ("FIS", "Glider Sector").
pub fn airspace_kind_label(
    kind: &strata_data::domain::AirspaceKind,
    class: strata_data::domain::AirspaceClass,
) -> String {
    use strata_data::domain::{AirspaceClass, AirspaceKind};
    match (kind, class) {
        (AirspaceKind::Area, AirspaceClass::Unclassified) => "Area".to_string(),
        (AirspaceKind::Area, class) => format!("Class {class}"),
        (kind, AirspaceClass::Unclassified) => kind.to_string(),
        (kind, class) => format!("{kind} {class}"),
    }
}

/// Short kind word for badges and search rows.
pub fn feature_kind_label(feature: &Feature) -> String {
    match feature {
        Feature::Airspace(a) => airspace_kind_label(&a.kind, a.class),
        Feature::Airport(a) => airport_kind_label(a.kind).to_string(),
        Feature::Navaid(n) => n.kind.to_string(),
        Feature::ReportingPoint(p) => {
            if p.mandatory {
                "Mandatory RP".to_string()
            } else {
                "Reporting point".to_string()
            }
        }
        Feature::Obstacle(o) => obstacle_kind_label(&o.kind).to_string(),
    }
}

pub fn airport_kind_label(kind: strata_data::domain::AirportKind) -> &'static str {
    use strata_data::domain::AirportKind::*;
    match kind {
        International => "International",
        Regional => "Regional",
        Airfield => "Airfield",
        GliderSite => "Glider site",
        Heliport => "Heliport",
        MilitaryAerodrome => "Military",
        UltraLightSite => "Ultralight",
        WaterAirfield => "Water airfield",
        LandingStrip => "Landing strip",
        Closed => "Closed",
        Other(_) => "Aerodrome",
    }
}

pub fn obstacle_kind_label(kind: &strata_data::domain::ObstacleKind) -> &'static str {
    use strata_data::domain::ObstacleKind::*;
    match kind {
        WindTurbine => "Wind turbine",
        Antenna => "Antenna",
        Mast => "Mast",
        Tower => "Tower",
        Chimney => "Chimney",
        Building => "Building",
        PowerLine => "Power line",
        Crane => "Crane",
        Bridge => "Bridge",
        Other(_) => "Obstacle",
    }
}

pub fn runway_surface_label(surface: strata_data::domain::RunwaySurface) -> &'static str {
    use strata_data::domain::RunwaySurface::*;
    match surface {
        Asphalt => "Asphalt",
        Concrete => "Concrete",
        Grass => "Grass",
        Sand => "Sand",
        Water => "Water",
        Gravel => "Gravel",
        Earth => "Earth",
        Snow => "Snow",
        Ice => "Ice",
        Other(_) => "Other",
        Unknown => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_color_round_trips_srgb_red() {
        // style::srgb(255, 0, 0, 0.5) => premultiplied linear (0.5, 0, 0, 0.5)
        let c = chip_color([0.5, 0.0, 0.0, 0.5]);
        assert!((c.r - 1.0).abs() < 1e-3, "r = {}", c.r);
        assert!(c.g.abs() < 1e-3);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn chip_color_handles_zero_alpha() {
        let c = chip_color([0.0, 0.0, 0.0, 0.0]);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn airspace_badges_read_like_chart_annotations() {
        use strata_data::domain::{AirspaceClass, AirspaceKind};
        // Generic area: the ICAO class alone is the right label.
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Area, AirspaceClass::E),
            "Class E"
        );
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Area, AirspaceClass::Unclassified),
            "Area"
        );
        // Known kinds show their abbreviation, plus the class when assigned.
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Tma, AirspaceClass::D),
            "TMA D"
        );
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Ctr, AirspaceClass::D),
            "CTR D"
        );
        assert_eq!(
            airspace_kind_label(&AirspaceKind::FisSector, AirspaceClass::Unclassified),
            "FIS"
        );
        assert_eq!(
            airspace_kind_label(&AirspaceKind::GliderSector, AirspaceClass::Unclassified),
            "Glider Sector"
        );
        // Only genuinely unknown codes still surface the raw code.
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Other(99), AirspaceClass::Unclassified),
            "Other (99)"
        );
        assert_eq!(
            airspace_kind_label(&AirspaceKind::Other(99), AirspaceClass::E),
            "Other (99) E"
        );
    }
}

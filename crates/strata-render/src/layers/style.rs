//! Chart styling for the aero layers (German VFR conventions).
//!
//! Colors come from the active [`crate::map_theme::MapTheme`]; the
//! *geometry* of the chart language — stroke widths, dash patterns, label
//! priorities — is shared by all themes and lives here. All colors are
//! **premultiplied linear RGBA** ready for the premultiplied blend state;
//! themes author them in sRGB bytes via [`srgb`].

use crate::features::{AirspaceStyleKey, FlightCategoryColor, IcaoClass};
use crate::map_theme::{AirspaceTheme, WeatherTheme};

/// Visual style of one airspace category.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirspaceStyle {
    /// Fill color (premultiplied linear). Translucent so stacked airspaces read.
    pub fill: [f32; 4],
    /// Border color (premultiplied linear).
    pub border: [f32; 4],
    /// Border stroke width in logical px (screen-stable).
    pub border_width_px: f32,
    /// Dash pattern `(on_px, off_px)` in logical px; `None` = solid.
    pub dash_px: Option<(f32, f32)>,
}

/// Every style key, for completeness tests and tooling.
pub const ALL_STYLE_KEYS: [AirspaceStyleKey; 16] = [
    AirspaceStyleKey::IcaoClass(IcaoClass::A),
    AirspaceStyleKey::IcaoClass(IcaoClass::B),
    AirspaceStyleKey::IcaoClass(IcaoClass::C),
    AirspaceStyleKey::IcaoClass(IcaoClass::D),
    AirspaceStyleKey::IcaoClass(IcaoClass::E),
    AirspaceStyleKey::IcaoClass(IcaoClass::F),
    AirspaceStyleKey::IcaoClass(IcaoClass::G),
    AirspaceStyleKey::Ctr,
    AirspaceStyleKey::Rmz,
    AirspaceStyleKey::Tmz,
    AirspaceStyleKey::Danger,
    AirspaceStyleKey::Restricted,
    AirspaceStyleKey::Prohibited,
    AirspaceStyleKey::GliderSector,
    AirspaceStyleKey::ParaJump,
    AirspaceStyleKey::Other,
];

/// Border width and dash pattern per key — chart grammar, identical in
/// every theme (only the *colors* are themed).
pub fn airspace_stroke_metrics(key: AirspaceStyleKey) -> (f32, Option<(f32, f32)>) {
    match key {
        AirspaceStyleKey::IcaoClass(IcaoClass::A | IcaoClass::B) => (1.6, None),
        AirspaceStyleKey::IcaoClass(IcaoClass::C) => (1.8, None),
        AirspaceStyleKey::IcaoClass(IcaoClass::D) => (1.5, None),
        // Class E lower boundary is depicted subtly: thin border.
        AirspaceStyleKey::IcaoClass(IcaoClass::E) => (1.0, None),
        AirspaceStyleKey::IcaoClass(IcaoClass::F) => (1.0, Some((7.0, 4.0))),
        AirspaceStyleKey::IcaoClass(IcaoClass::G) => (1.0, None),
        AirspaceStyleKey::Ctr => (1.8, Some((6.0, 3.0))),
        AirspaceStyleKey::Rmz => (1.4, Some((5.0, 3.0))),
        AirspaceStyleKey::Tmz => (1.4, Some((8.0, 4.0))),
        AirspaceStyleKey::Danger => (1.5, Some((5.0, 3.0))),
        AirspaceStyleKey::Restricted => (1.8, None),
        AirspaceStyleKey::Prohibited => (2.2, None),
        AirspaceStyleKey::GliderSector => (1.4, None),
        AirspaceStyleKey::ParaJump => (1.3, Some((4.0, 3.0))),
        AirspaceStyleKey::Other => (1.0, None),
    }
}

/// Chart-correct style per key: theme colors + shared stroke metrics.
pub fn airspace_style(theme: &AirspaceTheme, key: AirspaceStyleKey) -> AirspaceStyle {
    let colors = theme.colors(key);
    let (border_width_px, dash_px) = airspace_stroke_metrics(key);
    AirspaceStyle {
        fill: colors.fill,
        border: colors.border,
        border_width_px,
        dash_px,
    }
}

/// METAR flight-category dot color (premultiplied linear, opaque).
pub fn flight_category_color(theme: &WeatherTheme, category: FlightCategoryColor) -> [f32; 4] {
    match category {
        FlightCategoryColor::Vfr => theme.vfr,
        FlightCategoryColor::Mvfr => theme.mvfr,
        FlightCategoryColor::Ifr => theme.ifr,
        FlightCategoryColor::Lifr => theme.lifr,
    }
}

/// An opaque-ish label color derived from a premultiplied border color.
pub fn label_color_from_border(border: [f32; 4]) -> [f32; 4] {
    let a = border[3].max(1e-3);
    let out_a = 0.95;
    [
        border[0] / a * out_a,
        border[1] / a * out_a,
        border[2] / a * out_a,
        out_a,
    ]
}

/// Label collision priorities (higher wins). Airspace vertical bands sit
/// below airport idents by design.
pub mod priority {
    pub const AIRPORT: u8 = 200;
    /// Route leg labels (MH · GS · alt): the user's own annotations yield
    /// only to airport idents.
    pub const ROUTE_LEG: u8 = 180;
    pub const NAVAID: u8 = 160;
    pub const SIGMET: u8 = 140;
    pub const AIRSPACE_BAND: u8 = 120;
    pub const REPORTING_POINT: u8 = 110;
    pub const WEATHER_STATION: u8 = 100;
    pub const OBSTACLE: u8 = 60;

    // Chart rule: vertical-band labels must yield to airport idents.
    const _: () = assert!(AIRSPACE_BAND < AIRPORT);
    // Leg labels sit below airport idents (design §3.2) but above the
    // ambient feature labels.
    const _: () = assert!(ROUTE_LEG < AIRPORT && ROUTE_LEG > NAVAID);
}

/// sRGB bytes + straight alpha → premultiplied linear RGBA.
pub fn srgb(r: u8, g: u8, b: u8, alpha: f32) -> [f32; 4] {
    let a = alpha.clamp(0.0, 1.0);
    [
        srgb_channel_to_linear(r) * a,
        srgb_channel_to_linear(g) * a,
        srgb_channel_to_linear(b) * a,
        a,
    ]
}

fn srgb_channel_to_linear(byte: u8) -> f32 {
    let s = byte as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_theme::MapTheme;

    #[test]
    fn style_table_covers_every_key_with_sane_values() {
        // `airspace_style` is an exhaustive match, so completeness is also a
        // compile-time guarantee; this asserts the table stays well-formed
        // in every built-in theme.
        for theme in [
            MapTheme::oldworld(),
            MapTheme::high_contrast(),
            MapTheme::pastel_light(),
        ] {
            for key in ALL_STYLE_KEYS {
                let style = airspace_style(&theme.airspace, key);
                assert!(
                    style.fill[3] > 0.0 && style.fill[3] <= 0.5,
                    "{} {key:?}: fill must be translucent so stacking reads, got alpha {}",
                    theme.id,
                    style.fill[3]
                );
                assert!(
                    style.border[3] > 0.0 && style.border[3] <= 1.0,
                    "{} {key:?}: border alpha out of range",
                    theme.id
                );
                assert!(
                    style.border_width_px > 0.0 && style.border_width_px <= 4.0,
                    "{} {key:?}: border width out of range",
                    theme.id
                );
                if let Some((on, off)) = style.dash_px {
                    assert!(on > 0.0 && off > 0.0, "{key:?}: degenerate dash pattern");
                }
                for c in style.fill.iter().chain(style.border.iter()) {
                    assert!((0.0..=1.0).contains(c), "{key:?}: channel out of range");
                }
                // Premultiplied: no color channel may exceed alpha.
                for color in [style.fill, style.border] {
                    for c in &color[..3] {
                        assert!(*c <= color[3] + 1e-6, "{key:?}: color not premultiplied");
                    }
                }
            }
        }
    }

    #[test]
    fn all_style_keys_are_distinct() {
        for (i, a) in ALL_STYLE_KEYS.iter().enumerate() {
            for b in &ALL_STYLE_KEYS[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn chart_conventions_hold() {
        let theme = MapTheme::oldworld();
        let style = |key| airspace_style(&theme.airspace, key);
        // CTR is dashed; ED-R/ED-P are solid and denser than ED-D.
        let ctr = style(AirspaceStyleKey::Ctr);
        assert!(ctr.dash_px.is_some());
        let restricted = style(AirspaceStyleKey::Restricted);
        let prohibited = style(AirspaceStyleKey::Prohibited);
        let danger = style(AirspaceStyleKey::Danger);
        assert!(restricted.dash_px.is_none());
        assert!(prohibited.dash_px.is_none());
        assert!(danger.dash_px.is_some());
        assert!(restricted.fill[3] > danger.fill[3]);
        assert!(prohibited.fill[3] > restricted.fill[3]);
        // RMZ and TMZ are dashed; class C/D are solid.
        assert!(style(AirspaceStyleKey::Rmz).dash_px.is_some());
        assert!(style(AirspaceStyleKey::Tmz).dash_px.is_some());
        assert!(
            style(AirspaceStyleKey::IcaoClass(IcaoClass::C))
                .dash_px
                .is_none()
        );
        // Class E reads more subtle than C.
        let c = style(AirspaceStyleKey::IcaoClass(IcaoClass::C));
        let e = style(AirspaceStyleKey::IcaoClass(IcaoClass::E));
        assert!(e.fill[3] < c.fill[3]);
        assert!(e.border_width_px < c.border_width_px);
    }

    #[test]
    fn flight_category_colors_are_distinct_and_opaque() {
        let theme = MapTheme::oldworld();
        let all = [
            FlightCategoryColor::Vfr,
            FlightCategoryColor::Mvfr,
            FlightCategoryColor::Ifr,
            FlightCategoryColor::Lifr,
        ];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(flight_category_color(&theme.weather, *a)[3], 1.0);
            for b in &all[i + 1..] {
                assert_ne!(
                    flight_category_color(&theme.weather, *a),
                    flight_category_color(&theme.weather, *b)
                );
            }
        }
    }
}

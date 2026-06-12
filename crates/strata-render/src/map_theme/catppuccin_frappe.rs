//! "Catppuccin Frappe" — map theme paired with the Catppuccin Frappe UI
//! theme.
//!
//! The lightest dark Catppuccin flavor: grayer and softer than Macchiato.
//! The ground sits a few channels below the UI `background` `#232634` with
//! its gentle b ≈ r + 14 blue-gray lean carried through the band, so the
//! map matches the Frappe panels. Airspace anchors: controlled = Frappe
//! blue `#8caaee` as steel, CTR/restricted/prohibited = red `#e78284` as a
//! salmon rose, danger = peach `#ef9f76`, glider/para = yellow `#e7d682`
//! as sand, TMZ a lavender gray from `#babbf1`. LIFR leans Frappe's violet
//! magenta `#9882e7`; terrain shadows follow the blue-gray band.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Frappe accent set (sRGB bytes), pastelized.
const STEEL: (u8, u8, u8) = (134, 158, 206); // blue #8caaee — controlled
const FAINT_STEEL: (u8, u8, u8) = (144, 164, 200); // class E/F band
const ROSE: (u8, u8, u8) = (212, 126, 128); // red #e78284 — CTR / ED-R / ED-P
const PEACH: (u8, u8, u8) = (208, 142, 110); // peach #ef9f76 — danger areas
const LAVENDER_GREY: (u8, u8, u8) = (156, 156, 174); // lavender #babbf1 — TMZ
const SAND: (u8, u8, u8) = (206, 184, 126); // yellow #e7d682 — glider / para
const NEUTRAL: (u8, u8, u8) = (143, 144, 150);

fn tint(rgb: (u8, u8, u8), alpha: f32) -> [f32; 4] {
    srgb(rgb.0, rgb.1, rgb.2, alpha)
}

fn pair(rgb: (u8, u8, u8), fill_alpha: f32, border_alpha: f32) -> AirspaceColors {
    AirspaceColors {
        fill: tint(rgb, fill_alpha),
        border: tint(rgb, border_alpha),
    }
}

fn stop(value: f32, rgb: (u8, u8, u8), alpha: f32) -> ColorStop {
    ColorStop {
        value,
        color: tint(rgb, alpha),
    }
}

pub(super) fn theme() -> MapTheme {
    // Soft blue-gray ground just under the Frappe UI background.
    let land = srgb8(0x1d, 0x1f, 0x2b);
    MapTheme {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappe",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and a touch bluer than land.
            water: srgb8(0x19, 0x1b, 0x27),
            waterway: srgb8(0x27, 0x2d, 0x3f),
            // Landcover: a whisker around land, all in the blue-gray band.
            forest: srgb8(0x1b, 0x1d, 0x29),
            grass: srgb8(0x1c, 0x1e, 0x2a),
            farmland: srgb8(0x1e, 0x20, 0x2c),
            barren: srgb8(0x1f, 0x21, 0x2d),
            glacier: srgb8(0x23, 0x25, 0x32),
            park: srgb8(0x1c, 0x1e, 0x2a),
            urban: srgb8(0x21, 0x23, 0x30),
            urban_dense: srgb8(0x24, 0x26, 0x35),
            military: srgb8(0x20, 0x22, 0x2e),
            aerodrome: srgb8(0x22, 0x24, 0x33),
            // Compressed road ramp: motorway ~+16 channels over land,
            // then +11 / +8 / +5 / +2 — faint texture, never lines.
            road_highway: srgb8(0x2d, 0x2f, 0x3b),
            road_major: srgb8(0x28, 0x2a, 0x36),
            road_medium: srgb8(0x25, 0x27, 0x33),
            road_minor: srgb8(0x22, 0x24, 0x30),
            path: srgb8(0x1f, 0x21, 0x2d),
            rail: srgb8_a(0x26, 0x28, 0x35, 0.85),
            // Boundaries in Frappe's blue-gray overlay tones.
            boundary_country: srgb8_a(0x64, 0x66, 0x78, 0.55),
            boundary_region: srgb8_a(0x4b, 0x4d, 0x60, 0.30),
            place_label: srgb8(0x5a, 0x5c, 0x6e),
            country_label: srgb8(0x67, 0x69, 0x7c),
            water_label: srgb8(0x48, 0x50, 0x66),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.05, 0.72),
            class_b: pair(STEEL, 0.05, 0.72),
            class_c: pair(STEEL, 0.07, 0.78),
            class_d: pair(STEEL, 0.05, 0.7),
            class_e: pair(FAINT_STEEL, 0.02, 0.35),
            class_f: pair(FAINT_STEEL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(STEEL, 0.035, 0.68),
            tmz: pair(LAVENDER_GREY, 0.03, 0.75),
            danger: pair(PEACH, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Soft lavender-white symbols echo Frappe's text scale.
            airport: srgb(198, 200, 210, 1.0),
            glider: srgb(208, 188, 138, 1.0),
            navaid: srgb(148, 160, 184, 1.0),
            reporting: srgb(216, 220, 230, 1.0),
            obstacle: srgb(210, 130, 130, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 32, 40, 1.0),
        },
        weather: WeatherTheme {
            // Frappe green / blue / red, LIFR toward the violet magenta.
            vfr: srgb(110, 188, 128, 1.0),
            mvfr: srgb(112, 150, 220, 1.0),
            ifr: srgb(218, 110, 116, 1.0),
            lifr: srgb(186, 118, 196, 1.0),
            sigmet: srgb(215, 150, 105, 0.45),
            // Muted gridded overlays with a faint cool lean.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 150, 158), 0.0),
                stop(40.0, (156, 158, 166), 0.12),
                stop(75.0, (184, 186, 196), 0.28),
                stop(100.0, (208, 210, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (112, 142, 222), 0.0),
                stop(1.0, (112, 142, 222), 0.32),
                stop(5.0, (110, 192, 204), 0.42),
                stop(20.0, (216, 192, 108), 0.5),
                stop(50.0, (212, 100, 108), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (224, 164, 100), 0.0),
                stop(5.0, (218, 150, 88), 0.32),
                stop(15.0, (205, 100, 95), 0.5),
            ]),
        },
        // Route: Catppuccin pink (#f4b8e4 family) — the brand accent no
        // airspace uses; conflicts in frappe red.
        route: RouteTheme {
            line: srgb(242, 150, 202, 1.0),
            line_conflict: srgb(232, 88, 90, 1.0),
            handle_fill: srgb(242, 150, 202, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(242, 150, 202, 0.12),
        },
        labels: LabelTheme {
            // Frappe foreground #c6d0f5, slightly desaturated.
            text: srgb(196, 202, 222, 0.95),
            halo: [0.0; 4],
        },
        // Relief shadows toward the band's blue-violet, lights neutral-cool.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x18, 0x15, 0x22),
            light_tint: tint_from_srgb8(0x84, 0x80, 0x8a),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

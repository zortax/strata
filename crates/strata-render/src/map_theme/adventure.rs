//! "Adventure" — map theme paired with the Adventure UI theme: a pitch-black
//! console look (background `#040404`, faintly cool title bar `#0f1112`)
//! with steel-blue, teal, chartreuse and vermilion accents.
//!
//! The basemap is a near-black, barely-cool grayscale band sitting between
//! the UI background and title bar; roads stay faint texture. Airspaces
//! carry the accent identity: steel blue (`base.blue #417ab3`) for the
//! controlled classes, vermilion (`danger #d84a33`) for CTR/restricted/
//! prohibited, terracotta (`base.red.light #d76b42`) for danger areas, teal
//! (`base.cyan #41b3a9`) for RMZ and muted amber (`base.yellow #aa7900`)
//! for glider sectors.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Adventure accent set (sRGB bytes).
const STEEL: (u8, u8, u8) = (95, 145, 190); // base.blue #417ab3, lifted — controlled
const FAINT_STEEL: (u8, u8, u8) = (122, 158, 196); // class E/F band
const VERMILION: (u8, u8, u8) = (214, 100, 80); // danger #d84a33 — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (215, 124, 86); // base.red.light #d76b42 — danger areas
const TEAL: (u8, u8, u8) = (84, 178, 168); // base.cyan #41b3a9 — RMZ
const AMBER: (u8, u8, u8) = (198, 156, 84); // base.yellow #aa7900, lifted — glider / para
const MAUVE_GREY: (u8, u8, u8) = (146, 150, 158); // TMZ
const NEUTRAL: (u8, u8, u8) = (138, 142, 144);

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
    // Pitch-black ground a hair above the UI background (#040404), leaning
    // toward the title bar's cool tint so map and chrome read as one.
    let land = srgb8(0x08, 0x09, 0x09);
    MapTheme {
        id: "adventure",
        name: "Adventure",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and cooler than the already-black land.
            water: srgb8(0x06, 0x07, 0x09),
            waterway: srgb8(0x13, 0x19, 0x22),
            // Landcover: a whisker around land, neutral with the faintest
            // cool/green hints.
            forest: srgb8(0x07, 0x08, 0x08),
            grass: srgb8(0x08, 0x09, 0x08),
            farmland: srgb8(0x09, 0x0a, 0x09),
            barren: srgb8(0x0a, 0x0a, 0x0a),
            glacier: srgb8(0x0c, 0x0d, 0x0f),
            park: srgb8(0x08, 0x09, 0x08),
            urban: srgb8(0x0c, 0x0d, 0x0d),
            urban_dense: srgb8(0x0e, 0x0f, 0x10),
            military: srgb8(0x0b, 0x0c, 0x0c),
            aerodrome: srgb8(0x0b, 0x0c, 0x0e),
            // Compressed road ramp: tiny equal steps above the black ground
            // (the ground is so dark that even +7 lands at ratio ~1.8).
            road_highway: srgb8(0x0f, 0x10, 0x10),
            road_major: srgb8(0x0d, 0x0e, 0x0e),
            road_medium: srgb8(0x0c, 0x0d, 0x0d),
            road_minor: srgb8(0x0b, 0x0b, 0x0c),
            path: srgb8(0x0a, 0x0a, 0x0b),
            rail: srgb8_a(0x0d, 0x0e, 0x10, 0.85),
            boundary_country: srgb8_a(0x54, 0x56, 0x5c, 0.55),
            boundary_region: srgb8_a(0x3c, 0x3e, 0x44, 0.30),
            place_label: srgb8(0x4d, 0x51, 0x56),
            country_label: srgb8(0x5a, 0x5e, 0x63),
            water_label: srgb8(0x3b, 0x44, 0x4f),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.05, 0.72),
            class_b: pair(STEEL, 0.05, 0.72),
            class_c: pair(STEEL, 0.07, 0.78),
            class_d: pair(STEEL, 0.05, 0.7),
            class_e: pair(FAINT_STEEL, 0.02, 0.35),
            class_f: pair(FAINT_STEEL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(VERMILION, 0.09, 0.8),
            rmz: pair(TEAL, 0.035, 0.68),
            tmz: pair(MAUVE_GREY, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(VERMILION, 0.12, 0.8),
            prohibited: pair(VERMILION, 0.16, 0.85),
            glider_sector: pair(AMBER, 0.045, 0.75),
            para_jump: pair(AMBER, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(198, 200, 198, 1.0),
            glider: srgb(200, 170, 110, 1.0),
            navaid: srgb(140, 160, 180, 1.0),
            reporting: srgb(214, 218, 220, 1.0),
            obstacle: srgb(210, 118, 96, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(24, 26, 28, 1.0),
        },
        weather: WeatherTheme {
            // Semantic categories tuned to the accents: chartreuse-leaning
            // VFR (base.green), steel MVFR, vermilion IFR, magenta LIFR
            // (base.magenta.light, deepened).
            vfr: srgb(122, 186, 96, 1.0),
            mvfr: srgb(100, 150, 214, 1.0),
            ifr: srgb(216, 100, 92, 1.0),
            lifr: srgb(198, 108, 182, 1.0),
            sigmet: srgb(216, 140, 80, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 150, 152), 0.0),
                stop(40.0, (156, 158, 160), 0.12),
                stop(75.0, (184, 186, 188), 0.28),
                stop(100.0, (208, 212, 216), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (86, 134, 196), 0.0),
                stop(1.0, (86, 134, 196), 0.32),
                stop(5.0, (84, 182, 190), 0.42),
                stop(20.0, (206, 184, 92), 0.5),
                stop(50.0, (208, 90, 72), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (214, 160, 90), 0.0),
                stop(5.0, (210, 146, 76), 0.32),
                stop(15.0, (200, 90, 70), 0.5),
            ]),
        },
        // Route: high-vis amber over the near-black ground; conflicts in
        // vermilion red.
        route: RouteTheme {
            line: srgb(255, 176, 64, 1.0),
            line_conflict: srgb(235, 78, 70, 1.0),
            handle_fill: srgb(255, 176, 64, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 176, 64, 0.12),
        },
        labels: LabelTheme {
            // Cool off-white from the theme's near-white foreground.
            text: srgb(212, 216, 218, 0.95),
            halo: [0.0; 4],
        },
        // Neutral-cool relief to match the grayscale band.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x13, 0x14, 0x16),
            light_tint: tint_from_srgb8(0x7e, 0x80, 0x84),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

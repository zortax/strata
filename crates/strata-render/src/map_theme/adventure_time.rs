//! "Adventure Time" — map theme paired with the Adventure Time UI theme:
//! a deep indigo-violet world (background `#1f1d45`, title bar `#1c1a3e`)
//! with periwinkle, burnt-orange and cream accents.
//!
//! The basemap keeps the UI's strong blue hue ratio (b ≈ 2.2·r) in a
//! low-contrast indigo band a few channels below the window background.
//! Airspaces anchor on the periwinkle primary (`#5f72c6`) for controlled
//! classes, a dusty red from `base.red #a02733` for CTR/restricted/
//! prohibited, burnt orange (`base.yellow #ce7837`) for danger areas,
//! mauve (`base.magenta #665993`) for TMZ and a sand from
//! `base.yellow.light #efc11a` for glider sectors. Labels lean toward the
//! editor's warm cream foreground (`#f8dcc0`).

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Adventure Time accent set (sRGB bytes).
const PERIWINKLE: (u8, u8, u8) = (110, 126, 200); // primary #5f72c6 — controlled
const FAINT_PERIWINKLE: (u8, u8, u8) = (130, 144, 202); // class E/F band
const DUSTY_RED: (u8, u8, u8) = (192, 92, 100); // base.red #a02733, lifted — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (206, 128, 82); // base.yellow #ce7837 — danger areas
const MAUVE: (u8, u8, u8) = (150, 142, 178); // base.magenta #665993, lifted — TMZ
const SAND: (u8, u8, u8) = (212, 180, 106); // base.yellow.light #efc11a, muted — glider / para
const NEUTRAL: (u8, u8, u8) = (144, 144, 156); // violet-leaning neutral

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
    // A few channels below the UI background #1f1d45, same indigo ratio.
    let land = srgb8(0x1b, 0x1a, 0x3c);
    MapTheme {
        id: "adventure-time",
        name: "Adventure Time",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Darker and a touch bluer than the indigo land.
            water: srgb8(0x16, 0x16, 0x3a),
            waterway: srgb8(0x28, 0x2e, 0x5c),
            // Landcover: ±1..3 channels around land, all inside the indigo
            // band.
            forest: srgb8(0x19, 0x1b, 0x38),
            grass: srgb8(0x1b, 0x1c, 0x3a),
            farmland: srgb8(0x1d, 0x1c, 0x3c),
            barren: srgb8(0x1e, 0x1d, 0x3d),
            glacier: srgb8(0x21, 0x22, 0x43),
            park: srgb8(0x1a, 0x1b, 0x39),
            urban: srgb8(0x21, 0x1f, 0x43),
            urban_dense: srgb8(0x24, 0x22, 0x47),
            military: srgb8(0x20, 0x1e, 0x40),
            aerodrome: srgb8(0x22, 0x21, 0x47),
            // Road ramp: equal small steps above land, keeping the hue
            // (motorway ≈ +16 → luma ratio ~1.56).
            road_highway: srgb8(0x2b, 0x2a, 0x4d),
            road_major: srgb8(0x27, 0x26, 0x48),
            road_medium: srgb8(0x23, 0x22, 0x44),
            road_minor: srgb8(0x20, 0x1f, 0x41),
            path: srgb8(0x1e, 0x1d, 0x3f),
            rail: srgb8_a(0x23, 0x22, 0x48, 0.85),
            // Violet-gray boundaries, kin to the muted foreground #717192.
            boundary_country: srgb8_a(0x6a, 0x68, 0x90, 0.55),
            boundary_region: srgb8_a(0x4c, 0x4a, 0x70, 0.30),
            place_label: srgb8(0x62, 0x60, 0x8a),
            country_label: srgb8(0x6f, 0x6d, 0x99),
            water_label: srgb8(0x4d, 0x53, 0x80),
        },
        airspace: AirspaceTheme {
            class_a: pair(PERIWINKLE, 0.05, 0.72),
            class_b: pair(PERIWINKLE, 0.05, 0.72),
            class_c: pair(PERIWINKLE, 0.07, 0.78),
            class_d: pair(PERIWINKLE, 0.05, 0.7),
            class_e: pair(FAINT_PERIWINKLE, 0.02, 0.35),
            class_f: pair(FAINT_PERIWINKLE, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(DUSTY_RED, 0.09, 0.8),
            rmz: pair(PERIWINKLE, 0.035, 0.68),
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(DUSTY_RED, 0.12, 0.8),
            prohibited: pair(DUSTY_RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Cream-leaning symbols echo the editor foreground.
            airport: srgb(216, 206, 190, 1.0),
            glider: srgb(210, 184, 120, 1.0),
            navaid: srgb(146, 156, 196, 1.0),
            reporting: srgb(224, 220, 228, 1.0),
            obstacle: srgb(206, 112, 104, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 29, 52, 1.0),
        },
        weather: WeatherTheme {
            // Green from base.green #549235, blue toward the periwinkle
            // primary, red/magenta softened from base.red / base.magenta.
            vfr: srgb(118, 184, 108, 1.0),
            mvfr: srgb(108, 138, 224, 1.0),
            ifr: srgb(212, 96, 98, 1.0),
            lifr: srgb(190, 116, 196, 1.0),
            sigmet: srgb(210, 130, 70, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 150, 160), 0.0),
                stop(40.0, (158, 158, 168), 0.12),
                stop(75.0, (184, 184, 194), 0.28),
                stop(100.0, (208, 208, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (100, 130, 210), 0.0),
                stop(1.0, (100, 130, 210), 0.32),
                stop(5.0, (96, 184, 200), 0.42),
                stop(20.0, (210, 184, 96), 0.5),
                stop(50.0, (206, 94, 84), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (216, 160, 92), 0.0),
                stop(5.0, (212, 146, 80), 0.32),
                stop(15.0, (198, 92, 86), 0.5),
            ]),
        },
        // Route: high-vis amber against the indigo night ground; conflicts in
        // a vivid red.
        route: RouteTheme {
            line: srgb(255, 184, 70, 1.0),
            line_conflict: srgb(240, 80, 90, 1.0),
            handle_fill: srgb(255, 184, 70, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 184, 70, 0.12),
        },
        labels: LabelTheme {
            // Warm cream from the editor foreground #f8dcc0, desaturated.
            text: srgb(226, 212, 188, 0.95),
            halo: [0.0; 4],
        },
        // Violet shadows so the relief stays inside the indigo band.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x16, 0x13, 0x30),
            light_tint: tint_from_srgb8(0x8a, 0x86, 0xa0),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

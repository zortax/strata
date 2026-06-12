//! "Hybrid Dark" — map theme paired with the Hybrid Dark UI theme
//! (w0ng/vim-hybrid, Tomorrow-Night lineage).
//!
//! Palette rationale: the basemap is a cool blue-leaning gray band derived
//! from the UI background `#1D1F21` (the road_medium step lands exactly on
//! it, land sits a few channels below at `#16181A`), so map and chrome read
//! as one surface. Airspace anchors come from the Hybrid accent set:
//! controlled airspace uses the Hybrid blue (`base.blue.light #6e90b0`),
//! the CTR / restricted / prohibited family the Tomorrow-Night red
//! (`#cc6666`), danger the warmer orange (`#de935f`), glider/para the
//! yellow (`base.yellow.light #e4b55e`, muted to sand) and TMZ the soft
//! purple (`base.magenta.light #B294BB`). Weather keeps its semantics with
//! Hybrid's chartreuse green, steel blue, soft red and purple-magenta.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) from the Hybrid accent set.
const STEEL: (u8, u8, u8) = (110, 144, 176); // base.blue.light — controlled
const FAINT_STEEL: (u8, u8, u8) = (134, 156, 178); // class E/F band
const ROSE: (u8, u8, u8) = (204, 102, 102); // Tomorrow-Night red — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (210, 140, 92); // hybrid orange — danger areas
const MAUVE: (u8, u8, u8) = (178, 148, 187); // base.magenta.light — TMZ
const SAND: (u8, u8, u8) = (218, 178, 108); // base.yellow.light, muted — glider / para
const NEUTRAL: (u8, u8, u8) = (135, 135, 135); // muted_foreground

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
    // Cool blue-gray ground a few channels below the UI background #1D1F21,
    // keeping its g = r+2 / b = r+4 hue ratio.
    let land = srgb8(0x16, 0x18, 0x1a);
    MapTheme {
        id: "hybrid-dark",
        name: "Hybrid Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than land.
            water: srgb8(0x12, 0x14, 0x19),
            waterway: srgb8(0x1f, 0x26, 0x2e),
            // Landcover: a whisker around land, staying in the cool band.
            forest: srgb8(0x14, 0x16, 0x18),
            grass: srgb8(0x15, 0x17, 0x19),
            farmland: srgb8(0x17, 0x19, 0x1b),
            barren: srgb8(0x18, 0x19, 0x1b),
            glacier: srgb8(0x1a, 0x1c, 0x1f),
            park: srgb8(0x15, 0x17, 0x19),
            urban: srgb8(0x1a, 0x1c, 0x1e),
            urban_dense: srgb8(0x1d, 0x1f, 0x21),
            military: srgb8(0x19, 0x1b, 0x1d),
            aerodrome: srgb8(0x1b, 0x1d, 0x20),
            // Compressed road ramp: +2/+4/+7/+10/+14 over land; the medium
            // step coincides with the UI background.
            road_highway: srgb8(0x24, 0x26, 0x28),
            road_major: srgb8(0x20, 0x22, 0x24),
            road_medium: srgb8(0x1d, 0x1f, 0x21),
            road_minor: srgb8(0x1a, 0x1c, 0x1e),
            path: srgb8(0x18, 0x1a, 0x1c),
            rail: srgb8_a(0x1e, 0x20, 0x23, 0.85),
            // Cool gray boundaries, country clearly stronger than region.
            boundary_country: srgb8_a(0x5e, 0x61, 0x66, 0.55),
            boundary_region: srgb8_a(0x44, 0x47, 0x4c, 0.30),
            place_label: srgb8(0x54, 0x58, 0x5c),
            country_label: srgb8(0x62, 0x66, 0x6a),
            water_label: srgb8(0x3f, 0x49, 0x54),
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
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(196, 200, 202, 1.0),
            glider: srgb(212, 180, 112, 1.0),
            navaid: srgb(138, 158, 176, 1.0),
            reporting: srgb(214, 218, 220, 1.0),
            obstacle: srgb(204, 118, 110, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 32, 34, 1.0),
        },
        weather: WeatherTheme {
            // Hybrid's chartreuse green / steel blue / soft red / purple.
            vfr: srgb(140, 186, 100, 1.0),
            mvfr: srgb(110, 150, 210, 1.0),
            ifr: srgb(214, 108, 104, 1.0),
            lifr: srgb(190, 116, 194, 1.0),
            sigmet: srgb(222, 147, 95, 0.45),
            // Gridded overlays, muted and slightly cool.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 158), 0.0),
                stop(40.0, (156, 160, 166), 0.12),
                stop(75.0, (184, 188, 194), 0.28),
                stop(100.0, (210, 214, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (100, 140, 200), 0.0),
                stop(1.0, (100, 140, 200), 0.32),
                stop(5.0, (96, 186, 196), 0.42),
                stop(20.0, (206, 186, 98), 0.5),
                stop(50.0, (202, 94, 82), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (214, 160, 92), 0.0),
                stop(5.0, (210, 146, 80), 0.32),
                stop(15.0, (198, 88, 78), 0.5),
            ]),
        },
        // Route: high-vis amber over the slate ground; conflicts in Tomorrow-
        // Night red.
        route: RouteTheme {
            line: srgb(250, 180, 68, 1.0),
            line_conflict: srgb(226, 80, 80, 1.0),
            handle_fill: srgb(250, 180, 68, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 180, 68, 0.12),
        },
        labels: LabelTheme {
            // From the UI foreground #e8e8e8, slightly cooled and softened.
            text: srgb(200, 206, 210, 0.95),
            halo: [0.0; 4],
        },
        // Neutral-cool relief to match the blue-gray band.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x12, 0x14, 0x17),
            light_tint: tint_from_srgb8(0x7e, 0x82, 0x88),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

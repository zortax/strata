//! "Hybrid Light" — map theme paired with the Hybrid Light UI theme
//! (w0ng/vim-hybrid, light variant: neutral gray chrome with dim
//! terminal-style accents).
//!
//! Palette rationale: the UI is a pure neutral gray (`background #E4E4E4`,
//! `panel #d7d7d7`, `title_bar #D0D0D0`), so the map ground is neutral gray
//! paper just below the window background (`#E0E0E0`) with only whisper-faint
//! landcover tints. Airspace anchors come from the dim Hybrid Light accents:
//! controlled airspace from the teal-blue primary (`#005f87`, lifted to a
//! readable mid-tone), the CTR / restricted / prohibited family from the
//! red (`danger #ff5f5f`, deepened to a chart rose), danger areas a warmer
//! terracotta, glider/para the olive yellow (`#948000`) and TMZ the plum
//! magenta (`#5f1c51`, grayed). The motorway keeps an olive hue so the one
//! traceable road reads as part of the same dim-accent family.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), mid-toned so they read on gray paper.
const STEEL: (u8, u8, u8) = (38, 102, 134); // primary #005f87, lifted — controlled
const FAINT_STEEL: (u8, u8, u8) = (84, 124, 150); // class E/F band
const ROSE: (u8, u8, u8) = (186, 74, 74); // danger red, deepened — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (180, 102, 70); // danger areas
const PLUM: (u8, u8, u8) = (108, 86, 104); // base.magenta, grayed — TMZ
const OCHRE: (u8, u8, u8) = (146, 124, 38); // base.yellow #948000 — glider / para
const NEUTRAL: (u8, u8, u8) = (108, 108, 112); // muted_foreground family

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
    // Neutral gray paper just below the UI background #E4E4E4.
    let land = srgb8(0xe0, 0xe0, 0xe0);
    MapTheme {
        id: "hybrid-light",
        name: "Hybrid Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Desaturated teal-gray water, visibly darker than the paper.
            water: srgb8(0xae, 0xc2, 0xcc),
            waterway: srgb8(0x7a, 0x93, 0xa4),
            // Landcover: barely-tinted grays a whisker around land.
            forest: srgb8(0xd2, 0xd8, 0xce),
            grass: srgb8(0xd9, 0xde, 0xd2),
            farmland: srgb8(0xde, 0xdd, 0xd0),
            barren: srgb8(0xdb, 0xd6, 0xca),
            glacier: srgb8(0xe9, 0xed, 0xf0),
            park: srgb8(0xd5, 0xdb, 0xd0),
            urban: srgb8(0xd6, 0xd3, 0xd1),
            urban_dense: srgb8(0xcc, 0xc9, 0xc7),
            military: srgb8(0xd8, 0xcf, 0xcb),
            aerodrome: srgb8(0xd3, 0xd4, 0xdb),
            // Roads slightly darker than ground; the motorway keeps the
            // theme's olive-yellow hue.
            road_highway: srgb8(0xa8, 0x9b, 0x55),
            road_major: srgb8(0x8e, 0x8e, 0x92),
            road_medium: srgb8(0xa4, 0xa4, 0xa8),
            road_minor: srgb8(0xb8, 0xb8, 0xbb),
            path: srgb8(0xc6, 0xc6, 0xc9),
            rail: srgb8_a(0x97, 0x97, 0x9c, 0.9),
            // Muted plum-gray boundaries, legible on gray paper.
            boundary_country: srgb8_a(0x66, 0x5c, 0x68, 0.75),
            boundary_region: srgb8_a(0x83, 0x7c, 0x88, 0.5),
            place_label: srgb8(0x56, 0x52, 0x58),
            country_label: srgb8(0x48, 0x44, 0x4e),
            water_label: srgb8(0x46, 0x60, 0x6e),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.07, 0.75),
            class_b: pair(STEEL, 0.07, 0.75),
            class_c: pair(STEEL, 0.1, 0.85),
            class_d: pair(STEEL, 0.07, 0.75),
            class_e: pair(FAINT_STEEL, 0.035, 0.45),
            class_f: pair(FAINT_STEEL, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(ROSE, 0.11, 0.85),
            rmz: pair(STEEL, 0.05, 0.7),
            tmz: pair(PLUM, 0.04, 0.75),
            danger: pair(TERRACOTTA, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark symbols on light ground.
            airport: srgb(60, 62, 66, 1.0),
            glider: srgb(134, 110, 40, 1.0),
            navaid: srgb(52, 92, 116, 1.0),
            reporting: srgb(54, 56, 60, 1.0),
            obstacle: srgb(170, 66, 62, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 40, 44, 1.0),
        },
        weather: WeatherTheme {
            // The dim terminal accents, lifted just enough to read on gray.
            vfr: srgb(16, 140, 64, 1.0),
            mvfr: srgb(28, 92, 196, 1.0),
            ifr: srgb(196, 48, 52, 1.0),
            lifr: srgb(150, 52, 148, 1.0),
            sigmet: srgb(200, 94, 40, 0.5),
            // Gridded overlays: darker grays so the ramps read on gray paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (124, 128, 134), 0.0),
                stop(40.0, (130, 134, 140), 0.12),
                stop(75.0, (144, 148, 154), 0.26),
                stop(100.0, (158, 162, 168), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 110, 180), 0.0),
                stop(1.0, (60, 110, 180), 0.34),
                stop(5.0, (40, 150, 170), 0.44),
                stop(20.0, (180, 152, 48), 0.52),
                stop(50.0, (180, 58, 48), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (192, 128, 42), 0.0),
                stop(5.0, (186, 116, 36), 0.36),
                stop(15.0, (170, 52, 44), 0.54),
            ]),
        },
        // Route: deep violet — vivid on the neutral paper; conflicts in brick
        // red.
        route: RouteTheme {
            line: srgb(110, 70, 190, 1.0),
            line_conflict: srgb(204, 48, 54, 1.0),
            handle_fill: srgb(110, 70, 190, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(110, 70, 190, 0.12),
        },
        labels: LabelTheme {
            // Near-black text (UI foreground #1c1c1c softened) over a light
            // neutral halo.
            text: srgb(44, 44, 48, 0.95),
            halo: srgb(240, 240, 240, 0.85),
        },
        // Strictly neutral relief for the grayscale chrome.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x52, 0x52, 0x55),
            light_tint: tint_from_srgb8(0xea, 0xea, 0xec),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

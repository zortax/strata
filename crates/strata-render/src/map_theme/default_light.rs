//! "Default Light" — map theme paired with the Default Light UI theme (the
//! gpui-component registry built-in: shadcn's Tailwind *neutral* palette).
//!
//! The UI chrome is pure white / cool neutral (`background` `#ffffff`,
//! `group_box` neutral-100 `#f5f5f5`, `title_bar` `#f8f8f8`, borders
//! neutral-200), so the basemap is cool-neutral paper: land `#f0f0f0` just
//! below the white window background, landcover and roads a notch darker
//! and barely tinted. Identity comes from the Tailwind 600-level accents:
//! controlled airspace = blue-600 (`#2563eb`, slightly tamed),
//! CTR/restricted/prohibited = red-500 (`#ef4444`, toned down), danger
//! leans orange-600, glider/para = yellow-600 muted to ochre, TMZ a
//! purple-gray. Flight categories are the Tailwind 600s nearly verbatim
//! (purple-600 shifted toward magenta: its linear-space hue is only ~28°
//! from blue-600, below the 30° separation floor). Dark text over a light
//! halo; neutral relief.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) — Tailwind 600-level accents, tamed to read
// as chart ink on paper.
const BLUE: (u8, u8, u8) = (48, 102, 216); // blue-600 — controlled
const FAINT_BLUE: (u8, u8, u8) = (92, 124, 184); // class E/F band
const RED: (u8, u8, u8) = (208, 62, 66); // red-500 — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (198, 108, 56); // orange-600 lean — danger areas
const GRAY_VIOLET: (u8, u8, u8) = (112, 106, 128); // TMZ
const OCHRE: (u8, u8, u8) = (172, 128, 36); // yellow-600 muted — glider / para
const NEUTRAL: (u8, u8, u8) = (108, 108, 114);

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
    // Cool-neutral paper just below the pure-white UI background — gray,
    // not warm: the shadcn neutral palette has no cream in it.
    let land = srgb8(0xf0, 0xf0, 0xf0);
    MapTheme {
        id: "default-light",
        name: "Default Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly cooler and darker so water reads immediately on paper.
            water: srgb8(0xc2, 0xcd, 0xd6),
            waterway: srgb8(0x80, 0x95, 0xa8),
            // Landcover: a notch darker than ground, only whisper-tinted.
            forest: srgb8(0xdd, 0xe4, 0xda),
            grass: srgb8(0xe3, 0xe8, 0xdf),
            farmland: srgb8(0xeb, 0xe9, 0xe0),
            barren: srgb8(0xe7, 0xe2, 0xd8),
            glacier: srgb8(0xf3, 0xf5, 0xf8),
            park: srgb8(0xe0, 0xe6, 0xdc),
            urban: srgb8(0xe3, 0xe0, 0xde),
            urban_dense: srgb8(0xd9, 0xd6, 0xd4),
            military: srgb8(0xe4, 0xdb, 0xd8),
            aerodrome: srgb8(0xe0, 0xe0, 0xe8),
            // Roads slightly darker than paper; the motorway keeps a
            // restrained slate-blue (the theme's only cool ink) instead of
            // pastel-light's ochre.
            road_highway: srgb8(0x82, 0x8c, 0xa0),
            road_major: srgb8(0x96, 0x96, 0x9c),
            road_medium: srgb8(0xa8, 0xa8, 0xae),
            road_minor: srgb8(0xba, 0xba, 0xc0),
            path: srgb8(0xc8, 0xc8, 0xce),
            rail: srgb8_a(0x9e, 0x9c, 0xa4, 0.9),
            // Neutral dark grays, clearly legible on paper.
            boundary_country: srgb8_a(0x60, 0x60, 0x68, 0.75),
            boundary_region: srgb8_a(0x7a, 0x7a, 0x84, 0.5),
            place_label: srgb8(0x5a, 0x5a, 0x60),
            country_label: srgb8(0x48, 0x48, 0x50),
            water_label: srgb8(0x46, 0x5c, 0x74),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.07, 0.75),
            class_b: pair(BLUE, 0.07, 0.75),
            class_c: pair(BLUE, 0.1, 0.85),
            class_d: pair(BLUE, 0.07, 0.75),
            class_e: pair(FAINT_BLUE, 0.035, 0.45),
            class_f: pair(FAINT_BLUE, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(RED, 0.11, 0.85),
            rmz: pair(BLUE, 0.05, 0.7),
            tmz: pair(GRAY_VIOLET, 0.04, 0.75),
            danger: pair(ORANGE, 0.08, 0.75),
            restricted: pair(RED, 0.15, 0.85),
            prohibited: pair(RED, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark neutral ink on paper; accent families on glider/navaid/
            // obstacle.
            airport: srgb(58, 58, 64, 1.0),
            glider: srgb(150, 112, 30, 1.0),
            navaid: srgb(74, 92, 128, 1.0),
            reporting: srgb(50, 50, 58, 1.0),
            obstacle: srgb(180, 60, 58, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 40, 46, 1.0),
        },
        weather: WeatherTheme {
            // Tailwind 600s: green-600 / blue-600 / red-500; purple-600
            // shifted toward magenta for LIFR/MVFR hue separation.
            vfr: srgb(22, 163, 74, 1.0),
            mvfr: srgb(37, 99, 235, 1.0),
            ifr: srgb(239, 68, 68, 1.0),
            lifr: srgb(168, 44, 196, 1.0),
            sigmet: srgb(206, 102, 28, 0.5),
            // Gridded overlays lean darker so they read on light paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (124, 128, 136), 0.0),
                stop(40.0, (130, 134, 142), 0.12),
                stop(75.0, (144, 148, 156), 0.26),
                stop(100.0, (158, 162, 170), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (58, 110, 210), 0.0),
                stop(1.0, (58, 110, 210), 0.34),
                stop(5.0, (30, 150, 170), 0.44),
                stop(20.0, (190, 150, 40), 0.52),
                stop(50.0, (195, 55, 48), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (200, 130, 40), 0.0),
                stop(5.0, (192, 118, 36), 0.36),
                stop(15.0, (178, 52, 44), 0.54),
            ]),
        },
        // Route: deep violet (purple-600) — vivid on the neutral paper;
        // conflicts in red-600.
        route: RouteTheme {
            line: srgb(122, 62, 210, 1.0),
            line_conflict: srgb(224, 38, 44, 1.0),
            handle_fill: srgb(122, 62, 210, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(122, 62, 210, 0.12),
        },
        labels: LabelTheme {
            // Near-neutral-800 ink over a near-white halo.
            text: srgb(40, 40, 46, 0.95),
            halo: srgb(250, 250, 252, 0.85),
        },
        // Neutral relief: cool-gray shadows toward near-white lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x4e, 0x4e, 0x52),
            light_tint: tint_from_srgb8(0xf4, 0xf4, 0xf6),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

//! "Gruvbox Light" — map theme paired with the Gruvbox Light UI theme.
//!
//! Palette rationale: warm cream-yellow paper just below the UI background
//! (`#fbf1c7` window, `#ebdbb2` panels) at `#f3e9c4`, with landcover in
//! muted olive-tans and water in a desaturated lift of the deep teal
//! `base.blue #076678`. Airspace identity from the Gruvbox accents darkened
//! to mid-tones for paper: controlled airspace in the deep teal-blue,
//! CTR/restricted/prohibited from the red `#fb4934` (as a brick red),
//! danger a warmer burnt orange, glider/para in ochre from `base.yellow
//! #b57614`, TMZ a warm gray, LIFR from `base.magenta #8f3f71`. Dark
//! warm-gray label text (UI fg `#3c3836` family) over a cream halo; relief
//! shadows lean warm brown, lights toward paper.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Gruvbox light accents, darkened to mid-tones so they read on paper.
const BLUE: (u8, u8, u8) = (30, 100, 122); // base.blue — controlled airspace
const FAINT_BLUE: (u8, u8, u8) = (84, 124, 138); // class E/F band
const RED: (u8, u8, u8) = (180, 70, 56); // base.red — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (176, 102, 52); // burnt orange — danger areas
const WARM_GRAY: (u8, u8, u8) = (104, 98, 86); // muted fg family — TMZ
const OCHRE: (u8, u8, u8) = (158, 116, 38); // base.yellow — glider / para
const NEUTRAL: (u8, u8, u8) = (110, 104, 92);

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
    // Warm cream-yellow paper, a notch below the UI's `#fbf1c7`.
    let land = srgb8(0xf3, 0xe9, 0xc4);
    MapTheme {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated teal so water reads immediately.
            water: srgb8(0xa8, 0xc1, 0xb8),
            waterway: srgb8(0x62, 0x8e, 0x88),
            // Landcover a notch darker than paper, in muted olive-tans.
            forest: srgb8(0xd8, 0xdc, 0xae),
            grass: srgb8(0xe1, 0xe2, 0xb6),
            farmland: srgb8(0xee, 0xe2, 0xb4),
            barren: srgb8(0xe8, 0xda, 0xb2),
            glacier: srgb8(0xee, 0xee, 0xe2),
            park: srgb8(0xdc, 0xde, 0xb1),
            // Urban fabric as slightly darker, grayer-brown patches.
            urban: srgb8(0xe4, 0xd7, 0xb9),
            urban_dense: srgb8(0xd9, 0xcb, 0xae),
            military: srgb8(0xe3, 0xd2, 0xb0),
            aerodrome: srgb8(0xe0, 0xdc, 0xc4),
            // Roads darker than paper; motorway keeps the yellow-brown of
            // `base.yellow #b57614`, the rest stay warm gray.
            road_highway: srgb8(0xb4, 0x86, 0x46),
            road_major: srgb8(0x9b, 0x93, 0x7d),
            road_medium: srgb8(0xaf, 0xa7, 0x90),
            road_minor: srgb8(0xc1, 0xb9, 0xa1),
            path: srgb8(0xcf, 0xc7, 0xae),
            rail: srgb8_a(0xa3, 0x9b, 0x85, 0.9),
            // Dark warm grays, clearly legible on paper.
            boundary_country: srgb8_a(0x6c, 0x5f, 0x52, 0.75),
            boundary_region: srgb8_a(0x86, 0x79, 0x6a, 0.5),
            place_label: srgb8(0x60, 0x57, 0x49),
            country_label: srgb8(0x4f, 0x47, 0x3b),
            water_label: srgb8(0x3f, 0x63, 0x60),
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
            tmz: pair(WARM_GRAY, 0.04, 0.75),
            danger: pair(ORANGE, 0.08, 0.75),
            restricted: pair(RED, 0.15, 0.85),
            prohibited: pair(RED, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark warm symbols (UI fg #3c3836 family) on cream ground.
            airport: srgb(70, 64, 56, 1.0),
            glider: srgb(140, 102, 32, 1.0),
            navaid: srgb(52, 92, 104, 1.0),
            reporting: srgb(60, 54, 48, 1.0),
            obstacle: srgb(164, 58, 44, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(44, 40, 36, 1.0),
        },
        weather: WeatherTheme {
            // Gruvbox green / a blue lift of base.blue / brick red /
            // base.magenta — saturated enough to read on cream.
            vfr: srgb(84, 148, 56, 1.0),
            mvfr: srgb(28, 104, 190, 1.0),
            ifr: srgb(204, 52, 42, 1.0),
            lifr: srgb(150, 46, 134, 1.0),
            sigmet: srgb(192, 98, 28, 0.5),
            // Gridded overlays lean darker gray so they read on paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (126, 130, 126), 0.0),
                stop(40.0, (132, 136, 132), 0.12),
                stop(75.0, (146, 150, 146), 0.26),
                stop(100.0, (160, 164, 160), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (58, 110, 184), 0.0),
                stop(1.0, (58, 110, 184), 0.34),
                stop(5.0, (44, 150, 162), 0.44),
                stop(20.0, (184, 148, 46), 0.52),
                stop(50.0, (186, 58, 44), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (188, 126, 40), 0.0),
                stop(5.0, (182, 112, 34), 0.36),
                stop(15.0, (172, 56, 44), 0.54),
            ]),
        },
        // Route: deep gruvbox purple (#8f3f71 family) — vivid on the warm
        // cream; conflicts in gruvbox red.
        route: RouteTheme {
            line: srgb(150, 56, 108, 1.0),
            line_conflict: srgb(192, 48, 38, 1.0),
            handle_fill: srgb(150, 56, 108, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(150, 56, 108, 0.12),
        },
        labels: LabelTheme {
            // The UI foreground #3c3836, over a cream halo so idents read
            // on colored fills.
            text: srgb(64, 58, 52, 0.95),
            halo: srgb(248, 240, 208, 0.85),
        },
        // Relief: warm brown shadows, lights toward cream paper.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x5c, 0x52, 0x40),
            light_tint: tint_from_srgb8(0xf6, 0xef, 0xd6),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

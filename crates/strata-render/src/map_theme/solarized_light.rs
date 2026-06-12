//! "Solarized Light" — map theme paired with the Solarized Light UI theme.
//!
//! Solarized Light's chrome is base3 cream (`background #FDF6E3`, muted
//! base2 `#EEE8D5`). The map ground is the same warm cream a notch below
//! the window background. Airspace identity comes from the classic
//! Solarized accent row (the saturated parents of the UI theme's muted
//! `base.*` colors), darkened to mid-tones so they read on cream: blue
//! `#268BD2` for controlled airspace, red `#DC322F` for CTR/restricted/
//! prohibited, orange `#CB4B16` for danger, yellow `#B58900` as ochre for
//! glider sectors (and the muted motorway hue), cyan `#2AA198` tinting
//! the class E/F band — mirroring Solarized Dark. Text leans base02
//! (`#073642`) over a cream halo; boundaries sit on the grey-cyan
//! base01 scale.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), classic Solarized accents darkened for cream.
const BLUE: (u8, u8, u8) = (32, 110, 170); // blue #268BD2 — controlled
const FAINT_CYAN: (u8, u8, u8) = (52, 124, 118); // cyan #2AA198 — class E/F band
const ROSE: (u8, u8, u8) = (185, 60, 55); // red #DC322F — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (175, 85, 35); // orange #CB4B16 — danger areas
const OCHRE: (u8, u8, u8) = (150, 116, 25); // yellow #B58900 — glider / para
const TMZ_GREY: (u8, u8, u8) = (102, 112, 112); // base01 grey-cyan — TMZ
const NEUTRAL: (u8, u8, u8) = (108, 114, 110);

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
    // base3 cream, a notch below the window background #FDF6E3.
    let land = srgb8(0xf2, 0xeb, 0xd5);
    MapTheme {
        id: "solarized-light",
        name: "Solarized Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker grey-teal — Solarized's cyan note for water.
            water: srgb8(0xaf, 0xc4, 0xc6),
            waterway: srgb8(0x7b, 0x98, 0x9e),
            // Landcover: soft warm tints a whisker below cream.
            forest: srgb8(0xdc, 0xdd, 0xc0),
            grass: srgb8(0xe4, 0xe3, 0xc8),
            farmland: srgb8(0xec, 0xe2, 0xc2),
            barren: srgb8(0xe6, 0xda, 0xbe),
            glacier: srgb8(0xef, 0xf0, 0xea),
            park: srgb8(0xe0, 0xe0, 0xc4),
            urban: srgb8(0xe2, 0xd9, 0xc8),
            urban_dense: srgb8(0xd7, 0xcd, 0xbc),
            military: srgb8(0xe0, 0xd2, 0xc2),
            aerodrome: srgb8(0xdd, 0xd9, 0xd6),
            // Roads slightly darker than cream; the motorway keeps a muted
            // Solarized-yellow ochre (highway/land luma ≈ 0.60).
            road_highway: srgb8(0xac, 0x8c, 0x46),
            road_major: srgb8(0x8e, 0x8e, 0x82),
            road_medium: srgb8(0xa3, 0xa2, 0x94),
            road_minor: srgb8(0xb7, 0xb5, 0xa6),
            path: srgb8(0xc9, 0xc6, 0xb6),
            rail: srgb8_a(0x9b, 0x99, 0x8c, 0.9),
            // Boundaries on the base01 grey-cyan scale, legible on cream.
            boundary_country: srgb8_a(0x5e, 0x6c, 0x6e, 0.75),
            boundary_region: srgb8_a(0x7d, 0x88, 0x88, 0.5),
            place_label: srgb8(0x5a, 0x64, 0x68),
            country_label: srgb8(0x46, 0x55, 0x5c),
            water_label: srgb8(0x4a, 0x6b, 0x73),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.07, 0.75),
            class_b: pair(BLUE, 0.07, 0.75),
            class_c: pair(BLUE, 0.1, 0.85),
            class_d: pair(BLUE, 0.07, 0.75),
            class_e: pair(FAINT_CYAN, 0.035, 0.45),
            class_f: pair(FAINT_CYAN, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(ROSE, 0.11, 0.85),
            rmz: pair(BLUE, 0.05, 0.7),
            tmz: pair(TMZ_GREY, 0.04, 0.75),
            danger: pair(EMBER, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // base02-leaning dark symbols on cream.
            airport: srgb(45, 60, 66, 1.0),
            glider: srgb(138, 105, 25, 1.0),
            navaid: srgb(50, 90, 125, 1.0),
            reporting: srgb(35, 50, 56, 1.0),
            obstacle: srgb(175, 60, 50, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 44, 46, 1.0),
        },
        weather: WeatherTheme {
            // Classic Solarized accents, darkened to read on cream.
            vfr: srgb(100, 128, 25, 1.0),   // green #859900
            mvfr: srgb(30, 100, 195, 1.0),  // blue #268BD2
            ifr: srgb(195, 45, 40, 1.0),    // red #DC322F
            lifr: srgb(170, 42, 135, 1.0),  // magenta #D33682
            sigmet: srgb(185, 80, 28, 0.5), // orange #CB4B16
            // Gridded overlays: muted grey-cyan ramp on cream.
            cloud_cover: Colormap::new(&[
                stop(10.0, (126, 132, 134), 0.0),
                stop(40.0, (132, 138, 140), 0.12),
                stop(75.0, (146, 152, 154), 0.26),
                stop(100.0, (160, 166, 168), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (55, 110, 185), 0.0),
                stop(1.0, (55, 110, 185), 0.34),
                stop(5.0, (45, 150, 165), 0.44),
                stop(20.0, (180, 150, 48), 0.52),
                stop(50.0, (180, 58, 48), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (185, 128, 38), 0.0),
                stop(5.0, (180, 116, 32), 0.36),
                stop(15.0, (168, 55, 44), 0.54),
            ]),
        },
        // Route: solarized magenta #d33682 deepened for paper; conflicts in
        // solarized red.
        route: RouteTheme {
            line: srgb(190, 42, 114, 1.0),
            line_conflict: srgb(208, 40, 34, 1.0),
            handle_fill: srgb(190, 42, 114, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(190, 42, 114, 0.12),
        },
        labels: LabelTheme {
            // Softened base02 ink over a cream halo.
            text: srgb(20, 60, 72, 0.95),
            halo: srgb(250, 245, 227, 0.85),
        },
        // Relief: warm grey shadows, cream lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x57, 0x4f, 0x41),
            light_tint: tint_from_srgb8(0xf4, 0xee, 0xdb),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

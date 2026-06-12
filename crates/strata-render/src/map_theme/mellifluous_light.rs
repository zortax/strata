//! "Mellifluous Light" — map theme paired with the Mellifluous Light UI
//! theme. The UI is a soft neutral grey (`#E7E7E7` window, `#dfdfdf` title
//! bar, `#fafafa` panel) with gentle lavender identity (primary `#5A6599`,
//! blue `#a8a1be`), so the ground is a grey paper with the faintest mauve
//! cast and muted olive/warm landcover (the theme's green is olive
//! `#828040`, its yellow the soft orange `#c98f54` — which also tints the
//! motorway). Airspaces darken the muted accents to mid-tones: the indigo
//! primary `#5A6599` for controlled airspace, the dusty red `#C95954` for
//! CTR/restricted/prohibited with a warmer terracotta danger, the orange
//! deepened to ochre for glider/para, and a lavender-grey TMZ from
//! `#a8a1be`/`#b39fb0`. Boundaries and labels lean grey-violet so map and
//! chrome share the lavender undertone.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues, darkened to mid-tones so they read on grey paper.
const STEEL: (u8, u8, u8) = (82, 94, 150); // primary indigo #5A6599 — controlled
const FAINT_STEEL: (u8, u8, u8) = (110, 120, 160); // class E/F band
const ROSE: (u8, u8, u8) = (178, 76, 70); // red #C95954 deepened — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (184, 100, 62); // danger (warmer)
const MAUVE_GREY: (u8, u8, u8) = (122, 116, 134); // TMZ — lavender #a8a1be greyed
const OCHRE: (u8, u8, u8) = (156, 120, 58); // orange #c98f54 deepened — glider / para
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
    // Soft grey paper just below the #E7E7E7 window background, with the
    // faintest mauve cast.
    let land = srgb8(0xe5, 0xe4, 0xe6);
    MapTheme {
        id: "mellifluous-light",
        name: "Mellifluous Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated blue-grey so water reads at once.
            water: srgb8(0xb4, 0xbf, 0xca),
            waterway: srgb8(0x7f, 0x93, 0xa8),
            // Landcover: muted olive greens and soft warm greys, a notch
            // around the paper.
            forest: srgb8(0xd3, 0xd8, 0xc8),
            grass: srgb8(0xdc, 0xdf, 0xd0),
            farmland: srgb8(0xe2, 0xdf, 0xd2),
            barren: srgb8(0xde, 0xd8, 0xcc),
            glacier: srgb8(0xec, 0xef, 0xf2),
            park: srgb8(0xd7, 0xdc, 0xcb),
            urban: srgb8(0xdb, 0xd7, 0xd9),
            urban_dense: srgb8(0xd0, 0xcc, 0xcf),
            military: srgb8(0xd9, 0xd2, 0xcf),
            aerodrome: srgb8(0xd8, 0xd6, 0xde),
            // Roads slightly darker than paper; the motorway keeps the soft
            // orange hue, the rest a mauve-grey ramp.
            road_highway: srgb8(0xb5, 0x97, 0x70),
            road_major: srgb8(0x90, 0x8d, 0x97),
            road_medium: srgb8(0xa6, 0xa2, 0xac),
            road_minor: srgb8(0xb9, 0xb5, 0xbd),
            path: srgb8(0xc7, 0xc3, 0xc9),
            rail: srgb8_a(0x9a, 0x96, 0xa1, 0.9),
            // Grey-violet boundaries, clearly legible on paper.
            boundary_country: srgb8_a(0x6a, 0x64, 0x78, 0.75),
            boundary_region: srgb8_a(0x84, 0x7d, 0x92, 0.5),
            place_label: srgb8(0x5c, 0x57, 0x62),
            country_label: srgb8(0x4d, 0x48, 0x58),
            water_label: srgb8(0x4f, 0x61, 0x78),
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
            tmz: pair(MAUVE_GREY, 0.04, 0.75),
            danger: pair(TERRACOTTA, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark symbols on grey paper; navaid leans the indigo primary.
            airport: srgb(64, 62, 70, 1.0),
            glider: srgb(136, 104, 46, 1.0),
            navaid: srgb(76, 86, 124, 1.0),
            reporting: srgb(56, 56, 64, 1.0),
            obstacle: srgb(164, 68, 60, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 40, 46, 1.0),
        },
        weather: WeatherTheme {
            // Deepened for paper, semantics intact: VFR from the cyan-green
            // #54c981 (the olive green is too yellow), MVFR from the indigo
            // primary, IFR from the dusty red, LIFR from the mauve magenta.
            vfr: srgb(24, 148, 84, 1.0),
            mvfr: srgb(58, 92, 196, 1.0),
            ifr: srgb(192, 52, 48, 1.0),
            lifr: srgb(152, 52, 140, 1.0),
            sigmet: srgb(192, 104, 38, 0.5),
            cloud_cover: Colormap::new(&[
                stop(10.0, (128, 131, 138), 0.0),
                stop(40.0, (134, 137, 144), 0.12),
                stop(75.0, (148, 151, 158), 0.26),
                stop(100.0, (162, 165, 172), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (66, 112, 186), 0.0),
                stop(1.0, (66, 112, 186), 0.34),
                stop(5.0, (48, 156, 176), 0.44),
                stop(20.0, (186, 156, 48), 0.52),
                stop(50.0, (186, 60, 50), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (194, 130, 42), 0.0),
                stop(5.0, (188, 118, 36), 0.36),
                stop(15.0, (172, 56, 46), 0.54),
            ]),
        },
        // Route: deep indigo-violet (primary family, vivified); conflicts in
        // brick red.
        route: RouteTheme {
            line: srgb(104, 80, 196, 1.0),
            line_conflict: srgb(202, 52, 56, 1.0),
            handle_fill: srgb(104, 80, 196, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(104, 80, 196, 0.12),
        },
        labels: LabelTheme {
            // Dark grey-violet text (#383a42 family) over a light halo.
            text: srgb(50, 50, 58, 0.95),
            halo: srgb(246, 245, 248, 0.85),
        },
        // Mauve-grey relief: shadows toward the lavender undertone, lights
        // toward the panel white.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x58, 0x52, 0x60),
            light_tint: tint_from_srgb8(0xf0, 0xee, 0xf2),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

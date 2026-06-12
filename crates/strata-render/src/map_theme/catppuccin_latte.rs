//! "Catppuccin Latte" — map theme paired with the Catppuccin Latte UI
//! theme.
//!
//! The light Catppuccin flavor: cool blue-gray paper instead of Pastel
//! Light's warm cream. The ground sits just below the UI `background`
//! `#E5E9EF` (panels `#dce0e8`), so map and chrome share Latte's cool
//! paper; features run a notch darker, water a clearly cooler blue-gray.
//! Airspace anchors: controlled = Latte blue `#78acdc`/primary `#7287fd`
//! darkened to steel, CTR/restricted/prohibited = the Latte red family as
//! a deep rose, danger = Latte orange (peach `#fe640b`, muted) as
//! terracotta, glider/para = yellow `#df8e1d` as ochre. LIFR leans Latte's
//! violet magenta; dark text over a light halo; cool-gray relief.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Latte accent set, darkened to mid-tones for paper.
const STEEL: (u8, u8, u8) = (72, 104, 158); // blue #78acdc / #7287fd — controlled
const FAINT_STEEL: (u8, u8, u8) = (104, 128, 168); // class E/F band
const ROSE: (u8, u8, u8) = (178, 76, 84); // red family — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (182, 104, 70); // peach #fe640b, muted — danger
const GREY: (u8, u8, u8) = (104, 106, 122); // TMZ
const OCHRE: (u8, u8, u8) = (162, 120, 52); // yellow #df8e1d — glider / para
const NEUTRAL: (u8, u8, u8) = (108, 110, 120);

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
    // Cool blue-gray paper, just under the Latte UI background.
    let land = srgb8(0xe2, 0xe5, 0xec);
    MapTheme {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated blue so water reads immediately.
            water: srgb8(0xb1, 0xc0, 0xd6),
            waterway: srgb8(0x7f, 0x93, 0xb5),
            // Landcover: a whisker around the paper, staying in Latte's cool
            // base/mantle family — no green-yellow bias, blue stays high so
            // the tile slab keeps the chrome's lavender-cool cast.
            forest: srgb8(0xd5, 0xdb, 0xdd),
            grass: srgb8(0xda, 0xdf, 0xe2),
            farmland: srgb8(0xe0, 0xe1, 0xe6),
            barren: srgb8(0xe0, 0xdd, 0xe1),
            glacier: srgb8(0xee, 0xf1, 0xf6),
            park: srgb8(0xd7, 0xdd, 0xe0),
            // Urban fabric reads as a slightly darker, grayer patch.
            urban: srgb8(0xdc, 0xd9, 0xde),
            urban_dense: srgb8(0xd2, 0xcf, 0xd6),
            military: srgb8(0xde, 0xd4, 0xd6),
            aerodrome: srgb8(0xd8, 0xda, 0xe6),
            // Roads darker than paper: a muted tan motorway, cool grays below.
            road_highway: srgb8(0xb2, 0x92, 0x63),
            road_major: srgb8(0x92, 0x95, 0xa4),
            road_medium: srgb8(0xa8, 0xab, 0xb8),
            road_minor: srgb8(0xbb, 0xbe, 0xca),
            path: srgb8(0xc9, 0xcc, 0xd6),
            rail: srgb8_a(0x9b, 0x9d, 0xac, 0.9),
            // Dark blue-grays, clearly legible on the cool paper.
            boundary_country: srgb8_a(0x62, 0x64, 0x7e, 0.75),
            boundary_region: srgb8_a(0x7e, 0x80, 0x99, 0.5),
            place_label: srgb8(0x58, 0x5b, 0x72),
            country_label: srgb8(0x4a, 0x4d, 0x66),
            water_label: srgb8(0x50, 0x64, 0x7e),
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
            tmz: pair(GREY, 0.04, 0.75),
            danger: pair(TERRACOTTA, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark blue-gray symbols (Latte foreground family) on paper.
            airport: srgb(62, 64, 84, 1.0),
            glider: srgb(138, 100, 40, 1.0),
            navaid: srgb(80, 94, 118, 1.0),
            reporting: srgb(54, 56, 70, 1.0),
            obstacle: srgb(164, 68, 58, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 42, 50, 1.0),
        },
        weather: WeatherTheme {
            // Latte's saturated accents, calm enough for paper.
            vfr: srgb(52, 152, 78, 1.0),
            mvfr: srgb(44, 98, 206, 1.0),
            ifr: srgb(198, 46, 58, 1.0),
            lifr: srgb(158, 52, 170, 1.0),
            sigmet: srgb(200, 98, 30, 0.5),
            // Gridded overlays, muted; clouds lean a darker cool gray so the
            // ramp still reads on the light paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (126, 130, 142), 0.0),
                stop(40.0, (132, 136, 148), 0.12),
                stop(75.0, (146, 150, 162), 0.26),
                stop(100.0, (160, 164, 176), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (62, 110, 192), 0.0),
                stop(1.0, (62, 110, 192), 0.34),
                stop(5.0, (44, 154, 182), 0.44),
                stop(20.0, (192, 156, 48), 0.52),
                stop(50.0, (192, 58, 48), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (200, 134, 40), 0.0),
                stop(5.0, (194, 122, 34), 0.36),
                stop(15.0, (178, 52, 44), 0.54),
            ]),
        },
        // Route: latte mauve #8839ef — vivid on the light ground; conflicts in
        // latte red.
        route: RouteTheme {
            line: srgb(136, 57, 239, 1.0),
            line_conflict: srgb(210, 56, 70, 1.0),
            handle_fill: srgb(136, 57, 239, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(136, 57, 239, 0.12),
        },
        labels: LabelTheme {
            // Latte foreground #4c4f69 over a cool light halo.
            text: srgb(70, 72, 98, 0.95),
            halo: srgb(242, 244, 248, 0.85),
        },
        // Light-theme relief: cool gray-violet shadows, paper lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x52, 0x50, 0x5a),
            light_tint: tint_from_srgb8(0xee, 0xf0, 0xf6),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

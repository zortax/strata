//! "Pastel Light" — a light map: warm paper-like land with basemap features
//! drawn slightly darker so they read on light ground (visibly darker
//! water, legible borders and roads), airspaces in muted pastels with
//! enough contrast against paper, dark label text over a light halo. Not
//! washed out, not harsh.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Pastel airspace hues, darkened to mid-tones so they read on paper.
const STEEL: (u8, u8, u8) = (74, 102, 148); // slate/steel blue — controlled
const FAINT_STEEL: (u8, u8, u8) = (105, 128, 162); // class E/F band
const ROSE: (u8, u8, u8) = (172, 84, 92); // dusty rose — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (176, 100, 80); // danger areas
const GREY: (u8, u8, u8) = (105, 108, 118); // TMZ
const OCHRE: (u8, u8, u8) = (158, 126, 66); // glider / para-jump (muted sand)
const NEUTRAL: (u8, u8, u8) = (110, 110, 116);

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
    // Warm paper.
    let land = srgb8(0xec, 0xe7, 0xdc);
    MapTheme {
        id: "pastel-light",
        name: "Pastel Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated blue so water reads immediately.
            water: srgb8(0xb0, 0xc2, 0xcd),
            waterway: srgb8(0x7d, 0x93, 0xab),
            forest: srgb8(0xd2, 0xda, 0xc6),
            grass: srgb8(0xdc, 0xe1, 0xcd),
            farmland: srgb8(0xe6, 0xe0, 0xcb),
            barren: srgb8(0xe1, 0xd8, 0xc3),
            glacier: srgb8(0xee, 0xf1, 0xf4),
            park: srgb8(0xd7, 0xdf, 0xca),
            // Urban fabric reads as a slightly darker, greyer patch.
            urban: srgb8(0xdc, 0xd5, 0xd1),
            urban_dense: srgb8(0xd1, 0xca, 0xc8),
            military: srgb8(0xdc, 0xd0, 0xcc),
            aerodrome: srgb8(0xd8, 0xd7, 0xdf),
            // Roads darker than paper so they stay traceable.
            road_highway: srgb8(0xb0, 0x8d, 0x52),
            road_major: srgb8(0x8e, 0x8c, 0x96),
            road_medium: srgb8(0xa5, 0xa3, 0xab),
            road_minor: srgb8(0xb8, 0xb6, 0xbc),
            path: srgb8(0xc6, 0xc4, 0xc8),
            rail: srgb8_a(0x99, 0x97, 0xa0, 0.9),
            // Dark grey-violet, clearly legible on paper.
            boundary_country: srgb8_a(0x6b, 0x64, 0x76, 0.75),
            boundary_region: srgb8_a(0x83, 0x7c, 0x90, 0.5),
            place_label: srgb8(0x5a, 0x55, 0x60),
            country_label: srgb8(0x4c, 0x47, 0x57),
            water_label: srgb8(0x4e, 0x60, 0x76),
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
            // Dark symbols on light ground.
            airport: srgb(66, 62, 72, 1.0),
            glider: srgb(140, 108, 48, 1.0),
            navaid: srgb(84, 94, 110, 1.0),
            reporting: srgb(58, 58, 66, 1.0),
            obstacle: srgb(160, 70, 62, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            // Dark rim still works on paper: the tinted rim separates the
            // dot from both the light ground and the airport symbol.
            weather_outline: srgb(40, 40, 46, 1.0),
        },
        weather: WeatherTheme {
            // Saturated enough to read on paper, still calm.
            vfr: srgb(0, 150, 80, 1.0),
            mvfr: srgb(36, 100, 204, 1.0),
            ifr: srgb(198, 44, 54, 1.0),
            lifr: srgb(168, 44, 168, 1.0),
            sigmet: srgb(198, 96, 34, 0.5),
            // Gridded overlays, muted; clouds lean darker gray so the ramp
            // still reads on paper-light land.
            cloud_cover: Colormap::new(&[
                stop(10.0, (128, 132, 140), 0.0),
                stop(40.0, (134, 138, 146), 0.12),
                stop(75.0, (148, 152, 160), 0.26),
                stop(100.0, (162, 166, 174), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (70, 116, 188), 0.0),
                stop(1.0, (70, 116, 188), 0.34),
                stop(5.0, (52, 158, 178), 0.44),
                stop(20.0, (188, 158, 52), 0.52),
                stop(50.0, (188, 62, 52), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (196, 132, 44), 0.0),
                stop(5.0, (190, 120, 38), 0.36),
                stop(15.0, (174, 56, 46), 0.54),
            ]),
        },
        // Route: deep magenta-plum — vivid on warm paper, far from the muted
        // airspace pastels; conflicts in brick red.
        route: RouteTheme {
            line: srgb(150, 60, 130, 1.0),
            line_conflict: srgb(196, 50, 56, 1.0),
            handle_fill: srgb(150, 60, 130, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(150, 60, 130, 0.12),
        },
        labels: LabelTheme {
            // Dark text over a light halo so idents read over colored fills.
            text: srgb(52, 50, 58, 0.95),
            halo: srgb(248, 246, 240, 0.85),
        },
        // Light-theme relief: shadows toward grey-brown, lights toward paper.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x5a, 0x50, 0x46),
            light_tint: tint_from_srgb8(0xf2, 0xee, 0xe4),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

//! "Molokai Light" — map theme paired with the Molokai Light UI theme
//! (tomasr/molokai, light variant: warm pink-white chrome with a vivid
//! pink primary and fruity accents).
//!
//! Palette rationale: the ground is warm pink-tinged paper just below the
//! UI background `#FEFAF9` (land `#F4EEEB`, kin to the title bar `#e8e3e0`;
//! the urban tint lands exactly on the UI secondary `#E4DEDA`). Airspace
//! anchors come from the Molokai Light accents: the CTR / restricted /
//! prohibited family carries the signature pink primary (`#E14774`,
//! deepened to a chart mid-tone), controlled airspace the info teal
//! (`#1c8ca8`, the palette's only calm cool), danger the orange
//! (`#e16032`), glider/para the yellow (`#ccac0a`, deepened to ochre) and
//! TMZ the violet (`#7058be`, grayed). The motorway keeps a warm ochre
//! hue; weather stays semantic with a deepened lime VFR, violet-blue MVFR,
//! orange-red IFR and a pink-violet LIFR.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), mid-toned so they read on warm paper.
const PINK: (u8, u8, u8) = (196, 60, 102); // primary #E14774 deepened — CTR / ED-R / ED-P
const TEAL: (u8, u8, u8) = (40, 118, 140); // info teal #1c8ca8 — controlled
const FAINT_TEAL: (u8, u8, u8) = (92, 138, 156); // class E/F band
const ORANGE: (u8, u8, u8) = (198, 102, 62); // #e16032 muted — danger areas
const OCHRE: (u8, u8, u8) = (160, 132, 36); // #ccac0a deepened — glider / para
const VIOLET_GREY: (u8, u8, u8) = (110, 96, 142); // #7058be grayed — TMZ
const NEUTRAL: (u8, u8, u8) = (112, 110, 114); // muted_foreground family

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
    // Warm pink-tinged paper just below the UI background #FEFAF9.
    let land = srgb8(0xf4, 0xee, 0xeb);
    MapTheme {
        id: "molokai-light",
        name: "Molokai Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Desaturated blue, visibly darker than the warm paper.
            water: srgb8(0xb2, 0xc8, 0xd0),
            waterway: srgb8(0x78, 0x96, 0xa8),
            // Landcover: warm whiskers around the paper tone.
            forest: srgb8(0xd8, 0xe0, 0xd2),
            grass: srgb8(0xe2, 0xe7, 0xd8),
            farmland: srgb8(0xf0, 0xe8, 0xd8),
            barren: srgb8(0xea, 0xe0, 0xd1),
            glacier: srgb8(0xf0, 0xf4, 0xf6),
            park: srgb8(0xdd, 0xe4, 0xd5),
            // Urban lands exactly on the UI secondary #E4DEDA.
            urban: srgb8(0xe4, 0xde, 0xda),
            urban_dense: srgb8(0xd9, 0xd2, 0xce),
            military: srgb8(0xe4, 0xd8, 0xd4),
            aerodrome: srgb8(0xe0, 0xde, 0xe6),
            // Roads slightly darker than ground; the motorway keeps the
            // theme's warm ochre hue.
            road_highway: srgb8(0xb8, 0x9c, 0x58),
            road_major: srgb8(0x94, 0x90, 0x96),
            road_medium: srgb8(0xaa, 0xa6, 0xac),
            road_minor: srgb8(0xc0, 0xbc, 0xc1),
            path: srgb8(0xce, 0xca, 0xce),
            rail: srgb8_a(0xa0, 0x9c, 0xa2, 0.9),
            // Violet-gray boundaries (from the violet accent), clearly
            // legible on paper.
            boundary_country: srgb8_a(0x6e, 0x62, 0x80, 0.75),
            boundary_region: srgb8_a(0x86, 0x7c, 0x96, 0.5),
            place_label: srgb8(0x5e, 0x58, 0x60),
            country_label: srgb8(0x4e, 0x48, 0x54),
            water_label: srgb8(0x46, 0x62, 0x74),
        },
        airspace: AirspaceTheme {
            class_a: pair(TEAL, 0.07, 0.75),
            class_b: pair(TEAL, 0.07, 0.75),
            class_c: pair(TEAL, 0.1, 0.85),
            class_d: pair(TEAL, 0.07, 0.75),
            class_e: pair(FAINT_TEAL, 0.035, 0.45),
            class_f: pair(FAINT_TEAL, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(PINK, 0.11, 0.85),
            rmz: pair(TEAL, 0.05, 0.7),
            tmz: pair(VIOLET_GREY, 0.04, 0.75),
            danger: pair(ORANGE, 0.08, 0.75),
            restricted: pair(PINK, 0.15, 0.85),
            prohibited: pair(PINK, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark symbols on light ground.
            airport: srgb(64, 60, 64, 1.0),
            glider: srgb(142, 116, 40, 1.0),
            navaid: srgb(52, 100, 118, 1.0),
            reporting: srgb(58, 56, 60, 1.0),
            obstacle: srgb(186, 84, 62, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(44, 42, 46, 1.0),
        },
        weather: WeatherTheme {
            // Molokai accents deepened to read on paper: lime VFR,
            // violet-blue MVFR, orange-red IFR, pink-violet LIFR.
            vfr: srgb(70, 158, 40, 1.0),
            mvfr: srgb(62, 102, 196, 1.0),
            ifr: srgb(204, 62, 50, 1.0),
            lifr: srgb(170, 54, 160, 1.0),
            sigmet: srgb(206, 96, 40, 0.5),
            // Gridded overlays: darker grays so the ramps read on paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (130, 134, 140), 0.0),
                stop(40.0, (136, 140, 146), 0.12),
                stop(75.0, (150, 154, 160), 0.26),
                stop(100.0, (164, 168, 174), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (64, 118, 190), 0.0),
                stop(1.0, (64, 118, 190), 0.34),
                stop(5.0, (40, 160, 170), 0.44),
                stop(20.0, (192, 160, 48), 0.52),
                stop(50.0, (192, 64, 50), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (200, 134, 44), 0.0),
                stop(5.0, (194, 122, 38), 0.36),
                stop(15.0, (180, 58, 46), 0.54),
            ]),
        },
        // Route: deep violet (#7058be vivified) — vivid on the warm paper;
        // conflicts in molokai pink-red.
        route: RouteTheme {
            line: srgb(112, 88, 190, 1.0),
            line_conflict: srgb(216, 36, 80, 1.0),
            handle_fill: srgb(112, 88, 190, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(112, 88, 190, 0.12),
        },
        labels: LabelTheme {
            // Near-black text (UI foreground #0a0a0a softened) over a warm
            // paper halo.
            text: srgb(56, 52, 56, 0.95),
            halo: srgb(252, 249, 246, 0.85),
        },
        // Warm relief: shadows toward the brown of the chrome borders,
        // lights toward the paper.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x5c, 0x4e, 0x48),
            light_tint: tint_from_srgb8(0xf6, 0xf0, 0xea),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

//! "Ayu Light" — map theme paired with the Ayu Light UI theme.
//!
//! Ayu Light's chrome is a clean, faintly cool near-white (`background
//! #FCFCFC`, panels `#F3F4F5` / `#ECECED`) with cyan-blue (`#55b4d3`),
//! signature orange (`#FF9940` / `#F1AD49`), green (`#85b304`), violet
//! (`#9371f0`) and soft-red (`#F07171`) accents. The basemap is a cool
//! paper one notch below the window background, features a step darker
//! than ground; the motorway keeps ayu's orange-tan signature while the
//! rest of the ramp stays cool gray. Airspaces darken the accents to
//! mid-tones: cyan-steel for controlled airspace, ayu red for
//! CTR/ED-R/ED-P, orange for danger, ochre for glider/para and a
//! violet-gray TMZ from the violet accent.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues, darkened to mid-tones so they read on cool paper.
const STEEL: (u8, u8, u8) = (54, 118, 152); // from #55b4d3 — controlled
const FAINT_STEEL: (u8, u8, u8) = (96, 140, 166); // class E/F band
const ROSE: (u8, u8, u8) = (198, 84, 88); // from #F07171 — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (192, 112, 56); // from #FF9940 — danger areas
const VIOLET_GREY: (u8, u8, u8) = (110, 104, 130); // from #9371f0 — TMZ
const OCHRE: (u8, u8, u8) = (178, 128, 52); // from #F1AD49 — glider / para-jump
const NEUTRAL: (u8, u8, u8) = (108, 112, 118);

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
    // Cool paper, one notch below the #FCFCFC window background.
    let land = srgb8(0xee, 0xf0, 0xf1);
    MapTheme {
        id: "ayu-light",
        name: "Ayu Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated cyan-blue (ayu's water-blue family).
            water: srgb8(0xaf, 0xcb, 0xd6),
            waterway: srgb8(0x70, 0x95, 0xa8),
            // Landcover: gentle desaturated tints a step around the paper.
            forest: srgb8(0xd8, 0xe2, 0xd7),
            grass: srgb8(0xe0, 0xe8, 0xdd),
            farmland: srgb8(0xeb, 0xeb, 0xdf),
            barren: srgb8(0xe4, 0xe1, 0xd8),
            glacier: srgb8(0xf0, 0xf4, 0xf7),
            park: srgb8(0xdc, 0xe5, 0xd9),
            urban: srgb8(0xe0, 0xe0, 0xe2),
            urban_dense: srgb8(0xd6, 0xd6, 0xd9),
            military: srgb8(0xe0, 0xd9, 0xd6),
            aerodrome: srgb8(0xdb, 0xdd, 0xe4),
            // Roads darker than paper; the motorway carries ayu's orange-tan
            // signature, the rest of the ramp stays cool gray.
            road_highway: srgb8(0xc8, 0x9a, 0x5e),
            road_major: srgb8(0x8f, 0x95, 0x9b),
            road_medium: srgb8(0xa7, 0xad, 0xb3),
            road_minor: srgb8(0xbb, 0xc1, 0xc6),
            path: srgb8(0xcc, 0xd1, 0xd5),
            rail: srgb8_a(0x9a, 0x9e, 0xa6, 0.9),
            // Cool slate grays from ayu's foreground (#5c6166) family.
            boundary_country: srgb8_a(0x5e, 0x64, 0x6c, 0.75),
            boundary_region: srgb8_a(0x7b, 0x81, 0x8a, 0.5),
            place_label: srgb8(0x56, 0x5b, 0x62),
            country_label: srgb8(0x47, 0x4c, 0x54),
            water_label: srgb8(0x3f, 0x6a, 0x7d),
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
            tmz: pair(VIOLET_GREY, 0.04, 0.75),
            danger: pair(ORANGE, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark slate symbols on cool paper.
            airport: srgb(62, 66, 72, 1.0),
            glider: srgb(146, 104, 40, 1.0),
            navaid: srgb(62, 100, 124, 1.0),
            reporting: srgb(52, 56, 62, 1.0),
            obstacle: srgb(180, 72, 66, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 44, 50, 1.0),
        },
        weather: WeatherTheme {
            // Ayu accents, deepened to read on paper; semantics intact.
            vfr: srgb(96, 158, 32, 1.0),
            mvfr: srgb(40, 124, 186, 1.0),
            ifr: srgb(210, 52, 58, 1.0),
            lifr: srgb(146, 60, 196, 1.0),
            sigmet: srgb(204, 110, 40, 0.5),
            // Gridded overlays: muted, clouds a cool darker gray for paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (126, 131, 138), 0.0),
                stop(40.0, (132, 137, 144), 0.12),
                stop(75.0, (146, 151, 158), 0.26),
                stop(100.0, (160, 165, 172), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (56, 118, 184), 0.0),
                stop(1.0, (56, 118, 184), 0.34),
                stop(5.0, (44, 154, 172), 0.44),
                stop(20.0, (190, 156, 48), 0.52),
                stop(50.0, (192, 58, 50), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (198, 130, 42), 0.0),
                stop(5.0, (192, 118, 36), 0.36),
                stop(15.0, (176, 54, 44), 0.54),
            ]),
        },
        // Route: deep ayu violet (#9371f0 deepened) — vivid on the cool paper;
        // conflicts in brick red.
        route: RouteTheme {
            line: srgb(118, 70, 200, 1.0),
            line_conflict: srgb(200, 48, 52, 1.0),
            handle_fill: srgb(118, 70, 200, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(118, 70, 200, 0.12),
        },
        labels: LabelTheme {
            // Dark slate text (from #5c6166) over a near-white halo.
            text: srgb(74, 78, 84, 0.95),
            halo: srgb(250, 251, 252, 0.85),
        },
        // Relief: cool slate shadows, paper-white lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x4e, 0x52, 0x58),
            light_tint: tint_from_srgb8(0xf4, 0xf6, 0xf8),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

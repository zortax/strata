//! "Everforest Light" — map theme paired with the Everforest Light UI theme.
//!
//! Palette rationale: warm cream paper just below the UI background
//! (`#FEFCEE` window, `#f1f0e2` panels) at `#f1eedd`, with the landcover
//! band leaning gently green (it is a forest theme) and water/waterways in
//! the theme's teal. Airspace identity from the Everforest accents darkened
//! to mid-tones for paper: controlled airspace in teal (`base.blue
//! #7fbbb3`), CTR/restricted/prohibited in the soft red (`base.red
//! #e67e80`), danger in the primary orange (`#e69875`), glider/para in
//! ochre from `base.yellow #dbbc7f`, LIFR from `base.magenta #d699b6`.
//! Dark green-gray label text over a cream halo; relief lights toward
//! paper, shadows toward a mossy gray-brown.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Everforest accents, darkened to mid-tones so they read on paper.
const TEAL: (u8, u8, u8) = (58, 118, 110); // base.blue — controlled airspace
const FAINT_TEAL: (u8, u8, u8) = (96, 138, 132); // class E/F band
const RED: (u8, u8, u8) = (184, 90, 92); // base.red — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (186, 110, 76); // primary — danger areas
const GRAY_GREEN: (u8, u8, u8) = (104, 112, 108); // muted fg — TMZ
const OCHRE: (u8, u8, u8) = (160, 128, 64); // base.yellow — glider / para
const NEUTRAL: (u8, u8, u8) = (110, 114, 110);

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
    // Warm cream paper, a notch below the UI's `#FEFCEE` background.
    let land = srgb8(0xf1, 0xee, 0xdd);
    MapTheme {
        id: "everforest-light",
        name: "Everforest Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker desaturated teal so water reads immediately.
            water: srgb8(0xaa, 0xca, 0xc4),
            waterway: srgb8(0x6f, 0x9b, 0x95),
            // Landcover a notch darker than paper, gently green.
            forest: srgb8(0xd4, 0xdf, 0xc2),
            grass: srgb8(0xde, 0xe5, 0xc9),
            farmland: srgb8(0xea, 0xe6, 0xc8),
            barren: srgb8(0xe5, 0xdd, 0xc0),
            glacier: srgb8(0xee, 0xf2, 0xf0),
            park: srgb8(0xd9, 0xe2, 0xc5),
            // Urban fabric as slightly darker, grayer patches.
            urban: srgb8(0xe0, 0xda, 0xcc),
            urban_dense: srgb8(0xd5, 0xcf, 0xc3),
            military: srgb8(0xe0, 0xd5, 0xc6),
            aerodrome: srgb8(0xdd, 0xdd, 0xd8),
            // Roads darker than paper; motorway keeps a warm primary-orange
            // tan, the rest stay green-gray.
            road_highway: srgb8(0xb8, 0x90, 0x5e),
            road_major: srgb8(0x96, 0x9b, 0x93),
            road_medium: srgb8(0xab, 0xb0, 0xa7),
            road_minor: srgb8(0xbe, 0xc2, 0xb9),
            path: srgb8(0xcc, 0xcf, 0xc6),
            rail: srgb8_a(0x9e, 0xa3, 0x9b, 0.9),
            // Dark green-grays, clearly legible on paper.
            boundary_country: srgb8_a(0x60, 0x6e, 0x68, 0.75),
            boundary_region: srgb8_a(0x7e, 0x8b, 0x84, 0.5),
            place_label: srgb8(0x5c, 0x66, 0x60),
            country_label: srgb8(0x4b, 0x57, 0x52),
            water_label: srgb8(0x4a, 0x6e, 0x68),
        },
        airspace: AirspaceTheme {
            class_a: pair(TEAL, 0.07, 0.75),
            class_b: pair(TEAL, 0.07, 0.75),
            class_c: pair(TEAL, 0.1, 0.85),
            class_d: pair(TEAL, 0.07, 0.75),
            class_e: pair(FAINT_TEAL, 0.035, 0.45),
            class_f: pair(FAINT_TEAL, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(RED, 0.11, 0.85),
            rmz: pair(TEAL, 0.05, 0.7),
            tmz: pair(GRAY_GREEN, 0.04, 0.75),
            danger: pair(ORANGE, 0.08, 0.75),
            restricted: pair(RED, 0.15, 0.85),
            prohibited: pair(RED, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark green-gray symbols on cream ground.
            airport: srgb(62, 68, 66, 1.0),
            glider: srgb(138, 108, 46, 1.0),
            navaid: srgb(70, 100, 94, 1.0),
            reporting: srgb(56, 62, 60, 1.0),
            obstacle: srgb(170, 76, 70, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 44, 42, 1.0),
        },
        weather: WeatherTheme {
            // Saturated enough to read on cream, tuned to the accent set.
            vfr: srgb(88, 150, 62, 1.0),
            mvfr: srgb(44, 108, 196, 1.0),
            ifr: srgb(200, 58, 62, 1.0),
            lifr: srgb(170, 64, 150, 1.0),
            sigmet: srgb(200, 110, 40, 0.5),
            // Gridded overlays lean darker gray so they read on paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (126, 132, 128), 0.0),
                stop(40.0, (132, 138, 134), 0.12),
                stop(75.0, (146, 152, 148), 0.26),
                stop(100.0, (160, 166, 162), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 112, 180), 0.0),
                stop(1.0, (60, 112, 180), 0.34),
                stop(5.0, (48, 152, 160), 0.44),
                stop(20.0, (180, 150, 52), 0.52),
                stop(50.0, (180, 64, 52), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (190, 128, 46), 0.0),
                stop(5.0, (184, 116, 40), 0.36),
                stop(15.0, (170, 58, 48), 0.54),
            ]),
        },
        // Route: deep plum (purple family) — vivid on the cream paper;
        // conflicts in everforest red.
        route: RouteTheme {
            line: srgb(152, 58, 124, 1.0),
            line_conflict: srgb(192, 60, 54, 1.0),
            handle_fill: srgb(152, 58, 124, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(152, 58, 124, 0.12),
        },
        labels: LabelTheme {
            // The UI foreground (#5F6D75) darkened for contrast, over a
            // cream halo so idents read on colored fills.
            text: srgb(60, 72, 78, 0.95),
            halo: srgb(252, 250, 236, 0.85),
        },
        // Relief: mossy gray-brown shadows, lights toward paper.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x58, 0x55, 0x46),
            light_tint: tint_from_srgb8(0xf4, 0xf2, 0xe0),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

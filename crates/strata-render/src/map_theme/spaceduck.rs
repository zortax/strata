//! "Spaceduck" — map theme paired with the Spaceduck UI theme (deep-space
//! blue-violet: `background #0F111B`, `title_bar #141724`, primary cyan
//! `#089CC5`, muted slate `#4b6479`, cream foreground `#ecf0c1`).
//!
//! Palette rationale: the ground is a blue-violet near-black just below
//! the UI background (blue a dozen channels above red, like the chrome),
//! with the compressed road ramp keeping that lean. Water is darker and
//! bluer; the waterway stroke nods to the deep-cyan panel `#003a5b`.
//! Airspaces carry the Spaceduck accents: controlled airspace in the
//! signature cyan `#089CC5` (desaturated to steel), CTR / restricted /
//! prohibited in the dusty rose magenta `#C86D8C`, danger areas in the
//! orange-red `#e33400` calmed to ember, glider/para in olive-sand from
//! the yellow `#b89c00`, TMZ/other in slate greys. Weather: VFR from the
//! green `#5ccc96`, MVFR cyan-blue, IFR from the orange-red, LIFR a more
//! magenta cut of the rose. Symbols and labels lean the duck-cream
//! `#ecf0c1`; terrain relief is tinted cool blue-violet.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) from the Spaceduck accent set.
const CYAN: (u8, u8, u8) = (70, 150, 185); // primary #089CC5 — controlled
const FAINT_CYAN: (u8, u8, u8) = (104, 148, 172); // class E/F band
const ROSE: (u8, u8, u8) = (198, 110, 138); // magenta #C86D8C — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (212, 110, 76); // red #e33400 — danger areas
const MAUVE_GREY: (u8, u8, u8) = (146, 146, 158); // TMZ
const SAND: (u8, u8, u8) = (190, 165, 90); // yellow #b89c00 — glider / para-jump
const NEUTRAL: (u8, u8, u8) = (128, 134, 144); // slate

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
    // Just below the UI background #0F111B: blue-violet near-black.
    let land = srgb8(0x0d, 0x0f, 0x19);
    MapTheme {
        id: "spaceduck",
        name: "Spaceduck",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Darker still, leaning further blue.
            water: srgb8(0x0a, 0x0d, 0x18),
            waterway: srgb8(0x14, 0x26, 0x38),
            // Landcover: blue-violet band a whisker around `land`.
            forest: srgb8(0x0b, 0x0e, 0x17),
            grass: srgb8(0x0c, 0x0f, 0x18),
            farmland: srgb8(0x0e, 0x10, 0x1a),
            barren: srgb8(0x10, 0x11, 0x1b),
            glacier: srgb8(0x12, 0x14, 0x20),
            park: srgb8(0x0c, 0x0f, 0x18),
            urban: srgb8(0x12, 0x14, 0x1e),
            urban_dense: srgb8(0x15, 0x17, 0x21),
            military: srgb8(0x11, 0x12, 0x1c),
            aerodrome: srgb8(0x13, 0x14, 0x21),
            // Compressed road ramp keeping the blue lean (+2..+12).
            road_highway: srgb8(0x19, 0x1b, 0x25),
            road_major: srgb8(0x16, 0x18, 0x22),
            road_medium: srgb8(0x13, 0x15, 0x1f),
            road_minor: srgb8(0x11, 0x13, 0x1d),
            path: srgb8(0x0f, 0x11, 0x1b),
            rail: srgb8_a(0x16, 0x17, 0x22, 0.85),
            // Slate-cyan greys from the muted foreground #4b6479.
            boundary_country: srgb8_a(0x52, 0x62, 0x72, 0.55),
            boundary_region: srgb8_a(0x3c, 0x48, 0x56, 0.30),
            place_label: srgb8(0x4f, 0x5c, 0x6b),
            country_label: srgb8(0x5c, 0x6a, 0x79),
            water_label: srgb8(0x3c, 0x55, 0x68),
        },
        airspace: AirspaceTheme {
            class_a: pair(CYAN, 0.05, 0.72),
            class_b: pair(CYAN, 0.05, 0.72),
            class_c: pair(CYAN, 0.07, 0.78),
            class_d: pair(CYAN, 0.05, 0.7),
            class_e: pair(FAINT_CYAN, 0.02, 0.35),
            class_f: pair(FAINT_CYAN, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(CYAN, 0.035, 0.68),
            tmz: pair(MAUVE_GREY, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Duck-cream symbols (from #ecf0c1) on the space-blue ground.
            airport: srgb(208, 210, 186, 1.0),
            glider: srgb(198, 176, 108, 1.0),
            navaid: srgb(118, 158, 178, 1.0),
            reporting: srgb(222, 226, 202, 1.0),
            obstacle: srgb(212, 108, 84, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(20, 24, 34, 1.0),
        },
        weather: WeatherTheme {
            // Semantic categories from the Spaceduck accents.
            vfr: srgb(92, 204, 150, 1.0), // green #5ccc96
            mvfr: srgb(80, 140, 210, 1.0), // blue #00a3cc pushed past cyan
            ifr: srgb(218, 80, 60, 1.0),  // orange-red #e33400
            lifr: srgb(192, 90, 184, 1.0), // rose #C86D8C pushed to magenta
            sigmet: srgb(216, 120, 60, 0.45),
            // Gridded overlays: muted, a hair cool.
            cloud_cover: Colormap::new(&[
                stop(10.0, (146, 150, 158), 0.0),
                stop(40.0, (154, 158, 166), 0.12),
                stop(75.0, (182, 186, 194), 0.28),
                stop(100.0, (208, 212, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (70, 130, 200), 0.0),
                stop(1.0, (70, 130, 200), 0.32),
                stop(5.0, (60, 180, 195), 0.42),
                stop(20.0, (205, 180, 85), 0.5),
                stop(50.0, (210, 90, 70), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (212, 156, 80), 0.0),
                stop(5.0, (208, 142, 70), 0.32),
                stop(15.0, (205, 90, 70), 0.5),
            ]),
        },
        // Route: bright gold (#b89c00 vivified) against the space-indigo
        // ground; conflicts in spaceduck red-rose.
        route: RouteTheme {
            line: srgb(240, 200, 70, 1.0),
            line_conflict: srgb(230, 80, 96, 1.0),
            handle_fill: srgb(240, 200, 70, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(240, 200, 70, 0.12),
        },
        labels: LabelTheme {
            // Duck-cream #ecf0c1, toned down a notch.
            text: srgb(216, 218, 196, 0.95),
            halo: [0.0; 4],
        },
        // Cool blue-violet relief to match the space ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x12, 0x12, 0x1e),
            light_tint: tint_from_srgb8(0x78, 0x7a, 0x88),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

//! "Asciinema" — map theme paired with the Asciinema UI theme
//! (terminal-recorder neutral dark: `background #121314`, `panel #181919`,
//! `title_bar #1a1b1c`, foreground `#cccccc`).
//!
//! Palette rationale: the ground is a near-black *neutral* band one step
//! below the UI background (`#111213`, faintly cool like the chrome), with
//! the compressed Oldworld road ramp on top — the map reads as the deepest
//! layer under the gray panels. The airspace layer carries the terminal
//! accent set: controlled airspace in the signature cyan (`#26b0d7`,
//! desaturated to a steel-cyan), CTR / restricted / prohibited in the
//! raspberry danger red (`#dd3c69`), danger areas a warmer ember between
//! red and yellow, glider/para in sand from `#ddaf3c`. Weather keeps its
//! semantics with the theme's green `#4ebf22` (VFR), cyan-blue (MVFR),
//! raspberry (IFR) and magenta `#b954e1` (LIFR). Terrain tint stays
//! neutral with a hair of cool, matching the gray chrome.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), desaturated from the Asciinema accents.
const CYAN: (u8, u8, u8) = (62, 158, 192); // primary #26b0d7 — controlled
const FAINT_CYAN: (u8, u8, u8) = (108, 156, 178); // class E/F band
const RASPBERRY: (u8, u8, u8) = (212, 96, 128); // danger red #dd3c69 — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (214, 116, 96); // danger areas (warmer than raspberry)
const SLATE_GREY: (u8, u8, u8) = (148, 152, 156); // TMZ
const SAND: (u8, u8, u8) = (208, 174, 112); // yellow #ddaf3c — glider / para-jump
const NEUTRAL: (u8, u8, u8) = (140, 142, 144);

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
    // One step below the UI background #121314: neutral near-black with the
    // same faint cool lean (b one notch above r).
    let land = srgb8(0x11, 0x12, 0x13);
    MapTheme {
        id: "asciinema",
        name: "Asciinema",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and cooler than land.
            water: srgb8(0x0e, 0x10, 0x13),
            waterway: srgb8(0x1a, 0x22, 0x28),
            // Landcover: neutral greys a whisker around `land`.
            forest: srgb8(0x0f, 0x10, 0x11),
            grass: srgb8(0x10, 0x11, 0x12),
            farmland: srgb8(0x12, 0x13, 0x14),
            barren: srgb8(0x13, 0x14, 0x15),
            glacier: srgb8(0x15, 0x16, 0x1a),
            park: srgb8(0x10, 0x11, 0x12),
            urban: srgb8(0x15, 0x16, 0x17),
            urban_dense: srgb8(0x18, 0x19, 0x19), // echoes the UI panel #181919
            military: srgb8(0x14, 0x15, 0x16),
            aerodrome: srgb8(0x16, 0x17, 0x1a),
            // Compressed road ramp: faint texture, never competing lines.
            road_highway: srgb8(0x1f, 0x20, 0x21),
            road_major: srgb8(0x1b, 0x1c, 0x1d),
            road_medium: srgb8(0x18, 0x1a, 0x1b),
            road_minor: srgb8(0x15, 0x16, 0x17),
            path: srgb8(0x13, 0x14, 0x15),
            rail: srgb8_a(0x19, 0x1a, 0x1d, 0.85),
            // Neutral terminal greys (ring #5d5d5d / muted #6d6d6d family).
            boundary_country: srgb8_a(0x5c, 0x5d, 0x60, 0.55),
            boundary_region: srgb8_a(0x42, 0x43, 0x46, 0.30),
            place_label: srgb8(0x54, 0x55, 0x58),
            country_label: srgb8(0x62, 0x63, 0x66),
            water_label: srgb8(0x3e, 0x47, 0x4f),
        },
        airspace: AirspaceTheme {
            class_a: pair(CYAN, 0.05, 0.72),
            class_b: pair(CYAN, 0.05, 0.72),
            class_c: pair(CYAN, 0.07, 0.78),
            class_d: pair(CYAN, 0.05, 0.7),
            class_e: pair(FAINT_CYAN, 0.02, 0.35),
            class_f: pair(FAINT_CYAN, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(RASPBERRY, 0.09, 0.8),
            rmz: pair(CYAN, 0.035, 0.68),
            tmz: pair(SLATE_GREY, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(RASPBERRY, 0.12, 0.8),
            prohibited: pair(RASPBERRY, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(198, 198, 194, 1.0),
            glider: srgb(208, 182, 120, 1.0),
            navaid: srgb(138, 164, 180, 1.0),
            reporting: srgb(214, 216, 220, 1.0),
            obstacle: srgb(210, 116, 128, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 31, 34, 1.0),
        },
        weather: WeatherTheme {
            // Semantic categories from the terminal accent set.
            vfr: srgb(110, 192, 96, 1.0),   // green #4ebf22
            mvfr: srgb(96, 150, 214, 1.0),  // primary cyan pushed toward blue
            ifr: srgb(216, 96, 116, 1.0),   // raspberry #dd3c69
            lifr: srgb(192, 116, 216, 1.0), // magenta #b954e1
            sigmet: srgb(216, 150, 80, 0.45),
            // Gridded overlays: muted, a hair cooler than Oldworld's.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 156), 0.0),
                stop(40.0, (156, 160, 164), 0.12),
                stop(75.0, (184, 188, 192), 0.28),
                stop(100.0, (210, 214, 218), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (70, 140, 200), 0.0),
                stop(1.0, (70, 140, 200), 0.32),
                stop(5.0, (70, 180, 200), 0.42),
                stop(20.0, (208, 184, 96), 0.5),
                stop(50.0, (204, 90, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (214, 160, 92), 0.0),
                stop(5.0, (210, 146, 78), 0.32),
                stop(15.0, (202, 92, 80), 0.5),
            ]),
        },
        // Route: high-vis amber, clear of the cyan/raspberry accents;
        // conflicts in raspberry red.
        route: RouteTheme {
            line: srgb(255, 180, 60, 1.0),
            line_conflict: srgb(235, 70, 85, 1.0),
            handle_fill: srgb(255, 180, 60, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 180, 60, 0.12),
        },
        labels: LabelTheme {
            // The UI foreground #cccccc, softened.
            text: srgb(204, 204, 204, 0.95),
            halo: [0.0; 4],
        },
        // Neutral relief with a hair of cool to match the gray chrome.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x15, 0x17),
            light_tint: tint_from_srgb8(0x7c, 0x7e, 0x82),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

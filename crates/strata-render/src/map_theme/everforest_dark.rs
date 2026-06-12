//! "Everforest Dark" — map theme paired with the Everforest Dark UI theme.
//!
//! Palette rationale: the ground band drops a few channels below the UI's
//! blue-green slate background (`#262E34` window, `#1f262b` title bar) to a
//! near-black sea-green `#161b1d`, with landcover leaning a whisker greener
//! (Everforest *is* a forest theme) and roads as a faint equal-step ramp.
//! Airspace identity comes from the theme's accents: controlled airspace in
//! Everforest's teal-blue (`base.blue #7fbbb3`), CTR/restricted/prohibited
//! in its soft red (`base.red #e67e80`), danger in the primary orange
//! (`#e69875`), glider/para in the yellow (`base.yellow #dbbc7f`), TMZ in a
//! gray-green from `muted.foreground`. Weather stays semantic but pastel;
//! terrain tints lean cool-green to match the ground.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Everforest accent hues, pastelized for near-black ground (sRGB bytes).
const TEAL: (u8, u8, u8) = (118, 172, 164); // base.blue — controlled airspace
const FAINT_TEAL: (u8, u8, u8) = (138, 176, 170); // class E/F band
const RED: (u8, u8, u8) = (218, 122, 124); // base.red — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (216, 144, 110); // primary — danger areas
const GRAY_GREEN: (u8, u8, u8) = (150, 158, 152); // muted fg — TMZ
const SAND: (u8, u8, u8) = (211, 180, 122); // base.yellow — glider / para
const NEUTRAL: (u8, u8, u8) = (140, 146, 142);

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
    // Near-black sea-green slate, a few steps below the UI background so the
    // map reads as the deepest layer of the Everforest chrome.
    let land = srgb8(0x16, 0x1b, 0x1d);
    MapTheme {
        id: "everforest-dark",
        name: "Everforest Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and cooler — just enough to read as water.
            water: srgb8(0x12, 0x17, 0x1b),
            waterway: srgb8(0x21, 0x2c, 0x33),
            // Landcover hugs the land band, leaning faintly green.
            forest: srgb8(0x14, 0x1a, 0x1a),
            grass: srgb8(0x15, 0x1b, 0x1c),
            farmland: srgb8(0x18, 0x1d, 0x1e),
            barren: srgb8(0x19, 0x1d, 0x1f),
            glacier: srgb8(0x1b, 0x20, 0x24),
            park: srgb8(0x15, 0x1b, 0x1b),
            urban: srgb8(0x1b, 0x20, 0x22),
            urban_dense: srgb8(0x1e, 0x23, 0x25),
            military: srgb8(0x1a, 0x1e, 0x20),
            aerodrome: srgb8(0x1c, 0x21, 0x24),
            // Compressed road ramp: faint texture, keeping the band's hue.
            road_highway: srgb8(0x24, 0x29, 0x2b),
            road_major: srgb8(0x20, 0x25, 0x27),
            road_medium: srgb8(0x1d, 0x22, 0x24),
            road_minor: srgb8(0x1a, 0x1f, 0x21),
            path: srgb8(0x18, 0x1d, 0x1f),
            rail: srgb8_a(0x1f, 0x24, 0x27, 0.85),
            // Boundaries in desaturated green-grays of the UI's muted scale.
            boundary_country: srgb8_a(0x5e, 0x68, 0x66, 0.55),
            boundary_region: srgb8_a(0x46, 0x4f, 0x4d, 0.30),
            place_label: srgb8(0x59, 0x62, 0x5e),
            country_label: srgb8(0x66, 0x6f, 0x6b),
            water_label: srgb8(0x49, 0x55, 0x58),
        },
        airspace: AirspaceTheme {
            class_a: pair(TEAL, 0.05, 0.72),
            class_b: pair(TEAL, 0.05, 0.72),
            class_c: pair(TEAL, 0.07, 0.78),
            class_d: pair(TEAL, 0.05, 0.7),
            class_e: pair(FAINT_TEAL, 0.02, 0.35),
            class_f: pair(FAINT_TEAL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(RED, 0.09, 0.8),
            rmz: pair(TEAL, 0.035, 0.68),
            tmz: pair(GRAY_GREEN, 0.03, 0.75),
            danger: pair(ORANGE, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Warm parchment foreground (#d3c6aa family) on the green ground.
            airport: srgb(204, 198, 184, 1.0),
            glider: srgb(211, 184, 126, 1.0),
            navaid: srgb(150, 170, 166, 1.0),
            reporting: srgb(218, 220, 214, 1.0),
            obstacle: srgb(216, 128, 122, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 36, 38, 1.0),
        },
        weather: WeatherTheme {
            // Everforest green / a true-blue lift of base.blue / soft red /
            // light magenta — semantic and pairwise distinct.
            vfr: srgb(146, 188, 114, 1.0),
            mvfr: srgb(112, 152, 212, 1.0),
            ifr: srgb(224, 116, 118, 1.0),
            lifr: srgb(202, 134, 188, 1.0),
            sigmet: srgb(224, 148, 96, 0.45),
            // Gridded overlays, muted to match the pastel look.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 154, 152), 0.0),
                stop(40.0, (156, 162, 160), 0.12),
                stop(75.0, (184, 190, 188), 0.28),
                stop(100.0, (212, 214, 218), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (96, 140, 200), 0.0),
                stop(1.0, (96, 140, 200), 0.32),
                stop(5.0, (100, 186, 192), 0.42),
                stop(20.0, (210, 186, 100), 0.5),
                stop(50.0, (208, 100, 88), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (216, 166, 98), 0.0),
                stop(5.0, (212, 148, 80), 0.32),
                stop(15.0, (200, 94, 82), 0.5),
            ]),
        },
        // Route: bright amber above the forest greens; conflicts in everforest
        // red.
        route: RouteTheme {
            line: srgb(245, 180, 85, 1.0),
            line_conflict: srgb(230, 88, 84, 1.0),
            handle_fill: srgb(245, 180, 85, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(245, 180, 85, 0.12),
        },
        labels: LabelTheme {
            // Everforest's parchment foreground, slightly desaturated.
            text: srgb(206, 198, 178, 0.95),
            halo: [0.0; 4],
        },
        // Relief tinted toward the cool green ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x18, 0x18),
            light_tint: tint_from_srgb8(0x80, 0x84, 0x7c),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

//! "Kibble" — map theme paired with the Kibble UI theme: a green-tinted
//! near-black background (`#0e100a`) under a bright green primary
//! (`#6ce05c`), with indigo-blue, teal, salmon and mustard accents.
//!
//! The basemap keeps the UI's subtle green cast (g > r > b) in a near-black
//! band around the window background; the green identity also shows in the
//! grass/forest whispers, the VFR weather green and the green-leaning
//! terrain tints. Airspaces anchor on the indigo blue (`base.blue
//! #3449d1` / `#5c6ee0`) for the controlled classes, a softened
//! `base.red #C70231` for CTR/restricted/prohibited, the salmon danger
//! accent (`#e07b5c`) for danger areas, teal (`base.cyan #0798ab`) for RMZ,
//! mustard (`base.yellow #c7a302`) for glider sectors and a muted violet
//! (from `base.magenta #8400ff`) for TMZ.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Kibble accent set (sRGB bytes).
const INDIGO: (u8, u8, u8) = (96, 118, 212); // base.blue #3449d1 / #5c6ee0 — controlled
const FAINT_INDIGO: (u8, u8, u8) = (124, 140, 206); // class E/F band
const RED: (u8, u8, u8) = (210, 86, 96); // base.red #C70231, softened — CTR / ED-R / ED-P
const SALMON: (u8, u8, u8) = (216, 126, 96); // danger #e07b5c — danger areas
const TEAL: (u8, u8, u8) = (70, 170, 184); // base.cyan #0798ab — RMZ
const MUSTARD: (u8, u8, u8) = (198, 168, 72); // base.yellow #c7a302 — glider / para
const VIOLET_GREY: (u8, u8, u8) = (152, 140, 170); // base.magenta #8400ff, heavily muted — TMZ
const NEUTRAL: (u8, u8, u8) = (140, 144, 138); // green-leaning neutral

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
    // A couple of channels below the UI background #0e100a, keeping the
    // green cast (g > r > b).
    let land = srgb8(0x0c, 0x0e, 0x09);
    MapTheme {
        id: "kibble",
        name: "Kibble",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Darker, with the green cast traded for a teal lean.
            water: srgb8(0x09, 0x0b, 0x0b),
            waterway: srgb8(0x14, 0x1c, 0x20),
            // Landcover: green whispers a channel or two around land.
            forest: srgb8(0x0b, 0x0d, 0x08),
            grass: srgb8(0x0c, 0x0f, 0x09),
            farmland: srgb8(0x0d, 0x0f, 0x0a),
            barren: srgb8(0x0e, 0x0f, 0x0b),
            glacier: srgb8(0x10, 0x12, 0x0f),
            park: srgb8(0x0c, 0x0e, 0x08),
            urban: srgb8(0x10, 0x12, 0x0c),
            urban_dense: srgb8(0x13, 0x15, 0x10),
            military: srgb8(0x0f, 0x11, 0x0b),
            aerodrome: srgb8(0x11, 0x13, 0x10),
            // Compressed road ramp above the green-black ground
            // (motorway ≈ +10 → luma ratio ~1.75).
            road_highway: srgb8(0x16, 0x18, 0x11),
            road_major: srgb8(0x13, 0x15, 0x0f),
            road_medium: srgb8(0x11, 0x13, 0x0d),
            road_minor: srgb8(0x0f, 0x11, 0x0c),
            path: srgb8(0x0e, 0x10, 0x0b),
            rail: srgb8_a(0x12, 0x14, 0x12, 0.85),
            // Gray-green boundaries and labels.
            boundary_country: srgb8_a(0x56, 0x5a, 0x50, 0.55),
            boundary_region: srgb8_a(0x3e, 0x42, 0x3a, 0.30),
            place_label: srgb8(0x50, 0x54, 0x4b),
            country_label: srgb8(0x5d, 0x61, 0x57),
            water_label: srgb8(0x44, 0x50, 0x4e),
        },
        airspace: AirspaceTheme {
            class_a: pair(INDIGO, 0.05, 0.72),
            class_b: pair(INDIGO, 0.05, 0.72),
            class_c: pair(INDIGO, 0.07, 0.78),
            class_d: pair(INDIGO, 0.05, 0.7),
            class_e: pair(FAINT_INDIGO, 0.02, 0.35),
            class_f: pair(FAINT_INDIGO, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(RED, 0.09, 0.8),
            rmz: pair(TEAL, 0.035, 0.68),
            tmz: pair(VIOLET_GREY, 0.03, 0.75),
            danger: pair(SALMON, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(MUSTARD, 0.045, 0.75),
            para_jump: pair(MUSTARD, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(198, 202, 190, 1.0),
            glider: srgb(200, 176, 104, 1.0),
            navaid: srgb(138, 150, 190, 1.0),
            reporting: srgb(212, 218, 208, 1.0),
            obstacle: srgb(214, 122, 98, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(24, 26, 22, 1.0),
        },
        weather: WeatherTheme {
            // VFR carries the signature green (#6ce05c, calmed); the rest
            // follow base.blue / base.red / base.magenta.
            vfr: srgb(108, 200, 96, 1.0),
            mvfr: srgb(104, 134, 222, 1.0),
            ifr: srgb(212, 84, 84, 1.0),
            lifr: srgb(180, 110, 210, 1.0),
            sigmet: srgb(216, 138, 76, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 148), 0.0),
                stop(40.0, (156, 160, 156), 0.12),
                stop(75.0, (184, 188, 184), 0.28),
                stop(100.0, (210, 214, 210), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (92, 134, 204), 0.0),
                stop(1.0, (92, 134, 204), 0.32),
                stop(5.0, (84, 180, 186), 0.42),
                stop(20.0, (204, 186, 90), 0.5),
                stop(50.0, (208, 90, 78), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (214, 162, 88), 0.0),
                stop(5.0, (210, 148, 76), 0.32),
                stop(15.0, (200, 92, 76), 0.5),
            ]),
        },
        // Route: bright mustard-gold (#c7a302 vivified); conflicts in kibble
        // red.
        route: RouteTheme {
            line: srgb(250, 190, 60, 1.0),
            line_conflict: srgb(234, 64, 74, 1.0),
            handle_fill: srgb(250, 190, 60, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 190, 60, 0.12),
        },
        labels: LabelTheme {
            // Slightly green-warm white from the UI foreground #f7f7f7.
            text: srgb(206, 210, 198, 0.95),
            halo: [0.0; 4],
        },
        // Green-leaning relief to match the ground cast.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x16, 0x0e),
            light_tint: tint_from_srgb8(0x80, 0x84, 0x76),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

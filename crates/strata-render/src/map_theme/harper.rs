//! "Harper" — map theme paired with the Harper UI theme: the blackest
//! background of the catalog (`#010101`) under warm-gray text (`#a8a49d`),
//! with a lavender primary (`#B196C6`) and pink-red / sky-blue / amber
//! accents.
//!
//! The basemap is a warm near-black band (r = g > b, kin to the warm UI
//! grays) just above the pitch-black window. Airspaces anchor on the
//! lavender primary for the controlled classes, the pink-red
//! (`base.red #ff5874`) for CTR/restricted/prohibited, a warmer salmon for
//! danger areas, sky blue (`base.blue #7fb5e1`) for RMZ and a darkened
//! `base.yellow #f8b63f` sand for glider sectors.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Harper accent set (sRGB bytes).
const LAVENDER: (u8, u8, u8) = (177, 150, 198); // primary #B196C6 — controlled
const FAINT_LAVENDER: (u8, u8, u8) = (158, 150, 180); // class E/F band
const PINK: (u8, u8, u8) = (224, 108, 128); // base.red #ff5874, softened — CTR / ED-R / ED-P
const SALMON: (u8, u8, u8) = (216, 134, 96); // warmer than PINK — danger areas
const SKY: (u8, u8, u8) = (120, 170, 210); // base.blue #7fb5e1 — RMZ
const SAND: (u8, u8, u8) = (210, 170, 96); // base.yellow #f8b63f, darkened — glider / para
const VIOLET_GREY: (u8, u8, u8) = (150, 146, 156); // TMZ
const NEUTRAL: (u8, u8, u8) = (144, 142, 138); // warm neutral, kin to #726E69

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
    // Warm pitch-black just above the #010101 window, below the title bar.
    let land = srgb8(0x08, 0x08, 0x07);
    MapTheme {
        id: "harper",
        name: "Harper",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Water flips the warm balance: darker and cooler than land.
            water: srgb8(0x05, 0x06, 0x07),
            waterway: srgb8(0x12, 0x16, 0x1e),
            // Landcover: warm whispers around land.
            forest: srgb8(0x07, 0x07, 0x06),
            grass: srgb8(0x08, 0x08, 0x06),
            farmland: srgb8(0x09, 0x09, 0x07),
            barren: srgb8(0x0a, 0x0a, 0x08),
            glacier: srgb8(0x0b, 0x0b, 0x0c),
            park: srgb8(0x08, 0x08, 0x06),
            urban: srgb8(0x0b, 0x0b, 0x0a),
            urban_dense: srgb8(0x0d, 0x0d, 0x0c),
            military: srgb8(0x0a, 0x0a, 0x09),
            aerodrome: srgb8(0x0b, 0x0b, 0x0d),
            // Compressed warm road ramp (+1..+6 over the black ground).
            road_highway: srgb8(0x0e, 0x0e, 0x0d),
            road_major: srgb8(0x0c, 0x0c, 0x0b),
            road_medium: srgb8(0x0b, 0x0b, 0x0a),
            road_minor: srgb8(0x0a, 0x0a, 0x09),
            path: srgb8(0x09, 0x09, 0x08),
            rail: srgb8_a(0x0c, 0x0c, 0x0e, 0.85),
            // Warm grays from the muted foreground #726E69.
            boundary_country: srgb8_a(0x56, 0x53, 0x4e, 0.55),
            boundary_region: srgb8_a(0x3e, 0x3c, 0x38, 0.30),
            place_label: srgb8(0x52, 0x4f, 0x4a),
            country_label: srgb8(0x60, 0x5c, 0x57),
            water_label: srgb8(0x40, 0x47, 0x50),
        },
        airspace: AirspaceTheme {
            class_a: pair(LAVENDER, 0.05, 0.72),
            class_b: pair(LAVENDER, 0.05, 0.72),
            class_c: pair(LAVENDER, 0.07, 0.78),
            class_d: pair(LAVENDER, 0.05, 0.7),
            class_e: pair(FAINT_LAVENDER, 0.02, 0.35),
            class_f: pair(FAINT_LAVENDER, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(PINK, 0.09, 0.8),
            rmz: pair(SKY, 0.035, 0.68),
            tmz: pair(VIOLET_GREY, 0.03, 0.75),
            danger: pair(SALMON, 0.06, 0.7),
            restricted: pair(PINK, 0.12, 0.8),
            prohibited: pair(PINK, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Warm-gray symbols, lavender-leaning navaids.
            airport: srgb(202, 198, 190, 1.0),
            glider: srgb(206, 178, 112, 1.0),
            navaid: srgb(158, 148, 174, 1.0),
            reporting: srgb(216, 214, 220, 1.0),
            obstacle: srgb(212, 116, 120, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(28, 28, 30, 1.0),
        },
        weather: WeatherTheme {
            // base.green / base.blue / base.red / base.magenta, pastelized
            // toward the warm-black ground.
            vfr: srgb(104, 180, 116, 1.0),
            mvfr: srgb(110, 156, 216, 1.0),
            ifr: srgb(224, 100, 110, 1.0),
            lifr: srgb(192, 120, 200, 1.0),
            sigmet: srgb(220, 140, 84, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (152, 150, 146), 0.0),
                stop(40.0, (160, 158, 154), 0.12),
                stop(75.0, (188, 186, 182), 0.28),
                stop(100.0, (214, 212, 208), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (98, 140, 200), 0.0),
                stop(1.0, (98, 140, 200), 0.32),
                stop(5.0, (98, 186, 196), 0.42),
                stop(20.0, (210, 186, 98), 0.5),
                stop(50.0, (212, 96, 86), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (218, 164, 96), 0.0),
                stop(5.0, (214, 150, 82), 0.32),
                stop(15.0, (202, 96, 88), 0.5),
            ]),
        },
        // Route: spring green (#9ece6a kin) — the one accent free of airspace
        // duty; conflicts in harper pink-red.
        route: RouteTheme {
            line: srgb(138, 222, 112, 1.0),
            line_conflict: srgb(236, 70, 86, 1.0),
            handle_fill: srgb(138, 222, 112, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(138, 222, 112, 0.12),
        },
        labels: LabelTheme {
            // Warm gray-white, a brightened cousin of the UI foreground.
            text: srgb(208, 202, 190, 0.95),
            halo: [0.0; 4],
        },
        // Warm relief to match the warm-black band.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x17, 0x13, 0x11),
            light_tint: tint_from_srgb8(0x86, 0x80, 0x7a),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

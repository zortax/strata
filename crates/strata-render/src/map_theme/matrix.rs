//! "Matrix" — map theme paired with the Matrix UI theme (green phosphor on
//! black: `background #020D02`, `panel #001900`, primary `#00FF00`,
//! foreground `#88FF88`, muted `#007700`).
//!
//! Palette rationale: the ground is a green-tinted near-black just above
//! the UI background, with every basemap feature — roads, landcover,
//! boundaries, even place labels — held inside the green channel so the
//! whole map reads like a phosphor display. Water leans teal (the theme's
//! cyan `#00ffd5`) and darker. The airspace layer keeps the neon identity:
//! controlled airspace in phosphor green (the primary), CTR / restricted /
//! prohibited in the alarm red `#FF0000`, danger areas an amber between
//! red and the yellow `#ffea00`, glider/para in that yellow, TMZ and the
//! catch-all classes in green-grey. Weather categories stay semantic but
//! neon-leaning (pure-ish green / blue / red / magenta from the base
//! accent set). Terrain relief is tinted green so hillshade reads as
//! scanline texture rather than a foreign gray.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) from the Matrix accent set.
const PHOSPHOR: (u8, u8, u8) = (62, 220, 70); // primary #00FF00 — controlled
const FAINT_PHOSPHOR: (u8, u8, u8) = (110, 200, 116); // class E/F band
const RED: (u8, u8, u8) = (228, 56, 48); // #FF0000 — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (222, 128, 40); // danger areas (red warmed toward yellow)
const GREEN_GREY: (u8, u8, u8) = (134, 160, 134); // TMZ
const AMBER: (u8, u8, u8) = (212, 192, 60); // yellow #ffea00 — glider / para-jump
const NEUTRAL: (u8, u8, u8) = (112, 138, 112);

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
    // A whisker above the UI background #020D02: green-black phosphor ground.
    let land = srgb8(0x03, 0x0e, 0x03);
    MapTheme {
        id: "matrix",
        name: "Matrix",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Darker and teal-leaning (toward the cyan accent #00ffd5).
            water: srgb8(0x02, 0x0a, 0x06),
            waterway: srgb8(0x05, 0x1e, 0x16),
            // Landcover: green-black band a whisker around `land`.
            forest: srgb8(0x02, 0x0d, 0x02),
            grass: srgb8(0x03, 0x0f, 0x03),
            farmland: srgb8(0x04, 0x0f, 0x04),
            barren: srgb8(0x05, 0x10, 0x05),
            glacier: srgb8(0x06, 0x12, 0x0a),
            park: srgb8(0x03, 0x0f, 0x03),
            urban: srgb8(0x05, 0x12, 0x05),
            urban_dense: srgb8(0x07, 0x15, 0x07),
            military: srgb8(0x05, 0x11, 0x05),
            aerodrome: srgb8(0x06, 0x13, 0x08),
            // Compressed phosphor road ramp: +2..+10 on the green channel.
            road_highway: srgb8(0x09, 0x18, 0x09),
            road_major: srgb8(0x08, 0x16, 0x08),
            road_medium: srgb8(0x07, 0x14, 0x07),
            road_minor: srgb8(0x06, 0x12, 0x06),
            path: srgb8(0x05, 0x10, 0x05),
            rail: srgb8_a(0x07, 0x15, 0x0a, 0.85),
            // Borders and place names in the muted phosphor #007700 family.
            boundary_country: srgb8_a(0x33, 0x77, 0x33, 0.55),
            boundary_region: srgb8_a(0x26, 0x55, 0x26, 0.30),
            place_label: srgb8(0x2e, 0x7a, 0x2e),
            country_label: srgb8(0x3a, 0x8e, 0x3a),
            water_label: srgb8(0x20, 0x66, 0x4e),
        },
        airspace: AirspaceTheme {
            class_a: pair(PHOSPHOR, 0.05, 0.72),
            class_b: pair(PHOSPHOR, 0.05, 0.72),
            class_c: pair(PHOSPHOR, 0.07, 0.78),
            class_d: pair(PHOSPHOR, 0.05, 0.7),
            class_e: pair(FAINT_PHOSPHOR, 0.02, 0.35),
            class_f: pair(FAINT_PHOSPHOR, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(RED, 0.09, 0.8),
            rmz: pair(PHOSPHOR, 0.035, 0.68),
            tmz: pair(GREEN_GREY, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(AMBER, 0.045, 0.75),
            para_jump: pair(AMBER, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(168, 235, 168, 1.0),
            glider: srgb(212, 196, 96, 1.0),
            navaid: srgb(104, 196, 176, 1.0),
            reporting: srgb(196, 248, 196, 1.0),
            obstacle: srgb(232, 96, 84, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(6, 24, 6, 1.0),
        },
        weather: WeatherTheme {
            // Neon but semantic: green / blue / red / magenta base accents.
            vfr: srgb(90, 230, 110, 1.0),  // #00FF00
            mvfr: srgb(80, 110, 235, 1.0), // #0000ff
            ifr: srgb(235, 70, 60, 1.0),   // #FF0000
            lifr: srgb(220, 80, 220, 1.0), // #FF00FF
            sigmet: srgb(224, 164, 48, 0.45),
            // Gridded overlays: muted, with a faint phosphor cast on clouds.
            cloud_cover: Colormap::new(&[
                stop(10.0, (140, 148, 140), 0.0),
                stop(40.0, (150, 158, 150), 0.12),
                stop(75.0, (178, 186, 178), 0.28),
                stop(100.0, (204, 212, 204), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 90, 220), 0.0),
                stop(1.0, (60, 90, 220), 0.32),
                stop(5.0, (40, 210, 190), 0.42),
                stop(20.0, (225, 205, 60), 0.5),
                stop(50.0, (225, 70, 55), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (230, 170, 60), 0.0),
                stop(5.0, (225, 150, 50), 0.32),
                stop(15.0, (220, 80, 55), 0.5),
            ]),
        },
        // Route: pure phosphor yellow #ffea00 — reads over every green;
        // conflicts in alarm red-pink.
        route: RouteTheme {
            line: srgb(255, 234, 0, 1.0),
            line_conflict: srgb(255, 70, 90, 1.0),
            handle_fill: srgb(255, 234, 0, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 234, 0, 0.12),
        },
        labels: LabelTheme {
            // The UI foreground #88FF88, softened a notch.
            text: srgb(150, 240, 150, 0.95),
            halo: [0.0; 4],
        },
        // Green-tinted relief: hillshade as phosphor texture.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x04, 0x10, 0x04),
            light_tint: tint_from_srgb8(0x4e, 0x7a, 0x4e),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

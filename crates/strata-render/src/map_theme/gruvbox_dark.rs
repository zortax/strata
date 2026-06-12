//! "Gruvbox Dark" — map theme paired with the Gruvbox Dark UI theme.
//!
//! Palette rationale: the ground band sits a few channels below the UI's
//! `#1d2021` background as a warm near-black brown-gray `#191817` (r ≥ g ≥ b
//! — Gruvbox's warm identity, hard-muted), landcover leaning a whisker
//! olive-brown, roads a faint warm ramp. Airspace identity from the Gruvbox
//! accents: controlled airspace in a pastelized `base.blue #458588` teal,
//! CTR/restricted/prohibited from the red `#f06555`, danger in an orange
//! midpoint toward the yellow, glider/para in sand from the primary yellow
//! `#d79921`, TMZ in a warm mauve-gray from `base.magenta #b16286`.
//! Weather stays semantic (the olive `#98971a` green is lifted toward true
//! green for VFR); terrain tints warm brown to match the ground.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Gruvbox accent hues, pastelized for near-black warm ground (sRGB bytes).
const BLUE: (u8, u8, u8) = (102, 156, 160); // base.blue — controlled airspace
const FAINT_BLUE: (u8, u8, u8) = (122, 162, 164); // class E/F band
const RED: (u8, u8, u8) = (222, 110, 96); // base.red — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (214, 130, 72); // gruvbox orange — danger areas
const MAUVE: (u8, u8, u8) = (164, 140, 150); // base.magenta, grayed — TMZ
const SAND: (u8, u8, u8) = (212, 176, 108); // primary yellow — glider / para
const NEUTRAL: (u8, u8, u8) = (148, 144, 136); // warm gray (muted fg family)

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
    // Warm near-black brown-gray, below the UI's hard-contrast `#1d2021`.
    let land = srgb8(0x19, 0x18, 0x17);
    MapTheme {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker, leaning cool against the warm land.
            water: srgb8(0x14, 0x16, 0x18),
            waterway: srgb8(0x29, 0x31, 0x36),
            // Landcover hugs the land band with a faint olive-brown cast.
            forest: srgb8(0x17, 0x17, 0x14),
            grass: srgb8(0x18, 0x18, 0x15),
            farmland: srgb8(0x1b, 0x1a, 0x16),
            barren: srgb8(0x1c, 0x1a, 0x17),
            glacier: srgb8(0x1e, 0x1e, 0x1f),
            park: srgb8(0x18, 0x18, 0x15),
            urban: srgb8(0x1d, 0x1c, 0x1a),
            urban_dense: srgb8(0x20, 0x1f, 0x1d),
            military: srgb8(0x1c, 0x1a, 0x18),
            aerodrome: srgb8(0x1e, 0x1d, 0x1c),
            // Compressed warm road ramp: faint texture, never bright lines.
            road_highway: srgb8(0x27, 0x26, 0x24),
            road_major: srgb8(0x23, 0x22, 0x20),
            road_medium: srgb8(0x20, 0x1f, 0x1e),
            road_minor: srgb8(0x1d, 0x1c, 0x1b),
            path: srgb8(0x1b, 0x1a, 0x19),
            rail: srgb8_a(0x21, 0x20, 0x1f, 0.85),
            // Boundaries in the warm gray of `muted.foreground #928374`.
            boundary_country: srgb8_a(0x6a, 0x63, 0x58, 0.55),
            boundary_region: srgb8_a(0x4e, 0x49, 0x41, 0.30),
            place_label: srgb8(0x60, 0x5a, 0x50),
            country_label: srgb8(0x6d, 0x66, 0x5b),
            water_label: srgb8(0x4b, 0x52, 0x58),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.05, 0.72),
            class_b: pair(BLUE, 0.05, 0.72),
            class_c: pair(BLUE, 0.07, 0.78),
            class_d: pair(BLUE, 0.05, 0.7),
            class_e: pair(FAINT_BLUE, 0.02, 0.35),
            class_f: pair(FAINT_BLUE, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(RED, 0.09, 0.8),
            rmz: pair(BLUE, 0.035, 0.68),
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(ORANGE, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Gruvbox cream foreground (#ebdbb2 family) on the warm ground.
            airport: srgb(206, 198, 180, 1.0),
            glider: srgb(210, 180, 116, 1.0),
            navaid: srgb(140, 160, 162, 1.0),
            reporting: srgb(220, 216, 206, 1.0),
            obstacle: srgb(212, 118, 104, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(34, 32, 30, 1.0),
        },
        weather: WeatherTheme {
            // Olive green lifted to read as green, base.blue lifted to a
            // true blue, soft red, base.magenta lightened — all semantic.
            vfr: srgb(140, 180, 96, 1.0),
            mvfr: srgb(96, 148, 210, 1.0),
            ifr: srgb(226, 96, 84, 1.0),
            lifr: srgb(196, 104, 176, 1.0),
            sigmet: srgb(222, 150, 80, 0.45),
            // Gridded overlays, muted with a warm lean.
            cloud_cover: Colormap::new(&[
                stop(10.0, (152, 150, 146), 0.0),
                stop(40.0, (160, 158, 154), 0.12),
                stop(75.0, (188, 186, 182), 0.28),
                stop(100.0, (214, 212, 206), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (92, 134, 196), 0.0),
                stop(1.0, (92, 134, 196), 0.32),
                stop(5.0, (92, 184, 194), 0.42),
                stop(20.0, (210, 184, 92), 0.5),
                stop(50.0, (206, 90, 76), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (218, 160, 88), 0.0),
                stop(5.0, (214, 146, 76), 0.32),
                stop(15.0, (200, 92, 80), 0.5),
            ]),
        },
        // Route: gruvbox bright yellow #fabd2f; conflicts in bright red
        // #fb4934.
        route: RouteTheme {
            line: srgb(250, 189, 47, 1.0),
            line_conflict: srgb(235, 73, 52, 1.0),
            handle_fill: srgb(250, 189, 47, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 189, 47, 0.12),
        },
        labels: LabelTheme {
            // The UI cream foreground, slightly dimmed and desaturated.
            text: srgb(212, 202, 178, 0.95),
            halo: [0.0; 4],
        },
        // Relief tinted warm brown to match the ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x1a, 0x15, 0x10),
            light_tint: tint_from_srgb8(0x86, 0x7e, 0x70),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

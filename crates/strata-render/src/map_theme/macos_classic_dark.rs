//! "macOS Classic Dark" — map theme paired with the macOS Classic Dark UI
//! theme. The UI chrome is dead-neutral graphite (`#131313` window,
//! `#1C1C1E` title bar, `#202020` panels), so the basemap is a neutral
//! near-black band sitting just below the window background, with only a
//! whisper of the title bar's cool cast in glacier/rail/aerodrome tones.
//! Airspaces carry the macOS system accents, pastelized: System Blue
//! (`#419CFF`) for controlled airspace, System Red (`#FF5257`) for
//! CTR/restricted/prohibited, a warmer red-orange blend for danger,
//! System Yellow (`#FFC600`) muted to sand for glider/para sectors, and
//! the magenta accent (`#A550A7`) grayed down for TMZ. Weather categories
//! come straight from the system green/blue/red/magenta set, softened.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), pastelized from the macOS dark accent set.
const BLUE: (u8, u8, u8) = (110, 158, 224); // System Blue #419CFF — controlled
const FAINT_BLUE: (u8, u8, u8) = (126, 164, 212); // class E/F band
const RED: (u8, u8, u8) = (224, 108, 112); // System Red #FF5257 — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (226, 130, 96); // danger areas (warmer red)
const MAUVE_GREY: (u8, u8, u8) = (160, 148, 162); // TMZ — magenta #A550A7 grayed
const SAND: (u8, u8, u8) = (216, 182, 110); // System Yellow #FFC600 muted
const NEUTRAL: (u8, u8, u8) = (140, 140, 144);

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
    // Neutral graphite just below the UI window background (#131313); the
    // faint blue lift keeps it kin with the #1C1C1E title bar.
    let land = srgb8(0x11, 0x11, 0x13);
    MapTheme {
        id: "macos-classic-dark",
        name: "macOS Classic Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than the graphite ground.
            water: srgb8(0x0e, 0x0f, 0x13),
            waterway: srgb8(0x1b, 0x1e, 0x25),
            // Landcover: neutral greys a whisker around `land` — the UI has
            // no green/brown identity, so neither does the ground.
            forest: srgb8(0x0f, 0x0f, 0x11),
            grass: srgb8(0x10, 0x10, 0x12),
            farmland: srgb8(0x12, 0x12, 0x14),
            barren: srgb8(0x13, 0x13, 0x15),
            glacier: srgb8(0x15, 0x15, 0x18),
            park: srgb8(0x10, 0x10, 0x12),
            urban: srgb8(0x15, 0x15, 0x17),
            urban_dense: srgb8(0x18, 0x18, 0x19),
            military: srgb8(0x14, 0x14, 0x16),
            aerodrome: srgb8(0x16, 0x16, 0x18),
            // Compressed road ramp: +13 channels at the motorway down to +2
            // for paths — faint texture under the system-accent overlays.
            road_highway: srgb8(0x1e, 0x1e, 0x20),
            road_major: srgb8(0x1b, 0x1b, 0x1d),
            road_medium: srgb8(0x18, 0x18, 0x1a),
            road_minor: srgb8(0x15, 0x15, 0x17),
            path: srgb8(0x13, 0x13, 0x15),
            rail: srgb8_a(0x19, 0x19, 0x1d, 0.85),
            // Boundaries: neutral mid-greys from the #9D9D9D muted scale.
            boundary_country: srgb8_a(0x5c, 0x5c, 0x61, 0.55),
            boundary_region: srgb8_a(0x42, 0x42, 0x47, 0.30),
            place_label: srgb8(0x58, 0x58, 0x5c),
            country_label: srgb8(0x66, 0x66, 0x6a),
            water_label: srgb8(0x40, 0x49, 0x56),
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
            tmz: pair(MAUVE_GREY, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(200, 200, 204, 1.0),
            glider: srgb(214, 184, 118, 1.0),
            navaid: srgb(146, 162, 184, 1.0),
            reporting: srgb(218, 218, 224, 1.0),
            obstacle: srgb(220, 116, 118, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 30, 34, 1.0),
        },
        weather: WeatherTheme {
            // System green / blue / red / magenta, softened for the dark map.
            vfr: srgb(92, 196, 128, 1.0),
            mvfr: srgb(96, 150, 230, 1.0),
            ifr: srgb(224, 98, 102, 1.0),
            lifr: srgb(188, 112, 190, 1.0),
            sigmet: srgb(230, 150, 80, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 152, 158), 0.0),
                stop(40.0, (160, 162, 168), 0.12),
                stop(75.0, (188, 190, 196), 0.28),
                stop(100.0, (214, 216, 222), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (90, 140, 214), 0.0),
                stop(1.0, (90, 140, 214), 0.32),
                stop(5.0, (88, 190, 204), 0.42),
                stop(20.0, (212, 188, 96), 0.5),
                stop(50.0, (212, 90, 86), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (218, 160, 92), 0.0),
                stop(5.0, (214, 146, 80), 0.32),
                stop(15.0, (202, 92, 86), 0.5),
            ]),
        },
        // Route: System Orange #FF9F0A — the free Apple accent; conflicts in
        // System Red.
        route: RouteTheme {
            line: srgb(255, 159, 30, 1.0),
            line_conflict: srgb(240, 68, 76, 1.0),
            handle_fill: srgb(255, 159, 30, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 159, 30, 0.12),
        },
        labels: LabelTheme {
            // Cool light grey from the UI foreground (#DEDEDE), softened.
            text: srgb(206, 208, 212, 0.95),
            halo: [0.0; 4],
        },
        // Neutral relief for a neutral theme; shadows pick up the faint cool
        // cast of the ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x13, 0x13, 0x16),
            light_tint: tint_from_srgb8(0x82, 0x82, 0x88),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

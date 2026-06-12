//! "Alduin" — map theme paired with the Alduin UI theme.
//!
//! Alduin's chrome is neutral warm gray (`background #1C1C1C`, panel
//! `#282828`) with gruvbox-kin muted accents: teal primary `#458588` /
//! `base.blue #87afaf`, dusty rose `base.magenta #af8787`, olive
//! `base.green #7a875f`, sand `base.yellow #9d906c` and cream text
//! `#ebdbb2`. The basemap is therefore a warm near-black gray band (land
//! `#161615`, a few steps under the UI background) with whisper-olive
//! landcover; the airspace layer carries the accents — teal for
//! controlled airspace, dusty rose for CTR/restricted/prohibited, peach
//! terracotta for danger, cream-sand for glider sectors. Labels and
//! symbols lean gruvbox cream.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Alduin accent set (sRGB bytes).
const TEAL: (u8, u8, u8) = (108, 152, 155); // primary #458588 / blue #87afaf — controlled
const FAINT_TEAL: (u8, u8, u8) = (130, 158, 158); // class E/F band
const ROSE: (u8, u8, u8) = (191, 134, 132); // magenta #af8787 lifted — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (201, 131, 99); // keyword peach #E19773 muted — danger
const SAND: (u8, u8, u8) = (193, 174, 124); // yellow #9d906c toward cream — glider / para
const WARM_GREY: (u8, u8, u8) = (150, 147, 143); // TMZ
const NEUTRAL: (u8, u8, u8) = (140, 139, 136);

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
    // Warm near-black gray, a few channel steps below the UI background
    // (#1C1C1C) so the map reads as the deepest layer of the chrome.
    let land = srgb8(0x16, 0x16, 0x15);
    MapTheme {
        id: "alduin",
        name: "Alduin",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than the warm land.
            water: srgb8(0x11, 0x12, 0x15),
            waterway: srgb8(0x1e, 0x21, 0x25),
            // Landcover: a whisker around land, with a faint olive cast
            // (Alduin's green accent is olive) on the vegetated classes.
            forest: srgb8(0x13, 0x14, 0x11),
            grass: srgb8(0x15, 0x16, 0x13),
            farmland: srgb8(0x17, 0x17, 0x15),
            barren: srgb8(0x18, 0x18, 0x16),
            glacier: srgb8(0x1a, 0x1a, 0x1b),
            park: srgb8(0x14, 0x15, 0x12),
            urban: srgb8(0x1a, 0x1a, 0x18),
            urban_dense: srgb8(0x1d, 0x1d, 0x1b),
            military: srgb8(0x19, 0x18, 0x17),
            aerodrome: srgb8(0x1b, 0x1b, 0x1c),
            // Compressed warm-gray road ramp: faint texture only.
            road_highway: srgb8(0x23, 0x23, 0x22),
            road_major: srgb8(0x1f, 0x1f, 0x1e),
            road_medium: srgb8(0x1c, 0x1c, 0x1b),
            road_minor: srgb8(0x19, 0x19, 0x18),
            path: srgb8(0x18, 0x18, 0x17),
            rail: srgb8_a(0x1e, 0x1e, 0x20, 0.85),
            // Warm-gray boundaries, country clearly stronger than region.
            boundary_country: srgb8_a(0x62, 0x60, 0x5b, 0.55),
            boundary_region: srgb8_a(0x48, 0x46, 0x40, 0.30),
            place_label: srgb8(0x5a, 0x57, 0x50),
            country_label: srgb8(0x67, 0x64, 0x5c),
            water_label: srgb8(0x44, 0x48, 0x51),
        },
        airspace: AirspaceTheme {
            class_a: pair(TEAL, 0.05, 0.72),
            class_b: pair(TEAL, 0.05, 0.72),
            class_c: pair(TEAL, 0.07, 0.78),
            class_d: pair(TEAL, 0.05, 0.7),
            class_e: pair(FAINT_TEAL, 0.02, 0.35),
            class_f: pair(FAINT_TEAL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(TEAL, 0.035, 0.68),
            tmz: pair(WARM_GREY, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Gruvbox-cream symbols on the warm-gray ground.
            airport: srgb(212, 202, 178, 1.0),
            glider: srgb(200, 180, 130, 1.0),
            navaid: srgb(138, 162, 162, 1.0),
            reporting: srgb(220, 215, 200, 1.0),
            obstacle: srgb(198, 132, 124, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(34, 33, 31, 1.0),
        },
        weather: WeatherTheme {
            // Semantic categories tuned to Alduin: olive-leaning VFR,
            // desaturated steel MVFR, muted red IFR, dusty magenta LIFR.
            vfr: srgb(150, 180, 100, 1.0),
            mvfr: srgb(108, 142, 200, 1.0),
            ifr: srgb(210, 100, 100, 1.0),
            lifr: srgb(190, 115, 185, 1.0),
            sigmet: srgb(218, 150, 95, 0.45),
            // Gridded overlays: muted, warm-gray clouds.
            cloud_cover: Colormap::new(&[
                stop(10.0, (152, 150, 146), 0.0),
                stop(40.0, (160, 158, 154), 0.12),
                stop(75.0, (188, 186, 182), 0.28),
                stop(100.0, (212, 210, 206), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (100, 140, 195), 0.0),
                stop(1.0, (100, 140, 195), 0.32),
                stop(5.0, (100, 185, 190), 0.42),
                stop(20.0, (205, 185, 100), 0.5),
                stop(50.0, (200, 95, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (216, 160, 92), 0.0),
                stop(5.0, (212, 148, 80), 0.32),
                stop(15.0, (200, 92, 80), 0.5),
            ]),
        },
        // Route: bright amber above the muted parchment accents; conflicts in
        // ember red.
        route: RouteTheme {
            line: srgb(250, 180, 70, 1.0),
            line_conflict: srgb(228, 84, 76, 1.0),
            handle_fill: srgb(250, 180, 70, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 180, 70, 0.12),
        },
        labels: LabelTheme {
            // Gruvbox cream (#ebdbb2) pulled down to a calm reading tone.
            text: srgb(216, 207, 184, 0.95),
            halo: [0.0; 4],
        },
        // Warm neutral relief to match the warm-gray ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x18, 0x14, 0x12),
            light_tint: tint_from_srgb8(0x86, 0x7f, 0x74),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

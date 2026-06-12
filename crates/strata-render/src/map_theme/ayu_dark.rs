//! "Ayu Dark" — map theme paired with the Ayu Dark UI theme.
//!
//! Ayu Dark's chrome is a deep navy-tinted near-black (`background
//! #0D1016`, panels `#16191F`) under a warm off-white foreground, with
//! bright sky-blue (`#5ac1fe`), signature orange/amber (`#FF8F40` /
//! `#FEB454`), soft red (`#ef7177`) and violet (`#d2a6ff`) accents. The
//! basemap reuses the UI background verbatim as ground and keeps the whole
//! band inside that cool navy cast (road ramp +4..+12 channels, water a
//! shade darker and bluer); the airspace layer carries the accents: sky
//! blue for controlled airspace, ayu red for CTR/ED-R/ED-P, the signature
//! orange for danger areas, amber-sand for glider/para and a desaturated
//! violet-gray TMZ nodding to the magenta accent.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), pastelized from the Ayu Dark accent set.
const SKY: (u8, u8, u8) = (108, 174, 224); // from #5ac1fe — controlled
const FAINT_SKY: (u8, u8, u8) = (126, 168, 200); // class E/F band
const ROSE: (u8, u8, u8) = (226, 118, 124); // from #ef7177 — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (230, 148, 96); // from #ff8f40 — danger areas
const VIOLET_GREY: (u8, u8, u8) = (164, 154, 178); // from #d2a6ff — TMZ
const AMBER: (u8, u8, u8) = (228, 178, 118); // from #feb454 — glider / para-jump
const NEUTRAL: (u8, u8, u8) = (140, 140, 142);

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
    // The UI window background itself: deep navy near-black, b ≈ r + 9.
    let land = srgb8(0x0d, 0x10, 0x16);
    MapTheme {
        id: "ayu-dark",
        name: "Ayu Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // A shade darker and bluer than the navy ground.
            water: srgb8(0x0a, 0x0d, 0x14),
            waterway: srgb8(0x18, 0x20, 0x2c),
            // Landcover: ±1..3 channels around land, all inside the navy cast.
            forest: srgb8(0x0b, 0x0e, 0x13),
            grass: srgb8(0x0c, 0x0f, 0x15),
            farmland: srgb8(0x0e, 0x11, 0x17),
            barren: srgb8(0x0f, 0x12, 0x18),
            glacier: srgb8(0x11, 0x15, 0x1c),
            park: srgb8(0x0c, 0x0f, 0x15),
            urban: srgb8(0x11, 0x14, 0x1b),
            urban_dense: srgb8(0x14, 0x17, 0x1e),
            military: srgb8(0x10, 0x13, 0x19),
            aerodrome: srgb8(0x12, 0x15, 0x1d),
            // Compressed navy road ramp: motorway +12 per channel over land
            // (the major-road tone lands on the UI panel color #16191f).
            road_highway: srgb8(0x19, 0x1c, 0x22),
            road_major: srgb8(0x16, 0x19, 0x1f),
            road_medium: srgb8(0x13, 0x16, 0x1c),
            road_minor: srgb8(0x11, 0x14, 0x1a),
            path: srgb8(0x0f, 0x12, 0x18),
            rail: srgb8_a(0x15, 0x18, 0x21, 0.85),
            // Cool mid-grays; country borders stay clearly visible.
            boundary_country: srgb8_a(0x56, 0x5a, 0x62, 0.55),
            boundary_region: srgb8_a(0x3e, 0x42, 0x4a, 0.30),
            place_label: srgb8(0x53, 0x56, 0x5e),
            country_label: srgb8(0x61, 0x64, 0x6c),
            water_label: srgb8(0x3f, 0x4c, 0x60),
        },
        airspace: AirspaceTheme {
            class_a: pair(SKY, 0.05, 0.72),
            class_b: pair(SKY, 0.05, 0.72),
            class_c: pair(SKY, 0.07, 0.78),
            class_d: pair(SKY, 0.05, 0.7),
            class_e: pair(FAINT_SKY, 0.02, 0.35),
            class_f: pair(FAINT_SKY, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(SKY, 0.035, 0.68),
            tmz: pair(VIOLET_GREY, 0.03, 0.75),
            danger: pair(ORANGE, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(AMBER, 0.045, 0.75),
            para_jump: pair(AMBER, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Warm light gray (ayu's foreground family) on navy ground.
            airport: srgb(196, 194, 186, 1.0),
            glider: srgb(226, 182, 120, 1.0),
            navaid: srgb(140, 158, 180, 1.0),
            reporting: srgb(214, 216, 222, 1.0),
            obstacle: srgb(224, 124, 122, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(24, 28, 36, 1.0),
        },
        weather: WeatherTheme {
            // Ayu's accents, tamed: yellow-green VFR, sky MVFR, soft-red
            // IFR, violet-magenta LIFR.
            vfr: srgb(150, 206, 100, 1.0),
            mvfr: srgb(100, 180, 240, 1.0),
            ifr: srgb(232, 116, 120, 1.0),
            lifr: srgb(208, 150, 238, 1.0),
            sigmet: srgb(236, 150, 86, 0.45),
            // Gridded overlays: muted, with a whisper of the navy cool cast.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 158), 0.0),
                stop(40.0, (156, 160, 166), 0.12),
                stop(75.0, (184, 188, 194), 0.28),
                stop(100.0, (210, 214, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (90, 140, 210), 0.0),
                stop(1.0, (90, 140, 210), 0.32),
                stop(5.0, (90, 190, 200), 0.42),
                stop(20.0, (212, 186, 90), 0.5),
                stop(50.0, (210, 90, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (232, 162, 84), 0.0),
                stop(5.0, (224, 148, 72), 0.32),
                stop(15.0, (210, 92, 76), 0.5),
            ]),
        },
        // Route: ayu's signature bright orange (#ff8f40 vivified); conflicts
        // in a vivid red.
        route: RouteTheme {
            line: srgb(255, 145, 50, 1.0),
            line_conflict: srgb(240, 70, 80, 1.0),
            handle_fill: srgb(255, 145, 50, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 145, 50, 0.12),
        },
        labels: LabelTheme {
            // Ayu's warm off-white foreground (#B3B1AD), slightly lifted.
            text: srgb(185, 183, 178, 0.95),
            halo: [0.0; 4],
        },
        // Relief: navy-leaning shadows, neutral-warm lights (the warm
        // foreground against the cool ground is ayu dark's identity).
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x10, 0x12, 0x18),
            light_tint: tint_from_srgb8(0x82, 0x7e, 0x78),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

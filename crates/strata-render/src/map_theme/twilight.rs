//! "Twilight" — map theme paired with the Twilight UI theme.
//!
//! The classic TextMate Twilight: charcoal chrome (`background #141414`,
//! title bar `#1e1e1e`, translucent panel anchored on the background)
//! whose identity is the khaki/sand primary `#CDA869` with burnt orange
//! `#c06d44`, olive `#afb97a` and a gray-cyan `#778385` as the only cool
//! accent. The basemap is a khaki-whisper charcoal band (land `#141312`,
//! warm r ≥ g > b) with a faintly warm road ramp. Twilight has no real
//! blue, so controlled airspace takes the gray-cyan haze pushed slightly
//! cool; CTR/restricted/prohibited carry the burnt orange, danger a
//! lighter flame, and glider sectors get the hero sand `#CDA869`.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Twilight accent set (sRGB bytes).
const HAZE: (u8, u8, u8) = (112, 138, 152); // base.cyan #778385 cooled — controlled
const FAINT_HAZE: (u8, u8, u8) = (126, 146, 156); // class E/F band
const EMBER: (u8, u8, u8) = (198, 110, 80); // base.red #c06d44 — CTR / ED-R / ED-P
const FLAME: (u8, u8, u8) = (208, 134, 86); // red.light #de7c4c muted — danger areas
const SAND: (u8, u8, u8) = (205, 172, 112); // primary #CDA869 — glider / para
const WARM_GREY: (u8, u8, u8) = (150, 148, 142); // TMZ
const NEUTRAL: (u8, u8, u8) = (138, 137, 134);

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
    // Charcoal with a khaki whisper (r ≥ g > b), just under the #141414
    // UI background.
    let land = srgb8(0x14, 0x13, 0x12);
    MapTheme {
        id: "twilight",
        name: "Twilight",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than the khaki land.
            water: srgb8(0x10, 0x12, 0x15),
            waterway: srgb8(0x1d, 0x20, 0x25),
            // Landcover: olive-leaning whispers around land (Twilight's
            // green/magenta accents are both olive).
            forest: srgb8(0x12, 0x13, 0x10),
            grass: srgb8(0x14, 0x15, 0x11),
            farmland: srgb8(0x16, 0x15, 0x12),
            barren: srgb8(0x17, 0x16, 0x14),
            glacier: srgb8(0x19, 0x19, 0x1a),
            park: srgb8(0x13, 0x14, 0x10),
            urban: srgb8(0x18, 0x17, 0x15),
            urban_dense: srgb8(0x1b, 0x1a, 0x18),
            military: srgb8(0x17, 0x15, 0x11),
            aerodrome: srgb8(0x19, 0x18, 0x1a),
            // Compressed khaki road ramp: faintly warm texture.
            road_highway: srgb8(0x22, 0x1f, 0x1a),
            road_major: srgb8(0x1e, 0x1c, 0x18),
            road_medium: srgb8(0x1b, 0x19, 0x16),
            road_minor: srgb8(0x18, 0x17, 0x14),
            path: srgb8(0x16, 0x15, 0x13),
            rail: srgb8_a(0x1d, 0x1c, 0x1e, 0.85),
            // Warm-gray boundaries; country clearly stronger than region.
            boundary_country: srgb8_a(0x5f, 0x5c, 0x54, 0.55),
            boundary_region: srgb8_a(0x46, 0x44, 0x3d, 0.30),
            place_label: srgb8(0x57, 0x54, 0x4b),
            country_label: srgb8(0x64, 0x5f, 0x53),
            water_label: srgb8(0x43, 0x49, 0x50),
        },
        airspace: AirspaceTheme {
            class_a: pair(HAZE, 0.05, 0.72),
            class_b: pair(HAZE, 0.05, 0.72),
            class_c: pair(HAZE, 0.07, 0.78),
            class_d: pair(HAZE, 0.05, 0.7),
            class_e: pair(FAINT_HAZE, 0.02, 0.35),
            class_f: pair(FAINT_HAZE, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(EMBER, 0.09, 0.8),
            rmz: pair(HAZE, 0.035, 0.68),
            tmz: pair(WARM_GREY, 0.03, 0.75),
            danger: pair(FLAME, 0.06, 0.7),
            restricted: pair(EMBER, 0.12, 0.8),
            prohibited: pair(EMBER, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Soft warm grays (foreground #dcdcdc) with sand and ember kin.
            airport: srgb(210, 203, 188, 1.0),
            glider: srgb(205, 175, 116, 1.0),
            navaid: srgb(142, 156, 166, 1.0),
            reporting: srgb(218, 214, 204, 1.0),
            obstacle: srgb(202, 122, 92, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(32, 31, 29, 1.0),
        },
        weather: WeatherTheme {
            // Semantic categories tuned to Twilight: olive-cast VFR from
            // #afb97a, a desaturated dusk blue for MVFR (the theme has no
            // blue of its own), burnt-orange-leaning IFR, muted mauve LIFR.
            vfr: srgb(140, 185, 110, 1.0),
            mvfr: srgb(110, 140, 200, 1.0),
            ifr: srgb(210, 105, 80, 1.0),
            lifr: srgb(185, 115, 185, 1.0),
            sigmet: srgb(215, 140, 80, 0.45),
            // Gridded overlays: muted, warm-gray clouds.
            cloud_cover: Colormap::new(&[
                stop(10.0, (152, 150, 145), 0.0),
                stop(40.0, (160, 158, 152), 0.12),
                stop(75.0, (186, 184, 178), 0.28),
                stop(100.0, (210, 207, 200), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (95, 135, 200), 0.0),
                stop(1.0, (95, 135, 200), 0.32),
                stop(5.0, (90, 180, 185), 0.42),
                stop(20.0, (208, 182, 92), 0.5),
                stop(50.0, (200, 90, 75), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (212, 158, 88), 0.0),
                stop(5.0, (206, 142, 74), 0.32),
                stop(15.0, (196, 92, 78), 0.5),
            ]),
        },
        // Route: bright gold (primary #CDA869 vivified); conflicts in twilight
        // ember red.
        route: RouteTheme {
            line: srgb(250, 188, 70, 1.0),
            line_conflict: srgb(226, 74, 64, 1.0),
            handle_fill: srgb(250, 188, 70, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 188, 70, 0.12),
        },
        labels: LabelTheme {
            // The theme's soft gray foreground, warmed a touch.
            text: srgb(212, 206, 192, 0.95),
            halo: [0.0; 4],
        },
        // Khaki-warm relief: shadows toward umber, lights toward sand.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x18, 0x14, 0x10),
            light_tint: tint_from_srgb8(0x84, 0x7d, 0x6e),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

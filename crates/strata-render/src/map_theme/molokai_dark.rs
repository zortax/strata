//! "Molokai Dark" — map theme paired with the Molokai Dark UI theme
//! (tomasr/molokai, Monokai lineage).
//!
//! Palette rationale: the basemap is a near-black neutral band with the
//! faintest green-cyan lean, derived from the UI background `#1b1d1e`
//! (the road_medium step lands exactly on it, land sits a few channels
//! below at `#141617`). Airspace anchors come from the Monokai accent set:
//! the CTR / restricted / prohibited family carries the signature Molokai
//! pink (`#f92672`, softened), controlled airspace the cyan-blue
//! (`#66d9ef`, deepened to steel), danger the orange (`#fd971f`),
//! glider/para the yellow (`#e6db74`, muted to sand) and TMZ the purple
//! (`#ae81ff`, grayed). Weather keeps its semantics with Molokai's lime
//! green, cyan-leaning blue, warm red and pink-magenta.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) from the Monokai accent set, pastelized.
const PINK: (u8, u8, u8) = (224, 96, 138); // #f92672 softened — CTR / ED-R / ED-P
const STEEL: (u8, u8, u8) = (98, 156, 186); // #66d9ef deepened — controlled
const FAINT_STEEL: (u8, u8, u8) = (118, 158, 178); // class E/F band
const ORANGE: (u8, u8, u8) = (226, 152, 86); // #fd971f softened — danger areas
const SAND: (u8, u8, u8) = (214, 186, 116); // #e6db74 muted — glider / para
const MAUVE: (u8, u8, u8) = (168, 144, 196); // #ae81ff grayed — TMZ
const NEUTRAL: (u8, u8, u8) = (142, 142, 138); // warm neutral (muted_fg family)

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
    // Near-black ground a few channels below the UI background #1b1d1e,
    // keeping its slightly cool g = r+2 / b = r+3 lean.
    let land = srgb8(0x14, 0x16, 0x17);
    MapTheme {
        id: "molokai-dark",
        name: "Molokai Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than land.
            water: srgb8(0x10, 0x13, 0x16),
            waterway: srgb8(0x1c, 0x24, 0x2b),
            // Landcover: a whisker around land with the faintest green lean
            // (Molokai's lime identity, hard-muted).
            forest: srgb8(0x12, 0x15, 0x14),
            grass: srgb8(0x13, 0x16, 0x15),
            farmland: srgb8(0x15, 0x17, 0x16),
            barren: srgb8(0x16, 0x18, 0x18),
            glacier: srgb8(0x18, 0x1a, 0x1c),
            park: srgb8(0x13, 0x16, 0x15),
            urban: srgb8(0x18, 0x1a, 0x1a),
            urban_dense: srgb8(0x1b, 0x1d, 0x1d),
            military: srgb8(0x17, 0x18, 0x18),
            aerodrome: srgb8(0x19, 0x1b, 0x1d),
            // Compressed road ramp: +2/+4/+7/+10/+14 over land; the medium
            // step coincides with the UI background.
            road_highway: srgb8(0x22, 0x24, 0x25),
            road_major: srgb8(0x1e, 0x20, 0x21),
            road_medium: srgb8(0x1b, 0x1d, 0x1e),
            road_minor: srgb8(0x18, 0x1a, 0x1b),
            path: srgb8(0x16, 0x18, 0x19),
            rail: srgb8_a(0x1c, 0x1d, 0x1f, 0.85),
            // Warm-gray boundaries from the muted foreground #5b5a54.
            boundary_country: srgb8_a(0x5e, 0x5d, 0x58, 0.55),
            boundary_region: srgb8_a(0x46, 0x45, 0x41, 0.30),
            place_label: srgb8(0x56, 0x56, 0x52),
            country_label: srgb8(0x63, 0x63, 0x5e),
            water_label: srgb8(0x3e, 0x4a, 0x50),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.05, 0.72),
            class_b: pair(STEEL, 0.05, 0.72),
            class_c: pair(STEEL, 0.07, 0.78),
            class_d: pair(STEEL, 0.05, 0.7),
            class_e: pair(FAINT_STEEL, 0.02, 0.35),
            class_f: pair(FAINT_STEEL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(PINK, 0.09, 0.8),
            rmz: pair(STEEL, 0.035, 0.68),
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(ORANGE, 0.06, 0.7),
            restricted: pair(PINK, 0.12, 0.8),
            prohibited: pair(PINK, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(202, 200, 192, 1.0),
            glider: srgb(212, 186, 122, 1.0),
            navaid: srgb(130, 164, 184, 1.0),
            reporting: srgb(218, 218, 220, 1.0),
            obstacle: srgb(218, 108, 128, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 32, 32, 1.0),
        },
        weather: WeatherTheme {
            // Molokai lime / cyan-blue / warm red / pink-magenta.
            vfr: srgb(130, 200, 80, 1.0),
            mvfr: srgb(102, 160, 224, 1.0),
            ifr: srgb(226, 96, 88, 1.0),
            lifr: srgb(216, 100, 200, 1.0),
            sigmet: srgb(230, 150, 70, 0.45),
            // Gridded overlays, muted to match the dark band.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 151, 150), 0.0),
                stop(40.0, (158, 161, 160), 0.12),
                stop(75.0, (186, 189, 188), 0.28),
                stop(100.0, (212, 215, 213), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (90, 150, 210), 0.0),
                stop(1.0, (90, 150, 210), 0.32),
                stop(5.0, (80, 190, 200), 0.42),
                stop(20.0, (210, 190, 90), 0.5),
                stop(50.0, (212, 90, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (224, 164, 90), 0.0),
                stop(5.0, (218, 150, 76), 0.32),
                stop(15.0, (210, 92, 82), 0.5),
            ]),
        },
        // Route: molokai spring green #a6e22e — the signature accent no
        // airspace uses; conflicts in #f92672 magenta-red.
        route: RouteTheme {
            line: srgb(166, 226, 46, 1.0),
            line_conflict: srgb(244, 56, 110, 1.0),
            handle_fill: srgb(166, 226, 46, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(166, 226, 46, 0.12),
        },
        labels: LabelTheme {
            // From the UI foreground #f8f8f2, softened.
            text: srgb(212, 210, 202, 0.95),
            halo: [0.0; 4],
        },
        // Near-neutral relief with the band's faint green lean.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x15, 0x16, 0x13),
            light_tint: tint_from_srgb8(0x82, 0x84, 0x7c),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

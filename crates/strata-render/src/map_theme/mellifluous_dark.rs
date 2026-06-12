//! "Mellifluous Dark" — map theme paired with the Mellifluous Dark UI
//! theme. Mellifluous is soft and muted: a neutral `#1A1A1A` window with a
//! cool blue-grey panel (`#282c34`) and cool foreground (`#abb2bf`), so the
//! ground is a neutral near-black with a faint cool lift and whisper-level
//! olive/warm landcover hints (the theme's green is olive `#828040`, its
//! yellow a soft amber). Airspaces use the theme's already-muted accents
//! almost verbatim: blue `#5481c9` (toward the indigo primary `#5A6599`)
//! for controlled airspace, the dusty red `#C95954` for CTR/restricted/
//! prohibited, a warmer terracotta between red and amber for danger, the
//! amber `#c98d54` softened to sand for glider/para, and the mauve magenta
//! `#9C6995` greyed for TMZ. Boundaries and labels follow the cool
//! `#828997` muted-foreground scale.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues — the Mellifluous accent set, barely pastelized (it ships
// muted already).
const STEEL: (u8, u8, u8) = (100, 128, 186); // blue #5481c9 / primary #5A6599 — controlled
const FAINT_STEEL: (u8, u8, u8) = (118, 136, 170); // class E/F band
const ROSE: (u8, u8, u8) = (198, 96, 92); // red #C95954 — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (206, 118, 76); // danger (red → amber blend)
const MAUVE_GREY: (u8, u8, u8) = (148, 130, 146); // TMZ — magenta #9C6995 greyed
const SAND: (u8, u8, u8) = (198, 150, 100); // amber #c98d54 softened — glider / para
const NEUTRAL: (u8, u8, u8) = (138, 140, 146); // cool neutral (#828997 scale)

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
    // Neutral soft black just below the #1A1A1A window background, with a
    // faint cool lift toward the #282c34 panel.
    let land = srgb8(0x16, 0x16, 0x18);
    MapTheme {
        id: "mellifluous-dark",
        name: "Mellifluous Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Slightly cooler and darker — water recedes, softly.
            water: srgb8(0x12, 0x14, 0x19),
            waterway: srgb8(0x20, 0x24, 0x2c),
            // Landcover: whisper-level hints of the olive green and soft
            // amber, otherwise neutral.
            forest: srgb8(0x14, 0x15, 0x11),
            grass: srgb8(0x15, 0x16, 0x13),
            farmland: srgb8(0x17, 0x16, 0x14),
            barren: srgb8(0x18, 0x17, 0x15),
            glacier: srgb8(0x1a, 0x1a, 0x1d),
            park: srgb8(0x14, 0x15, 0x12),
            urban: srgb8(0x1a, 0x1a, 0x1b),
            urban_dense: srgb8(0x1d, 0x1d, 0x1e),
            military: srgb8(0x19, 0x18, 0x18),
            aerodrome: srgb8(0x1b, 0x1b, 0x1e),
            // Compressed neutral road ramp: +14 channels at the motorway
            // down to +2 for paths — faint texture only.
            road_highway: srgb8(0x24, 0x24, 0x26),
            road_major: srgb8(0x20, 0x20, 0x22),
            road_medium: srgb8(0x1d, 0x1d, 0x1f),
            road_minor: srgb8(0x1a, 0x1a, 0x1c),
            path: srgb8(0x18, 0x18, 0x1a),
            rail: srgb8_a(0x1e, 0x1e, 0x22, 0.85),
            // Cool grey-blue boundaries from the #828997 muted scale.
            boundary_country: srgb8_a(0x5e, 0x61, 0x68, 0.55),
            boundary_region: srgb8_a(0x45, 0x47, 0x4e, 0.30),
            place_label: srgb8(0x56, 0x59, 0x60),
            country_label: srgb8(0x63, 0x66, 0x6d),
            water_label: srgb8(0x42, 0x48, 0x54),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.05, 0.72),
            class_b: pair(STEEL, 0.05, 0.72),
            class_c: pair(STEEL, 0.07, 0.78),
            class_d: pair(STEEL, 0.05, 0.7),
            class_e: pair(FAINT_STEEL, 0.02, 0.35),
            class_f: pair(FAINT_STEEL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(STEEL, 0.035, 0.68),
            tmz: pair(MAUVE_GREY, 0.03, 0.75),
            danger: pair(TERRACOTTA, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Cool light greys from the #abb2bf foreground family.
            airport: srgb(198, 202, 210, 1.0),
            glider: srgb(200, 162, 110, 1.0),
            navaid: srgb(142, 156, 182, 1.0),
            reporting: srgb(214, 218, 226, 1.0),
            obstacle: srgb(210, 114, 108, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 30, 34, 1.0),
        },
        weather: WeatherTheme {
            // Soft and semantically intact: the olive green is too yellow
            // for VFR, so VFR borrows the cyan's freshness shifted green.
            vfr: srgb(84, 196, 140, 1.0),
            mvfr: srgb(94, 134, 212, 1.0),
            ifr: srgb(210, 96, 92, 1.0),
            lifr: srgb(186, 114, 178, 1.0),
            sigmet: srgb(206, 140, 84, 0.45),
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 158), 0.0),
                stop(40.0, (156, 160, 166), 0.12),
                stop(75.0, (184, 188, 194), 0.28),
                stop(100.0, (210, 214, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (94, 134, 196), 0.0),
                stop(1.0, (94, 134, 196), 0.32),
                stop(5.0, (90, 186, 182), 0.42),
                stop(20.0, (206, 182, 94), 0.5),
                stop(50.0, (200, 90, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (212, 156, 90), 0.0),
                stop(5.0, (208, 142, 78), 0.32),
                stop(15.0, (196, 92, 82), 0.5),
            ]),
        },
        // Route: warm amber a step brighter than the muted accents; conflicts
        // in mellifluous red.
        route: RouteTheme {
            line: srgb(240, 178, 90, 1.0),
            line_conflict: srgb(230, 76, 74, 1.0),
            handle_fill: srgb(240, 178, 90, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(240, 178, 90, 0.12),
        },
        labels: LabelTheme {
            // The cool #abb2bf foreground, lightened a touch for the map.
            text: srgb(196, 202, 214, 0.95),
            halo: [0.0; 4],
        },
        // Cool-neutral relief matching the blue-grey panel cast.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x15, 0x15, 0x1a),
            light_tint: tint_from_srgb8(0x7e, 0x80, 0x88),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

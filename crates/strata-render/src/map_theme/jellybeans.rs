//! "Jellybeans" — map theme paired with the Jellybeans UI theme.
//!
//! Jellybeans is candy on charcoal: a pure-neutral dark gray window
//! (`background`/`panel #151515`, title bar `#101010`) under soft candy
//! accents — pastel blue `#97bedc` (the primary), candy red `#e27373`,
//! apricot `#ffba7b`, mauve `#B294BB`, teal `#00988e`. The basemap is a
//! strictly neutral charcoal band (land `#121212`, a notch below the UI
//! background) so all color belongs to the overlays: pastel blue for
//! controlled airspace, candy red for CTR/restricted/prohibited, apricot
//! danger, sand glider sectors, teal RMZ and mauve TMZ give the map the
//! theme's jellybean variety without breaking chart semantics.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Jellybeans accent set (sRGB bytes).
const BLUE: (u8, u8, u8) = (135, 172, 205); // primary #97bedc deepened — controlled
const FAINT_BLUE: (u8, u8, u8) = (140, 162, 188); // class E/F band
const RED: (u8, u8, u8) = (220, 115, 115); // base.red #e27373 — CTR / ED-R / ED-P
const APRICOT: (u8, u8, u8) = (230, 160, 100); // base.yellow #ffba7b muted — danger areas
const SAND: (u8, u8, u8) = (228, 196, 134); // yellow.light #ffdca0 toward sand — glider / para
const TEAL: (u8, u8, u8) = (84, 158, 150); // base.cyan #00988e lifted — RMZ
const MAUVE: (u8, u8, u8) = (170, 150, 180); // base.magenta #B294BB — TMZ
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
    // Pure-neutral charcoal a notch below the #151515 UI background.
    let land = srgb8(0x12, 0x12, 0x12);
    MapTheme {
        id: "jellybeans",
        name: "Jellybeans",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than the neutral land.
            water: srgb8(0x0f, 0x10, 0x13),
            waterway: srgb8(0x1c, 0x1f, 0x24),
            // Landcover: strictly neutral grays a whisker around land —
            // all the candy color stays in the overlays.
            forest: srgb8(0x10, 0x10, 0x11),
            grass: srgb8(0x11, 0x11, 0x12),
            farmland: srgb8(0x13, 0x13, 0x14),
            barren: srgb8(0x15, 0x15, 0x15),
            glacier: srgb8(0x16, 0x16, 0x18),
            park: srgb8(0x11, 0x11, 0x12),
            urban: srgb8(0x16, 0x16, 0x17),
            urban_dense: srgb8(0x19, 0x19, 0x1a),
            military: srgb8(0x15, 0x15, 0x16),
            aerodrome: srgb8(0x17, 0x17, 0x19),
            // Compressed neutral road ramp: faint texture, no hue.
            road_highway: srgb8(0x1f, 0x1f, 0x1f),
            road_major: srgb8(0x1c, 0x1c, 0x1c),
            road_medium: srgb8(0x19, 0x19, 0x19),
            road_minor: srgb8(0x16, 0x16, 0x16),
            path: srgb8(0x14, 0x14, 0x14),
            rail: srgb8_a(0x1a, 0x1a, 0x1d, 0.85),
            boundary_country: srgb8_a(0x5c, 0x5c, 0x60, 0.55),
            boundary_region: srgb8_a(0x42, 0x42, 0x47, 0.30),
            place_label: srgb8(0x54, 0x54, 0x58),
            country_label: srgb8(0x61, 0x61, 0x65),
            water_label: srgb8(0x40, 0x45, 0x4e),
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
            rmz: pair(TEAL, 0.035, 0.68),
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(APRICOT, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Cream-tinted symbols (foreground #E8E8D3) on charcoal.
            airport: srgb(212, 212, 198, 1.0),
            glider: srgb(224, 192, 132, 1.0),
            navaid: srgb(150, 172, 198, 1.0),
            reporting: srgb(224, 224, 212, 1.0),
            obstacle: srgb(216, 122, 120, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(28, 28, 31, 1.0),
        },
        weather: WeatherTheme {
            // Candy-pastel categories straight from the accent row:
            // green #94b979, blue #97bedc, red #e27373, magenta #B294BB.
            vfr: srgb(125, 190, 115, 1.0),
            mvfr: srgb(115, 155, 225, 1.0),
            ifr: srgb(222, 105, 105, 1.0),
            lifr: srgb(195, 125, 200, 1.0),
            sigmet: srgb(236, 158, 96, 0.45),
            // Gridded overlays: muted, neutral clouds.
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 152, 156), 0.0),
                stop(40.0, (158, 160, 164), 0.12),
                stop(75.0, (186, 188, 192), 0.28),
                stop(100.0, (210, 212, 216), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (100, 142, 208), 0.0),
                stop(1.0, (100, 142, 208), 0.32),
                stop(5.0, (88, 184, 196), 0.42),
                stop(20.0, (215, 190, 95), 0.5),
                stop(50.0, (215, 95, 85), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (240, 175, 105), 0.0),
                stop(5.0, (228, 152, 88), 0.32),
                stop(15.0, (212, 92, 82), 0.5),
            ]),
        },
        // Route: bright candy amber; conflicts in jellybeans red.
        route: RouteTheme {
            line: srgb(252, 186, 80, 1.0),
            line_conflict: srgb(232, 76, 84, 1.0),
            handle_fill: srgb(252, 186, 80, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(252, 186, 80, 0.12),
        },
        labels: LabelTheme {
            // The theme's warm off-white foreground (#E8E8D3), dimmed.
            text: srgb(224, 224, 208, 0.95),
            halo: [0.0; 4],
        },
        // Neutral relief to match the colorless ground.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x13, 0x12),
            light_tint: tint_from_srgb8(0x80, 0x7c, 0x76),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

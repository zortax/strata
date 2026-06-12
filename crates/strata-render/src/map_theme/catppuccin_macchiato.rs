//! "Catppuccin Macchiato" — map theme paired with the Catppuccin Macchiato
//! UI theme.
//!
//! The middle dark Catppuccin flavor: one notch lighter than Mocha, with a
//! stronger blue lean. The ground drops a few channels below the UI
//! `background` `#1E2030` and keeps its b ≈ r + 17 ratio across the band so
//! map and chrome read as one surface. Airspace anchors: controlled =
//! Macchiato blue `#8aadf4` as steel, CTR/restricted/prohibited = red
//! `#ed8796` as dusty rose, danger = peach `#f5a97f`, glider/para = yellow
//! `#eed49f` as sand, TMZ a lavender gray. Weather hues lean toward
//! Macchiato green/blue/red/pink; terrain shadows follow the violet band.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Macchiato accent set (sRGB bytes), pastelized.
const STEEL: (u8, u8, u8) = (132, 160, 212); // blue #8aadf4 — controlled
const FAINT_STEEL: (u8, u8, u8) = (142, 166, 206); // class E/F band
const ROSE: (u8, u8, u8) = (214, 124, 140); // red #ed8796 — CTR / ED-R / ED-P
const PEACH: (u8, u8, u8) = (212, 142, 108); // peach #f5a97f — danger areas
const LAVENDER_GREY: (u8, u8, u8) = (158, 156, 172); // TMZ
const SAND: (u8, u8, u8) = (214, 186, 134); // yellow #eed49f — glider / para
const NEUTRAL: (u8, u8, u8) = (144, 145, 152);

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
    // Dark blue-violet ground just under the Macchiato UI background.
    let land = srgb8(0x16, 0x18, 0x27);
    MapTheme {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and a touch bluer than land.
            water: srgb8(0x13, 0x15, 0x23),
            waterway: srgb8(0x20, 0x26, 0x3a),
            // Landcover: a whisker around land, staying in the blue band.
            forest: srgb8(0x14, 0x16, 0x25),
            grass: srgb8(0x15, 0x17, 0x26),
            farmland: srgb8(0x17, 0x19, 0x28),
            barren: srgb8(0x18, 0x1a, 0x29),
            glacier: srgb8(0x1b, 0x1e, 0x30),
            park: srgb8(0x15, 0x17, 0x26),
            urban: srgb8(0x1a, 0x1c, 0x2e),
            urban_dense: srgb8(0x1d, 0x1f, 0x33),
            military: srgb8(0x19, 0x1b, 0x2b),
            aerodrome: srgb8(0x1b, 0x1d, 0x31),
            // Compressed road ramp: motorway ~+14 channels over land,
            // then +10 / +7 / +4 / +2 — faint texture, never lines.
            road_highway: srgb8(0x24, 0x26, 0x35),
            road_major: srgb8(0x20, 0x22, 0x31),
            road_medium: srgb8(0x1d, 0x1f, 0x2e),
            road_minor: srgb8(0x1a, 0x1c, 0x2b),
            path: srgb8(0x18, 0x1a, 0x29),
            rail: srgb8_a(0x1e, 0x20, 0x2f, 0.85),
            // Boundaries in Macchiato's blue-leaning overlay grays.
            boundary_country: srgb8_a(0x60, 0x62, 0x7a, 0.55),
            boundary_region: srgb8_a(0x47, 0x49, 0x62, 0.30),
            place_label: srgb8(0x56, 0x58, 0x70),
            country_label: srgb8(0x63, 0x65, 0x7e),
            water_label: srgb8(0x44, 0x4c, 0x66),
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
            tmz: pair(LAVENDER_GREY, 0.03, 0.75),
            danger: pair(PEACH, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Lavender-white symbols echo Macchiato's text scale.
            airport: srgb(200, 202, 214, 1.0),
            glider: srgb(212, 190, 140, 1.0),
            navaid: srgb(150, 162, 188, 1.0),
            reporting: srgb(218, 222, 234, 1.0),
            obstacle: srgb(214, 130, 142, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(28, 30, 42, 1.0),
        },
        weather: WeatherTheme {
            // Macchiato green / blue / red / pink, channel dominance intact.
            vfr: srgb(110, 192, 132, 1.0),
            mvfr: srgb(112, 152, 224, 1.0),
            ifr: srgb(222, 112, 128, 1.0),
            lifr: srgb(206, 124, 200, 1.0),
            sigmet: srgb(224, 152, 102, 0.45),
            // Muted gridded overlays with a faint cool lean.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 150, 158), 0.0),
                stop(40.0, (156, 158, 166), 0.12),
                stop(75.0, (184, 186, 196), 0.28),
                stop(100.0, (208, 210, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (112, 142, 222), 0.0),
                stop(1.0, (112, 142, 222), 0.32),
                stop(5.0, (110, 192, 204), 0.42),
                stop(20.0, (216, 192, 108), 0.5),
                stop(50.0, (212, 100, 108), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (224, 164, 100), 0.0),
                stop(5.0, (218, 150, 88), 0.32),
                stop(15.0, (205, 100, 95), 0.5),
            ]),
        },
        // Route: Catppuccin pink — the brand accent no airspace uses;
        // conflicts in macchiato red.
        route: RouteTheme {
            line: srgb(244, 154, 200, 1.0),
            line_conflict: srgb(230, 90, 96, 1.0),
            handle_fill: srgb(244, 154, 200, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(244, 154, 200, 0.12),
        },
        labels: LabelTheme {
            // Macchiato foreground #cad3f5, slightly desaturated.
            text: srgb(200, 206, 224, 0.95),
            halo: [0.0; 4],
        },
        // Relief shadows toward the band's violet, lights neutral-cool.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x17, 0x13, 0x22),
            light_tint: tint_from_srgb8(0x82, 0x7e, 0x88),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

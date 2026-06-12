//! "Catppuccin Mocha" — map theme paired with the Catppuccin Mocha UI theme.
//!
//! The darkest Catppuccin flavor: the ground sits a few channels below the
//! UI `background` `#181825` and keeps Mocha's blue-violet lean (b ≈ r + 11)
//! through the whole near-black basemap band, so the map reads as the
//! deepest layer under the Mocha chrome. Airspace anchors: controlled
//! airspace = Mocha blue `#89b4fa` pastelized to steel, CTR/restricted/
//! prohibited = Mocha red `#f38ba8` as a dusty pink-rose, danger = peach
//! `#fab387`, glider/para = yellow `#f9e2af` darkened to sand, TMZ a
//! lavender gray. Weather keeps semantic hues tuned toward Mocha green/
//! blue/red/pink; terrain shadows lean the band's violet.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Mocha accent set (sRGB bytes), pastelized.
const STEEL: (u8, u8, u8) = (130, 160, 210); // blue #89b4fa — controlled
const FAINT_STEEL: (u8, u8, u8) = (140, 162, 204); // class E/F band
const ROSE: (u8, u8, u8) = (212, 118, 140); // red #f38ba8 — CTR / ED-R / ED-P
const PEACH: (u8, u8, u8) = (210, 140, 105); // peach #fab387 — danger areas
const LAVENDER_GREY: (u8, u8, u8) = (158, 154, 174); // TMZ
const SAND: (u8, u8, u8) = (215, 185, 130); // yellow #f9e2af — glider / para
const NEUTRAL: (u8, u8, u8) = (146, 146, 152);

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
    // Near-black blue-violet ground, just under the Mocha UI background.
    let land = srgb8(0x13, 0x13, 0x1e);
    MapTheme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and a touch bluer than land.
            water: srgb8(0x10, 0x11, 0x1c),
            waterway: srgb8(0x1c, 0x20, 0x30),
            // Landcover: a whisker around land, all in the violet band.
            forest: srgb8(0x11, 0x11, 0x20),
            grass: srgb8(0x12, 0x12, 0x1d),
            farmland: srgb8(0x14, 0x14, 0x1f),
            barren: srgb8(0x15, 0x15, 0x1f),
            glacier: srgb8(0x18, 0x18, 0x26),
            park: srgb8(0x12, 0x12, 0x1e),
            urban: srgb8(0x17, 0x17, 0x2a),
            urban_dense: srgb8(0x1a, 0x1a, 0x2d),
            military: srgb8(0x16, 0x16, 0x24),
            aerodrome: srgb8(0x18, 0x18, 0x28),
            // Compressed road ramp: motorway ~+14 channels over land,
            // then +10 / +7 / +4 / +2 — faint texture, never lines.
            road_highway: srgb8(0x21, 0x21, 0x30),
            road_major: srgb8(0x1d, 0x1d, 0x28),
            road_medium: srgb8(0x1a, 0x1a, 0x25),
            road_minor: srgb8(0x17, 0x17, 0x22),
            path: srgb8(0x15, 0x15, 0x20),
            rail: srgb8_a(0x1b, 0x1b, 0x26, 0.85),
            // Boundaries in Mocha's violet-leaning overlay grays.
            boundary_country: srgb8_a(0x5e, 0x5c, 0x72, 0.55),
            boundary_region: srgb8_a(0x45, 0x44, 0x5a, 0.30),
            place_label: srgb8(0x54, 0x52, 0x6a),
            country_label: srgb8(0x61, 0x5f, 0x78),
            water_label: srgb8(0x42, 0x49, 0x60),
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
            // Lavender-white symbols echo Mocha's text scale.
            airport: srgb(198, 200, 212, 1.0),
            glider: srgb(212, 188, 134, 1.0),
            navaid: srgb(150, 162, 188, 1.0),
            reporting: srgb(216, 220, 232, 1.0),
            obstacle: srgb(212, 128, 140, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(28, 28, 40, 1.0),
        },
        weather: WeatherTheme {
            // Mocha green / blue / red / pink, channel dominance intact.
            vfr: srgb(108, 195, 130, 1.0),
            mvfr: srgb(110, 150, 225, 1.0),
            ifr: srgb(220, 110, 125, 1.0),
            lifr: srgb(205, 120, 200, 1.0),
            sigmet: srgb(225, 150, 100, 0.45),
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
        // Route: Catppuccin pink (#f5c2e7 family) — the brand accent no
        // airspace uses; conflicts in mocha red.
        route: RouteTheme {
            line: srgb(245, 158, 204, 1.0),
            line_conflict: srgb(232, 92, 100, 1.0),
            handle_fill: srgb(245, 158, 204, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(245, 158, 204, 0.12),
        },
        labels: LabelTheme {
            // Mocha foreground #cdd6f4, slightly desaturated.
            text: srgb(202, 206, 222, 0.95),
            halo: [0.0; 4],
        },
        // Relief shadows toward the band's violet, lights neutral-cool.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x16, 0x12, 0x20),
            light_tint: tint_from_srgb8(0x80, 0x7c, 0x84),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

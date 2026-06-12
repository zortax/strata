//! "Solarized Dark" — map theme paired with the Solarized Dark UI theme.
//!
//! Solarized Dark's chrome is the famous deep teal base03 `#002B36`
//! (`r = 0`, blue over green). The ground band keeps exactly that hue
//! ratio a few steps below the window background, so the map reads as the
//! deepest layer of the same teal well — note water consequently also has
//! `r = 0` to stay cooler than land. Airspace identity comes from the
//! classic Solarized accent row (the parents of the UI theme's muted
//! `base.*` colors), pastelized for the dark ground: blue `#268BD2` for
//! controlled airspace, red `#DC322F` for CTR/restricted/prohibited,
//! orange `#CB4B16` for danger, yellow `#B58900` as glider sand, and cyan
//! `#2AA198` tinting the wide class E/F band. Labels are base2/base3
//! cream; boundaries sit on the base01/base00 grey-cyan scale.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), pastelized classic Solarized accents.
const BLUE: (u8, u8, u8) = (70, 130, 185); // blue #268BD2 — controlled
const FAINT_CYAN: (u8, u8, u8) = (98, 152, 148); // cyan #2AA198 — class E/F band
const ROSE: (u8, u8, u8) = (200, 90, 85); // red #DC322F — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (195, 115, 75); // orange #CB4B16 — danger areas
const SAND: (u8, u8, u8) = (185, 155, 85); // yellow #B58900 — glider / para
const TMZ_GREY: (u8, u8, u8) = (130, 140, 140); // base0 grey-cyan — TMZ
const NEUTRAL: (u8, u8, u8) = (125, 132, 132);

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
    // base03 hue (#002B36) dropped a few steps: the deep teal well.
    let land = srgb8(0x00, 0x24, 0x2d);
    MapTheme {
        id: "solarized-dark",
        name: "Solarized Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Deeper into the well; r stays 0 so water is at least as cool
            // as the (already very cool) land.
            water: srgb8(0x00, 0x1e, 0x28),
            waterway: srgb8(0x14, 0x3e, 0x4b),
            // Landcover: a whisker around `land`, all inside the teal band.
            forest: srgb8(0x00, 0x22, 0x2a),
            grass: srgb8(0x00, 0x23, 0x2b),
            farmland: srgb8(0x02, 0x25, 0x2e),
            barren: srgb8(0x04, 0x26, 0x2e),
            glacier: srgb8(0x08, 0x2a, 0x34),
            park: srgb8(0x00, 0x23, 0x2b),
            urban: srgb8(0x06, 0x29, 0x32),
            urban_dense: srgb8(0x0a, 0x2d, 0x36),
            military: srgb8(0x05, 0x28, 0x2f),
            aerodrome: srgb8(0x08, 0x2a, 0x35),
            // Compressed teal road ramp — faint texture barely above ground
            // (highway/land luma ≈ 1.55).
            road_highway: srgb8(0x10, 0x34, 0x3e),
            road_major: srgb8(0x0a, 0x2e, 0x38),
            road_medium: srgb8(0x06, 0x2a, 0x34),
            road_minor: srgb8(0x04, 0x28, 0x31),
            path: srgb8(0x02, 0x26, 0x2f),
            rail: srgb8_a(0x08, 0x2c, 0x38, 0.85),
            // Boundaries on the base01 (#586E75) grey-cyan scale.
            boundary_country: srgb8_a(0x58, 0x6e, 0x75, 0.55),
            boundary_region: srgb8_a(0x41, 0x52, 0x58, 0.30),
            place_label: srgb8(0x51, 0x64, 0x6b),
            country_label: srgb8(0x5e, 0x71, 0x77),
            water_label: srgb8(0x3e, 0x5b, 0x66),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.05, 0.72),
            class_b: pair(BLUE, 0.05, 0.72),
            class_c: pair(BLUE, 0.07, 0.78),
            class_d: pair(BLUE, 0.05, 0.7),
            class_e: pair(FAINT_CYAN, 0.02, 0.35),
            class_f: pair(FAINT_CYAN, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(BLUE, 0.035, 0.68),
            tmz: pair(TMZ_GREY, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Cream base2 symbols on the teal well.
            airport: srgb(210, 205, 188, 1.0),
            glider: srgb(195, 168, 105, 1.0),
            navaid: srgb(130, 150, 165, 1.0),
            reporting: srgb(220, 217, 205, 1.0),
            obstacle: srgb(200, 110, 95, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(5, 35, 43, 1.0),
        },
        weather: WeatherTheme {
            // Classic Solarized accents, pastelized; semantics intact.
            vfr: srgb(110, 175, 95, 1.0),   // green #859900
            mvfr: srgb(85, 135, 215, 1.0),  // blue #268BD2
            ifr: srgb(215, 95, 90, 1.0),    // red #DC322F
            lifr: srgb(200, 90, 170, 1.0),  // magenta #D33682
            sigmet: srgb(210, 125, 70, 0.45), // orange #CB4B16
            // Gridded overlays: muted, faintly cool neutrals.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 155), 0.0),
                stop(40.0, (156, 160, 163), 0.12),
                stop(75.0, (184, 188, 191), 0.28),
                stop(100.0, (211, 214, 217), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (75, 125, 200), 0.0),
                stop(1.0, (75, 125, 200), 0.32),
                stop(5.0, (75, 170, 185), 0.42),
                stop(20.0, (200, 180, 85), 0.5),
                stop(50.0, (200, 85, 75), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (205, 155, 85), 0.0),
                stop(5.0, (200, 140, 70), 0.32),
                stop(15.0, (195, 85, 75), 0.5),
            ]),
        },
        // Route: bright solarized orange (#cb4b16 vivified) over the deep teal
        // ground; conflicts in solarized red.
        route: RouteTheme {
            line: srgb(255, 154, 40, 1.0),
            line_conflict: srgb(235, 64, 52, 1.0),
            handle_fill: srgb(255, 154, 40, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 154, 40, 0.12),
        },
        labels: LabelTheme {
            // base2 cream (#EEE8D5 family), slightly softened.
            text: srgb(232, 226, 208, 0.95),
            halo: [0.0; 4],
        },
        // Relief tinted into the teal well; lights stay grey-green neutral.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x06, 0x20, 0x28),
            light_tint: tint_from_srgb8(0x7e, 0x85, 0x80),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

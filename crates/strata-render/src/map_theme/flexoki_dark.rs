//! "Flexoki Dark" — map theme paired with the Flexoki Dark UI theme.
//!
//! Flexoki is kepano's "ink on paper" palette; the dark mode is warm black
//! ink (`background #100F0F`, `panel #1C1B1A`, warm `r ≥ g ≥ b` cast).
//! The ground band sits just above the window background and keeps that
//! warm cast throughout the road ramp and landcover. Airspace identity
//! comes from Flexoki's light accent row: paper-blue `#4385BE` for
//! controlled airspace, the red `#D14D41` for CTR/restricted/prohibited,
//! orange `#DA702C` for danger areas, yellow `#D0A215` as glider sand, and
//! the signature teal/cyan primary `#3AA99F` tints the wide class E/F band
//! so the map reads unmistakably Flexoki next to the UI chrome.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), pastelized from the Flexoki Dark accents.
const BLUE: (u8, u8, u8) = (108, 142, 186); // base.blue.light #4385BE — controlled
const FAINT_TEAL: (u8, u8, u8) = (110, 160, 152); // primary cyan #3AA99F — class E/F band
const ROSE: (u8, u8, u8) = (200, 95, 86); // base.red.light #D14D41 — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (204, 128, 84); // syntax orange #DA702C — danger areas
const SAND: (u8, u8, u8) = (198, 166, 100); // base.yellow.light #D0A215 — glider / para
const TMZ_GREY: (u8, u8, u8) = (150, 146, 138); // warm ink grey — TMZ
const NEUTRAL: (u8, u8, u8) = (140, 138, 132); // muted_fg family

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
    // Warm ink, a couple of channels above the UI background #100F0F so
    // the map reads as the deepest layer of the same warm-black chrome.
    let land = srgb8(0x12, 0x11, 0x10);
    MapTheme {
        id: "flexoki-dark",
        name: "Flexoki Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker — the one cool note in the warm band.
            water: srgb8(0x0e, 0x0f, 0x12),
            waterway: srgb8(0x1c, 0x20, 0x28),
            // Landcover: a whisker around `land`, keeping the warm cast.
            forest: srgb8(0x10, 0x0f, 0x0e),
            grass: srgb8(0x11, 0x10, 0x0f),
            farmland: srgb8(0x13, 0x12, 0x11),
            barren: srgb8(0x14, 0x13, 0x12),
            glacier: srgb8(0x16, 0x16, 0x17),
            park: srgb8(0x11, 0x10, 0x0f),
            urban: srgb8(0x16, 0x15, 0x14),
            urban_dense: srgb8(0x19, 0x18, 0x17),
            military: srgb8(0x15, 0x14, 0x13),
            aerodrome: srgb8(0x17, 0x16, 0x18),
            // Compressed warm road ramp: faint ink strokes, never lines that
            // compete with the airspace overlays (highway/land luma ≈ 1.75).
            road_highway: srgb8(0x1f, 0x1e, 0x1c),
            road_major: srgb8(0x1b, 0x1a, 0x18),
            road_medium: srgb8(0x18, 0x17, 0x16),
            road_minor: srgb8(0x16, 0x15, 0x14),
            path: srgb8(0x14, 0x13, 0x12),
            rail: srgb8_a(0x19, 0x18, 0x19, 0.85),
            // Boundaries in Flexoki's warm greys (muted_fg #878580 family).
            boundary_country: srgb8_a(0x60, 0x5e, 0x58, 0.55),
            boundary_region: srgb8_a(0x46, 0x44, 0x3f, 0.30),
            place_label: srgb8(0x56, 0x55, 0x50),
            country_label: srgb8(0x63, 0x62, 0x5c),
            water_label: srgb8(0x46, 0x49, 0x4e),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.05, 0.72),
            class_b: pair(BLUE, 0.05, 0.72),
            class_c: pair(BLUE, 0.07, 0.78),
            class_d: pair(BLUE, 0.05, 0.7),
            class_e: pair(FAINT_TEAL, 0.02, 0.35),
            class_f: pair(FAINT_TEAL, 0.018, 0.3),
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
            // Paper-light symbols on ink (foreground #CECDC3 family).
            airport: srgb(205, 200, 190, 1.0),
            glider: srgb(200, 170, 110, 1.0),
            navaid: srgb(140, 158, 178, 1.0),
            reporting: srgb(215, 213, 205, 1.0),
            obstacle: srgb(205, 110, 100, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(30, 29, 28, 1.0),
        },
        weather: WeatherTheme {
            // Flexoki light accents, semantically intact.
            vfr: srgb(130, 175, 90, 1.0),     // green #879A39
            mvfr: srgb(95, 140, 215, 1.0),    // blue #4385BE
            ifr: srgb(215, 95, 85, 1.0),      // red #D14D41
            lifr: srgb(200, 90, 165, 1.0),    // magenta #CE5D97
            sigmet: srgb(215, 130, 70, 0.45), // orange #DA702C
            // Gridded overlays: muted, warm-leaning neutrals.
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 150, 146), 0.0),
                stop(40.0, (160, 158, 154), 0.12),
                stop(75.0, (188, 186, 182), 0.28),
                stop(100.0, (214, 212, 208), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (80, 130, 200), 0.0),
                stop(1.0, (80, 130, 200), 0.32),
                stop(5.0, (80, 175, 185), 0.42),
                stop(20.0, (210, 180, 90), 0.5),
                stop(50.0, (208, 90, 70), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (215, 160, 90), 0.0),
                stop(5.0, (210, 145, 75), 0.32),
                stop(15.0, (200, 90, 75), 0.5),
            ]),
        },
        // Route: bright flexoki yellow (#D0A215 vivified); conflicts in
        // flexoki red.
        route: RouteTheme {
            line: srgb(250, 195, 75, 1.0),
            line_conflict: srgb(225, 76, 65, 1.0),
            handle_fill: srgb(250, 195, 75, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 195, 75, 0.12),
        },
        labels: LabelTheme {
            // Flexoki's warm paper foreground, slightly softened.
            text: srgb(206, 205, 195, 0.95),
            halo: [0.0; 4],
        },
        // Relief tinted toward the warm ink band.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x17, 0x12, 0x0e),
            light_tint: tint_from_srgb8(0x86, 0x80, 0x74),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

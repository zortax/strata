//! "High Contrast" — the original high-saturation dark chart look,
//! extracted verbatim from the pre-theme style constants
//! (`assets/themes/oldworld.json` derived). The regression test in the
//! parent module pins the values byte-exact.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace base hues (sRGB bytes) — German VFR / ICAO chart conventions.
const BLUE: (u8, u8, u8) = (64, 110, 205); // controlled airspace blue
const FAINT_BLUE: (u8, u8, u8) = (110, 145, 215); // class E band
const RED: (u8, u8, u8) = (214, 48, 58); // CTR / ED-R / ED-P
const GREY: (u8, u8, u8) = (150, 152, 158); // TMZ
const AMBER: (u8, u8, u8) = (228, 168, 50); // glider / para-jump
const NEUTRAL: (u8, u8, u8) = (140, 140, 140);

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
    // Land — a neutral dark grey a few steps above the Oldworld gpui
    // theme's `background` (#161617) so land, app chrome and water all
    // separate.
    let land = srgb8(0x21, 0x21, 0x24);
    MapTheme {
        id: "high-contrast",
        name: "High Contrast",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Darker than land (water recedes) and clearly blue-leaning.
            water: srgb8(0x18, 0x22, 0x30),
            // Clearly lighter than the water fill; major rivers are key VFR
            // ground reference.
            waterway: srgb8(0x3f, 0x56, 0x80),
            forest: srgb8(0x1e, 0x25, 0x1f),
            grass: srgb8(0x20, 0x25, 0x1f),
            farmland: srgb8(0x23, 0x23, 0x1f),
            barren: srgb8(0x26, 0x24, 0x20),
            glacier: srgb8(0x2a, 0x2a, 0x2e),
            park: srgb8(0x20, 0x27, 0x20),
            urban: srgb8(0x2b, 0x2a, 0x2e),
            urban_dense: srgb8(0x30, 0x2e, 0x32),
            military: srgb8(0x2b, 0x25, 0x28),
            aerodrome: srgb8(0x2a, 0x2a, 0x31),
            // Motorways — the one warm accent (muted `base.yellow` of the
            // Oldworld gpui theme).
            road_highway: srgb8(0x6b, 0x5a, 0x45),
            road_major: srgb8(0x4e, 0x4e, 0x56),
            road_medium: srgb8(0x3c, 0x3c, 0x42),
            road_minor: srgb8(0x31, 0x31, 0x36),
            path: srgb8(0x2a, 0x2a, 0x2e),
            rail: srgb8_a(0x38, 0x38, 0x3f, 0.9),
            // Desaturated grey-violet, dashed; clearly visible at every zoom.
            boundary_country: srgb8_a(0x7d, 0x78, 0x86, 0.7),
            boundary_region: srgb8_a(0x6c, 0x68, 0x74, 0.45),
            // Muted grey-violet, quiet next to aero labels.
            place_label: srgb8(0x5f, 0x5b, 0x68),
            country_label: srgb8(0x6f, 0x6b, 0x79),
            water_label: srgb8(0x47, 0x55, 0x71),
        },
        airspace: AirspaceTheme {
            // A/B do not occur in German lower airspace; styled like generic
            // controlled airspace so foreign data still renders sanely.
            class_a: pair(BLUE, 0.06, 0.85),
            class_b: pair(BLUE, 0.06, 0.85),
            class_c: pair(BLUE, 0.08, 0.9),
            class_d: pair(BLUE, 0.06, 0.8),
            // Class E is depicted subtly: faint band, thin border.
            class_e: pair(FAINT_BLUE, 0.025, 0.4),
            class_f: pair(FAINT_BLUE, 0.02, 0.35),
            class_g: pair(NEUTRAL, 0.01, 0.2),
            ctr: pair(RED, 0.1, 0.9),
            rmz: pair(BLUE, 0.04, 0.8),
            tmz: pair(GREY, 0.03, 0.85),
            danger: pair(RED, 0.07, 0.8),
            restricted: pair(RED, 0.14, 0.9),
            prohibited: pair(RED, 0.18, 0.95),
            glider_sector: pair(AMBER, 0.05, 0.85),
            para_jump: pair(AMBER, 0.05, 0.8),
            other: pair(NEUTRAL, 0.02, 0.5),
        },
        symbols: SymbolTheme {
            airport: srgb(206, 201, 190, 1.0),
            glider: srgb(214, 184, 100, 1.0),
            navaid: srgb(152, 163, 180, 1.0),
            reporting: srgb(226, 226, 232, 1.0),
            obstacle: srgb(204, 120, 110, 1.0),
            // White; tinted by the per-instance flight-category color.
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            // Dark rim; the instance tint darkens it toward the category hue.
            weather_outline: srgb(30, 30, 34, 1.0),
        },
        weather: WeatherTheme {
            vfr: srgb(0, 176, 92, 1.0),
            mvfr: srgb(20, 122, 255, 1.0),
            ifr: srgb(229, 48, 57, 1.0),
            lifr: srgb(199, 42, 199, 1.0),
            sigmet: srgb(236, 120, 44, 0.5),
            // Gridded overlays at full chart strength.
            cloud_cover: Colormap::new(&[
                stop(10.0, (160, 165, 175), 0.0),
                stop(40.0, (172, 177, 186), 0.18),
                stop(75.0, (205, 209, 216), 0.38),
                stop(100.0, (238, 240, 244), 0.55),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 130, 220), 0.0),
                stop(1.0, (60, 130, 220), 0.4),
                stop(5.0, (45, 200, 215), 0.5),
                stop(20.0, (235, 210, 60), 0.58),
                stop(50.0, (230, 55, 45), 0.68),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (255, 176, 46), 0.0),
                stop(5.0, (250, 150, 36), 0.42),
                stop(15.0, (226, 48, 40), 0.62),
            ]),
        },
        // Route: saturated chart yellow matching the high-contrast language;
        // conflicts in alarm red.
        route: RouteTheme {
            line: srgb(255, 196, 0, 1.0),
            line_conflict: srgb(255, 40, 60, 1.0),
            handle_fill: srgb(255, 196, 0, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 196, 0, 0.12),
        },
        labels: LabelTheme {
            // Warm light grey, friendly to the Oldworld gpui chrome.
            text: srgb(216, 211, 199, 0.95),
            // No halo — the original dark look has none.
            halo: [0.0; 4],
        },
        // Shadows pull toward deep brown-black, lights toward a warm grey.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x1a, 0x12, 0x0c),
            light_tint: tint_from_srgb8(0x8c, 0x84, 0x78),
            opacity: 0.5,
        },
        clear_color: land,
    }
}

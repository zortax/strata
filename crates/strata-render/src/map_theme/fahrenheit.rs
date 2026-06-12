//! "Fahrenheit" — map theme paired with the Fahrenheit UI theme.
//!
//! Fahrenheit is heat on black: a pure-black window (`background
//! #000000`, title bar `#0e0e0e`) with deep ember accents (primary
//! `#720202`, red `#723202` / `#c97636`, yellow `#726302` / `#c9b536`)
//! and warm cream text (`#FFFFCE`). The basemap is the darkest of the
//! catalog — ember-warm near-black land `#0d0c0a` between the black
//! window and the title bar, with a faint warm road glow. Airspaces
//! carry the fire identity: ember red for CTR/restricted/prohibited,
//! flame orange for danger, muted brass for glider sectors; controlled
//! airspace takes the one cool accent (`base.blue.light #0551cb`,
//! pastelized to steel) so the chart hierarchy still reads cool vs warm.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Fahrenheit accent set (sRGB bytes).
const STEEL: (u8, u8, u8) = (96, 124, 190); // blue.light #0551cb pastelized — controlled
const FAINT_STEEL: (u8, u8, u8) = (112, 134, 180); // class E/F band
const EMBER: (u8, u8, u8) = (200, 90, 70); // primary #720202 family — CTR / ED-R / ED-P
const FLAME: (u8, u8, u8) = (210, 128, 62); // red.light #c97636 — danger areas
const BRASS: (u8, u8, u8) = (196, 176, 100); // yellow.light #c9b536 muted — glider / para
const WARM_GREY: (u8, u8, u8) = (150, 144, 136); // TMZ
const NEUTRAL: (u8, u8, u8) = (138, 134, 130);

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
    // Ember-warm near-black: between the pure-black window background and
    // the #0e0e0e title bar, with a whisper of warmth (r > g > b).
    let land = srgb8(0x0d, 0x0c, 0x0a);
    MapTheme {
        id: "fahrenheit",
        name: "Fahrenheit",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // The coolest thing on the ground: barely darker, blue-leaning.
            water: srgb8(0x09, 0x0a, 0x0d),
            waterway: srgb8(0x16, 0x1a, 0x20),
            // Landcover hugs the ember land within a couple of channels.
            forest: srgb8(0x0b, 0x0a, 0x08),
            grass: srgb8(0x0c, 0x0b, 0x09),
            farmland: srgb8(0x0e, 0x0d, 0x0b),
            barren: srgb8(0x10, 0x0e, 0x0b),
            glacier: srgb8(0x12, 0x12, 0x14),
            park: srgb8(0x0c, 0x0b, 0x09),
            // Cities as a faint heat glow.
            urban: srgb8(0x12, 0x10, 0x0d),
            urban_dense: srgb8(0x15, 0x12, 0x0f),
            military: srgb8(0x11, 0x0f, 0x0c),
            aerodrome: srgb8(0x12, 0x11, 0x13),
            // Tightly compressed warm road ramp over the near-black ground.
            road_highway: srgb8(0x16, 0x14, 0x0f),
            road_major: srgb8(0x13, 0x11, 0x10),
            road_medium: srgb8(0x11, 0x10, 0x0e),
            road_minor: srgb8(0x0f, 0x0e, 0x0c),
            path: srgb8(0x0e, 0x0d, 0x0b),
            rail: srgb8_a(0x13, 0x12, 0x14, 0.85),
            // Warm-gray boundaries stay the one clearly visible ground line.
            boundary_country: srgb8_a(0x59, 0x51, 0x49, 0.55),
            boundary_region: srgb8_a(0x40, 0x3a, 0x33, 0.30),
            place_label: srgb8(0x56, 0x4f, 0x46),
            country_label: srgb8(0x64, 0x5c, 0x50),
            water_label: srgb8(0x3c, 0x42, 0x4d),
        },
        airspace: AirspaceTheme {
            class_a: pair(STEEL, 0.05, 0.72),
            class_b: pair(STEEL, 0.05, 0.72),
            class_c: pair(STEEL, 0.07, 0.78),
            class_d: pair(STEEL, 0.05, 0.7),
            class_e: pair(FAINT_STEEL, 0.02, 0.35),
            class_f: pair(FAINT_STEEL, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(EMBER, 0.09, 0.8),
            rmz: pair(STEEL, 0.035, 0.68),
            tmz: pair(WARM_GREY, 0.03, 0.75),
            danger: pair(FLAME, 0.06, 0.7),
            restricted: pair(EMBER, 0.12, 0.8),
            prohibited: pair(EMBER, 0.16, 0.85),
            glider_sector: pair(BRASS, 0.045, 0.75),
            para_jump: pair(BRASS, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Warm cream (from #FFFFCE) on the black ground.
            airport: srgb(224, 218, 184, 1.0),
            glider: srgb(200, 180, 110, 1.0),
            navaid: srgb(124, 144, 186, 1.0),
            reporting: srgb(230, 226, 202, 1.0),
            obstacle: srgb(205, 110, 85, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(26, 24, 22, 1.0),
        },
        weather: WeatherTheme {
            // Categories from the bright accent row (#39c936 / #0551cb /
            // ember red / #58197a violet), kept semantically intact.
            vfr: srgb(88, 190, 100, 1.0),
            mvfr: srgb(90, 130, 220, 1.0),
            ifr: srgb(215, 90, 75, 1.0),
            lifr: srgb(170, 100, 200, 1.0),
            sigmet: srgb(220, 130, 60, 0.45),
            // Gridded overlays: muted warm-gray clouds, fire-leaning ramps.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 146, 142), 0.0),
                stop(40.0, (156, 154, 150), 0.12),
                stop(75.0, (184, 181, 176), 0.28),
                stop(100.0, (210, 206, 200), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (85, 125, 200), 0.0),
                stop(1.0, (85, 125, 200), 0.32),
                stop(5.0, (80, 180, 185), 0.42),
                stop(20.0, (210, 180, 80), 0.5),
                stop(50.0, (205, 85, 60), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (222, 150, 70), 0.0),
                stop(5.0, (215, 135, 55), 0.34),
                stop(15.0, (205, 80, 55), 0.52),
            ]),
        },
        // Route: ice cyan — the one cool pop against the all-heat palette;
        // conflicts in flame red.
        route: RouteTheme {
            line: srgb(90, 200, 230, 1.0),
            line_conflict: srgb(235, 64, 52, 1.0),
            handle_fill: srgb(90, 200, 230, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(90, 200, 230, 0.12),
        },
        labels: LabelTheme {
            // The theme's cream foreground, slightly dimmed for map labels.
            text: srgb(228, 222, 188, 0.95),
            halo: [0.0; 4],
        },
        // Ember relief: shadows fall toward burnt brown.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x0d, 0x08),
            light_tint: tint_from_srgb8(0x80, 0x76, 0x6a),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

//! "Oldworld" — the default dark map theme: desaturated pastel hues and
//! slightly less contrast than "High Contrast". No hard red/blue: CTR and
//! the danger family lean dusty rose / terracotta, controlled airspace
//! leans slate/steel blue, glider sectors muted sand. The basemap is a
//! deliberately neutral dark grayscale — no greens, browns or saturated
//! hues; ground structure (roads, water, landuse) stays low-contrast so
//! the dominant contrast is between the basemap as a whole and the pastel
//! airspace overlays. Any edit here is a visual change to the default
//! map; the regression test in the parent module pins the values.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Pastel airspace hues (sRGB bytes).
const STEEL: (u8, u8, u8) = (122, 144, 178); // slate/steel blue — controlled
const FAINT_STEEL: (u8, u8, u8) = (138, 154, 180); // class E/F band
const ROSE: (u8, u8, u8) = (198, 116, 120); // dusty rose — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (200, 124, 98); // danger areas
const MAUVE_GREY: (u8, u8, u8) = (152, 150, 158); // TMZ
const SAND: (u8, u8, u8) = (205, 178, 124); // glider / para-jump
const NEUTRAL: (u8, u8, u8) = (142, 142, 144);

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
    // Near-black neutral ground; the whole basemap stays within a very
    // narrow grayscale band so the pastel airspaces carry the contrast.
    let land = srgb8(0x14, 0x14, 0x15);
    MapTheme {
        id: "oldworld",
        name: "Oldworld",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than land — just enough tint to read
            // as water, still receding behind land.
            water: srgb8(0x10, 0x11, 0x14),
            waterway: srgb8(0x1e, 0x21, 0x26),
            // Landcover/landuse: near-neutral greys a whisker around `land`
            // (no green/brown hues).
            forest: srgb8(0x12, 0x12, 0x13),
            grass: srgb8(0x13, 0x13, 0x14),
            farmland: srgb8(0x15, 0x15, 0x16),
            barren: srgb8(0x16, 0x16, 0x17),
            glacier: srgb8(0x18, 0x18, 0x1a),
            park: srgb8(0x13, 0x13, 0x14),
            urban: srgb8(0x18, 0x18, 0x19),
            urban_dense: srgb8(0x1b, 0x1b, 0x1c),
            military: srgb8(0x17, 0x17, 0x18),
            aerodrome: srgb8(0x19, 0x19, 0x1b),
            // Roads: a deliberately compressed ramp barely above ground —
            // faint texture, never lines that compete with the airspaces.
            // Minor roads/paths may vanish at low zoom; that is intended.
            road_highway: srgb8(0x22, 0x22, 0x23),
            road_major: srgb8(0x1e, 0x1e, 0x1f),
            road_medium: srgb8(0x1b, 0x1b, 0x1c),
            road_minor: srgb8(0x18, 0x18, 0x19),
            path: srgb8(0x16, 0x16, 0x17),
            rail: srgb8_a(0x1c, 0x1c, 0x1f, 0.85),
            // Boundaries: country borders keep their contrast (the one
            // basemap feature that must stay clearly visible); region
            // borders scale down with the darker ground.
            boundary_country: srgb8_a(0x5f, 0x5f, 0x64, 0.55),
            boundary_region: srgb8_a(0x45, 0x45, 0x4a, 0.30),
            place_label: srgb8(0x56, 0x56, 0x5a),
            country_label: srgb8(0x63, 0x63, 0x67),
            water_label: srgb8(0x42, 0x47, 0x50),
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
            airport: srgb(200, 196, 188, 1.0),
            glider: srgb(205, 182, 128, 1.0),
            navaid: srgb(152, 160, 175, 1.0),
            reporting: srgb(216, 216, 222, 1.0),
            obstacle: srgb(198, 130, 122, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(32, 32, 36, 1.0),
        },
        weather: WeatherTheme {
            // Pastelized but semantically intact: green / blue / red / magenta.
            vfr: srgb(96, 188, 138, 1.0),
            mvfr: srgb(112, 150, 218, 1.0),
            ifr: srgb(214, 108, 112, 1.0),
            lifr: srgb(198, 122, 198, 1.0),
            sigmet: srgb(218, 142, 92, 0.45),
            // Gridded overlays, muted to match the pastel look.
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 152, 156), 0.0),
                stop(40.0, (158, 160, 164), 0.12),
                stop(75.0, (186, 188, 192), 0.28),
                stop(100.0, (212, 214, 218), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (96, 138, 198), 0.0),
                stop(1.0, (96, 138, 198), 0.32),
                stop(5.0, (96, 188, 198), 0.42),
                stop(20.0, (208, 188, 96), 0.5),
                stop(50.0, (204, 92, 80), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (216, 162, 92), 0.0),
                stop(5.0, (212, 148, 80), 0.32),
                stop(15.0, (198, 94, 84), 0.5),
            ]),
        },
        // Route: warm high-vis amber — free of the pastel airspace hues;
        // conflicts in a vivid terracotta-red.
        route: RouteTheme {
            line: srgb(255, 178, 72, 1.0),
            line_conflict: srgb(236, 86, 80, 1.0),
            handle_fill: srgb(255, 178, 72, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 178, 72, 0.12),
        },
        labels: LabelTheme {
            // A touch softer than high-contrast's warm light grey.
            text: srgb(208, 204, 196, 0.95),
            halo: [0.0; 4],
        },
        // Slightly flatter relief than high-contrast (less contrast overall).
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x19, 0x14, 0x12),
            light_tint: tint_from_srgb8(0x84, 0x7e, 0x76),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

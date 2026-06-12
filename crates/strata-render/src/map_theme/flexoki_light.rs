//! "Flexoki Light" — map theme paired with the Flexoki Light UI theme.
//!
//! Flexoki Light is warm paper (`background #FFFCF0`, `panel #F2F0E5`,
//! near-black ink foreground `#100F0F`). The map ground is the same paper
//! a notch below the window background, with all features drawn as
//! slightly darker ink. Airspace identity comes from the Flexoki Light
//! accents, darkened to mid-tones so they read on paper: blue `#4385BE`
//! for controlled airspace, red `#D14D41` for CTR/restricted/prohibited,
//! orange `#BC5215` for danger, yellow `#D0A215` as ochre for glider
//! sectors (and the muted motorway hue), and the teal primary `#3AA99F`
//! tinting the class E/F band — the mirror image of Flexoki Dark.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes), Flexoki Light accents darkened for paper.
const BLUE: (u8, u8, u8) = (52, 106, 160); // base.blue #4385BE — controlled
const FAINT_TEAL: (u8, u8, u8) = (70, 132, 124); // primary cyan #3AA99F — class E/F band
const ROSE: (u8, u8, u8) = (178, 70, 60); // base.red #D14D41 — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (180, 92, 40); // syntax orange #BC5215 — danger areas
const OCHRE: (u8, u8, u8) = (166, 128, 38); // base.yellow #D0A215 — glider / para
const TMZ_GREY: (u8, u8, u8) = (112, 110, 100); // warm ink grey — TMZ
const NEUTRAL: (u8, u8, u8) = (115, 113, 106); // muted_fg #6F6E69 family

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
    // Flexoki paper, a notch below the window background #FFFCF0 / panel
    // #F2F0E5 so chrome and map read as one warm sheet.
    let land = srgb8(0xf2, 0xee, 0xdf);
    MapTheme {
        id: "flexoki-light",
        name: "Flexoki Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker, desaturated cool blue so water reads at once.
            water: srgb8(0xb7, 0xc5, 0xcc),
            waterway: srgb8(0x84, 0x99, 0xa8),
            // Landcover: soft warm tints a whisker below paper.
            forest: srgb8(0xd9, 0xdc, 0xc4),
            grass: srgb8(0xe2, 0xe3, 0xcb),
            farmland: srgb8(0xec, 0xe5, 0xc9),
            barren: srgb8(0xe7, 0xdd, 0xc2),
            glacier: srgb8(0xf1, 0xf2, 0xf0),
            park: srgb8(0xde, 0xe0, 0xc7),
            urban: srgb8(0xe2, 0xdc, 0xd2),
            urban_dense: srgb8(0xd8, 0xd1, 0xc7),
            military: srgb8(0xe2, 0xd6, 0xcc),
            aerodrome: srgb8(0xde, 0xdc, 0xe0),
            // Roads as slightly darker ink; the motorway keeps a muted
            // Flexoki-yellow ochre (highway/land luma ≈ 0.62).
            road_highway: srgb8(0xb3, 0x92, 0x4e),
            road_major: srgb8(0x93, 0x90, 0x8a),
            road_medium: srgb8(0xa8, 0xa5, 0x9c),
            road_minor: srgb8(0xbb, 0xb8, 0xae),
            path: srgb8(0xcc, 0xc9, 0xbe),
            rail: srgb8_a(0x9d, 0x9a, 0x92, 0.9),
            // Warm ink-grey boundaries, clearly legible on paper.
            boundary_country: srgb8_a(0x6e, 0x6a, 0x60, 0.75),
            boundary_region: srgb8_a(0x88, 0x83, 0x78, 0.5),
            place_label: srgb8(0x5c, 0x5a, 0x52),
            country_label: srgb8(0x4a, 0x48, 0x40),
            water_label: srgb8(0x4f, 0x64, 0x70),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.07, 0.75),
            class_b: pair(BLUE, 0.07, 0.75),
            class_c: pair(BLUE, 0.1, 0.85),
            class_d: pair(BLUE, 0.07, 0.75),
            class_e: pair(FAINT_TEAL, 0.035, 0.45),
            class_f: pair(FAINT_TEAL, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(ROSE, 0.11, 0.85),
            rmz: pair(BLUE, 0.05, 0.7),
            tmz: pair(TMZ_GREY, 0.04, 0.75),
            danger: pair(EMBER, 0.08, 0.75),
            restricted: pair(ROSE, 0.15, 0.85),
            prohibited: pair(ROSE, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Ink symbols on paper (foreground #100F0F family).
            airport: srgb(40, 38, 36, 1.0),
            glider: srgb(140, 105, 30, 1.0),
            navaid: srgb(60, 90, 130, 1.0),
            reporting: srgb(50, 48, 44, 1.0),
            obstacle: srgb(170, 62, 52, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(42, 40, 38, 1.0),
        },
        weather: WeatherTheme {
            // Flexoki Light's stronger highlight accents, kept semantic.
            vfr: srgb(90, 130, 20, 1.0),    // green #66800B
            mvfr: srgb(38, 96, 190, 1.0),   // blue #205EA6
            ifr: srgb(190, 48, 40, 1.0),    // red #AF3029
            lifr: srgb(165, 40, 130, 1.0),  // magenta #A02F6F
            sigmet: srgb(188, 88, 28, 0.5), // orange #BC5215
            // Gridded overlays: muted darker greys so they read on paper.
            cloud_cover: Colormap::new(&[
                stop(10.0, (132, 131, 127), 0.0),
                stop(40.0, (138, 137, 133), 0.12),
                stop(75.0, (152, 151, 147), 0.26),
                stop(100.0, (166, 165, 161), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 112, 190), 0.0),
                stop(1.0, (60, 112, 190), 0.34),
                stop(5.0, (45, 150, 170), 0.44),
                stop(20.0, (180, 150, 45), 0.52),
                stop(50.0, (180, 60, 45), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (190, 128, 40), 0.0),
                stop(5.0, (185, 115, 35), 0.36),
                stop(15.0, (170, 55, 42), 0.54),
            ]),
        },
        // Route: flexoki magenta #A02F6F — vivid ink on paper; conflicts in
        // flexoki red.
        route: RouteTheme {
            line: srgb(160, 47, 111, 1.0),
            line_conflict: srgb(204, 50, 38, 1.0),
            handle_fill: srgb(160, 47, 111, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(160, 47, 111, 0.12),
        },
        labels: LabelTheme {
            // Near-black ink over a paper halo.
            text: srgb(48, 46, 42, 0.95),
            halo: srgb(253, 250, 240, 0.85),
        },
        // Relief: warm ink shadows, paper lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x5c, 0x52, 0x44),
            light_tint: tint_from_srgb8(0xf6, 0xf1, 0xe2),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

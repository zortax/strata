//! "macOS Classic Light" — map theme paired with the macOS Classic Light UI
//! theme. The UI chrome is neutral near-white (`#F9F9F9` window, `#FEFEFE`
//! title bar, `#EAEAEA` panels), so the ground is a cool-neutral paper grey
//! a notch below the window background, with restrained landcover tints.
//! The motorway alone keeps a hue — the System Yellow (`#B59A00`) muted to
//! a warm khaki — while the rest of the road ramp stays neutral grey, true
//! to macOS's reserved chrome. Airspaces carry the saturated light-mode
//! accents, darkened to mid-tones: blue `#0060de` for controlled airspace,
//! red `#d21f07` (rose-shifted) for CTR/restricted/prohibited and a warmer
//! terracotta for danger, yellow `#B59A00` as ochre for glider/para, the
//! magenta `#9A0068` reserved for the LIFR weather category.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues, darkened to mid-tones so they read on the light ground.
const BLUE: (u8, u8, u8) = (56, 100, 182); // #0060de — controlled
const FAINT_BLUE: (u8, u8, u8) = (96, 126, 172); // class E/F band
const RED: (u8, u8, u8) = (178, 66, 70); // #d21f07 rose-shifted — CTR / ED-R / ED-P
const TERRACOTTA: (u8, u8, u8) = (184, 102, 66); // danger areas (warmer)
const GREY: (u8, u8, u8) = (104, 106, 116); // TMZ
const OCHRE: (u8, u8, u8) = (152, 126, 52); // #B59A00 muted — glider / para
const NEUTRAL: (u8, u8, u8) = (108, 108, 114);

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
    // Cool-neutral paper, just below the #F9F9F9 window background.
    let land = srgb8(0xeb, 0xeb, 0xec);
    MapTheme {
        id: "macos-classic-light",
        name: "macOS Classic Light",
        mode: MapThemeMode::Light,
        basemap: BasemapTheme {
            land,
            // Visibly darker neutral-cool blue-grey so water reads at once.
            water: srgb8(0xb6, 0xc3, 0xd2),
            waterway: srgb8(0x7e, 0x96, 0xb2),
            // Landcover: restrained tints a notch around the paper grey.
            forest: srgb8(0xd9, 0xde, 0xd2),
            grass: srgb8(0xe0, 0xe4, 0xd6),
            farmland: srgb8(0xe8, 0xe5, 0xd8),
            barren: srgb8(0xe3, 0xdd, 0xd1),
            glacier: srgb8(0xef, 0xf2, 0xf5),
            park: srgb8(0xdc, 0xe0, 0xd2),
            urban: srgb8(0xdf, 0xdc, 0xdc),
            urban_dense: srgb8(0xd4, 0xd1, 0xd1),
            military: srgb8(0xde, 0xd3, 0xd0),
            aerodrome: srgb8(0xda, 0xda, 0xe2),
            // Roads slightly darker than paper; only the motorway keeps a
            // hue (muted System Yellow), the rest stay neutral grey.
            road_highway: srgb8(0xab, 0x9a, 0x62),
            road_major: srgb8(0x9c, 0x9c, 0xa2),
            road_medium: srgb8(0xae, 0xae, 0xb4),
            road_minor: srgb8(0xbe, 0xbe, 0xc4),
            path: srgb8(0xcd, 0xcd, 0xd2),
            rail: srgb8_a(0xa0, 0xa0, 0xa8, 0.9),
            // Dark neutral greys, clearly legible on paper.
            boundary_country: srgb8_a(0x64, 0x64, 0x6c, 0.75),
            boundary_region: srgb8_a(0x80, 0x80, 0x8a, 0.5),
            place_label: srgb8(0x59, 0x59, 0x5e),
            country_label: srgb8(0x48, 0x48, 0x4e),
            water_label: srgb8(0x4d, 0x60, 0x78),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.07, 0.75),
            class_b: pair(BLUE, 0.07, 0.75),
            class_c: pair(BLUE, 0.1, 0.85),
            class_d: pair(BLUE, 0.07, 0.75),
            class_e: pair(FAINT_BLUE, 0.035, 0.45),
            class_f: pair(FAINT_BLUE, 0.03, 0.4),
            class_g: pair(NEUTRAL, 0.015, 0.25),
            ctr: pair(RED, 0.11, 0.85),
            rmz: pair(BLUE, 0.05, 0.7),
            tmz: pair(GREY, 0.04, 0.75),
            danger: pair(TERRACOTTA, 0.08, 0.75),
            restricted: pair(RED, 0.15, 0.85),
            prohibited: pair(RED, 0.19, 0.9),
            glider_sector: pair(OCHRE, 0.06, 0.8),
            para_jump: pair(OCHRE, 0.06, 0.75),
            other: pair(NEUTRAL, 0.025, 0.5),
        },
        symbols: SymbolTheme {
            // Dark symbols on light ground; navaid leans the system blue.
            airport: srgb(62, 62, 68, 1.0),
            glider: srgb(134, 106, 40, 1.0),
            navaid: srgb(66, 92, 132, 1.0),
            reporting: srgb(54, 54, 60, 1.0),
            obstacle: srgb(172, 60, 50, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(40, 40, 46, 1.0),
        },
        weather: WeatherTheme {
            // The light-mode accent set nearly verbatim — already saturated
            // mid-tones built for white chrome.
            vfr: srgb(44, 146, 24, 1.0),
            mvfr: srgb(20, 96, 210, 1.0),
            ifr: srgb(200, 46, 34, 1.0),
            lifr: srgb(158, 40, 128, 1.0),
            sigmet: srgb(200, 98, 28, 0.5),
            cloud_cover: Colormap::new(&[
                stop(10.0, (128, 130, 138), 0.0),
                stop(40.0, (134, 136, 144), 0.12),
                stop(75.0, (148, 150, 158), 0.26),
                stop(100.0, (162, 164, 172), 0.4),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (60, 110, 192), 0.0),
                stop(1.0, (60, 110, 192), 0.34),
                stop(5.0, (40, 156, 180), 0.44),
                stop(20.0, (190, 158, 46), 0.52),
                stop(50.0, (192, 56, 44), 0.6),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (196, 130, 40), 0.0),
                stop(5.0, (192, 118, 32), 0.36),
                stop(15.0, (178, 52, 42), 0.54),
            ]),
        },
        // Route: System Purple — vivid on the light ground; conflicts in
        // System Red.
        route: RouteTheme {
            line: srgb(125, 52, 178, 1.0),
            line_conflict: srgb(210, 46, 50, 1.0),
            handle_fill: srgb(125, 52, 178, 1.0),
            handle_outline: srgb(252, 250, 246, 1.0),
            corridor: srgb(125, 52, 178, 0.12),
        },
        labels: LabelTheme {
            // Near-black text over a near-white halo — the macOS contrast.
            text: srgb(44, 44, 50, 0.95),
            halo: srgb(250, 250, 252, 0.85),
        },
        // Neutral grey relief to match the neutral paper.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x56, 0x56, 0x5c),
            light_tint: tint_from_srgb8(0xf1, 0xf1, 0xf4),
            opacity: 0.35,
        },
        clear_color: land,
    }
}

//! "Default Dark" — map theme paired with the Default Dark UI theme (the
//! gpui-component registry built-in: shadcn's Tailwind *neutral* palette).
//!
//! The UI chrome is a dead-neutral grayscale (`background` neutral-950
//! `#0a0a0a`, `title_bar` `#171717`, `sidebar.accent` neutral-800
//! `#262626`), so the basemap is a pure-gray band with land `#111111`
//! sitting between the window background and the title bar — even more
//! neutral than Oldworld's faintly blue-leaning grays. Identity comes from
//! the Tailwind 400-level accents: controlled airspace = blue-400
//! (`#60a5fa`, pastelized), CTR/restricted/prohibited = red-400
//! (`#f87171`), danger leans orange-400, glider/para = yellow-400 toned to
//! sand, TMZ a purple-400 gray. Flight categories are the Tailwind 400s
//! nearly verbatim (purple-400 nudged toward magenta so the LIFR/MVFR
//! linear-space hues stay ≥ 30° apart). Terrain stays fully neutral.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues (sRGB bytes) — Tailwind 400-level accents, pastelized.
const BLUE: (u8, u8, u8) = (102, 158, 232); // blue-400 — controlled
const FAINT_BLUE: (u8, u8, u8) = (124, 160, 212); // class E/F band
const RED: (u8, u8, u8) = (232, 112, 114); // red-400 — CTR / ED-R / ED-P
const ORANGE: (u8, u8, u8) = (230, 140, 92); // orange-400 lean — danger areas
const MAUVE: (u8, u8, u8) = (170, 156, 186); // purple-400 grayed — TMZ
const SAND: (u8, u8, u8) = (222, 188, 102); // yellow-400 muted — glider / para
const NEUTRAL: (u8, u8, u8) = (140, 140, 146);

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
    // Pure-neutral near-black ground between the UI's neutral-950 window
    // background and its #171717 title bar; the whole basemap stays a
    // grayscale band so the Tailwind accents carry all the contrast.
    let land = srgb8(0x11, 0x11, 0x11);
    MapTheme {
        id: "default-dark",
        name: "Default Dark",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely cooler and darker than land — just enough to read as
            // water in an otherwise dead-neutral band.
            water: srgb8(0x0d, 0x0e, 0x10),
            waterway: srgb8(0x1a, 0x1d, 0x22),
            // Landcover: pure grays a whisker around `land`.
            forest: srgb8(0x0f, 0x0f, 0x0f),
            grass: srgb8(0x10, 0x10, 0x10),
            farmland: srgb8(0x12, 0x12, 0x12),
            barren: srgb8(0x13, 0x13, 0x13),
            glacier: srgb8(0x16, 0x16, 0x18),
            park: srgb8(0x10, 0x10, 0x10),
            urban: srgb8(0x14, 0x14, 0x14),
            urban_dense: srgb8(0x17, 0x17, 0x17),
            military: srgb8(0x13, 0x13, 0x14),
            aerodrome: srgb8(0x15, 0x15, 0x17),
            // Roads: compressed ramp scaled to the darker ground (motorway
            // luma ≈ 1.7× land) — faint texture, never competing lines.
            road_highway: srgb8(0x1d, 0x1d, 0x1d),
            road_major: srgb8(0x19, 0x19, 0x19),
            road_medium: srgb8(0x17, 0x17, 0x17),
            road_minor: srgb8(0x15, 0x15, 0x15),
            path: srgb8(0x13, 0x13, 0x13),
            rail: srgb8_a(0x18, 0x18, 0x1a, 0.85),
            // Country borders stay the one clearly visible basemap feature.
            boundary_country: srgb8_a(0x5c, 0x5c, 0x5e, 0.55),
            boundary_region: srgb8_a(0x40, 0x40, 0x43, 0.30),
            place_label: srgb8(0x52, 0x52, 0x52),
            country_label: srgb8(0x5e, 0x5e, 0x5e),
            water_label: srgb8(0x3e, 0x44, 0x4d),
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
            rmz: pair(BLUE, 0.035, 0.68),
            tmz: pair(MAUVE, 0.03, 0.75),
            danger: pair(ORANGE, 0.06, 0.7),
            restricted: pair(RED, 0.12, 0.8),
            prohibited: pair(RED, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            // Neutral light grays (the UI's neutral-200/300 range), with the
            // accent families on glider/navaid/obstacle.
            airport: srgb(206, 206, 208, 1.0),
            glider: srgb(228, 196, 110, 1.0),
            navaid: srgb(148, 164, 186, 1.0),
            reporting: srgb(220, 220, 224, 1.0),
            obstacle: srgb(226, 120, 120, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(28, 28, 30, 1.0),
        },
        weather: WeatherTheme {
            // Tailwind 400s: green-400 / blue-400 / red-400; purple-400
            // nudged toward magenta for LIFR/MVFR hue separation.
            vfr: srgb(74, 222, 128, 1.0),
            mvfr: srgb(96, 165, 250, 1.0),
            ifr: srgb(248, 113, 113, 1.0),
            lifr: srgb(200, 124, 240, 1.0),
            sigmet: srgb(236, 150, 92, 0.45),
            // Gridded overlays: neutral cloud grays, muted radar ramp with
            // the Tailwind blue/red leaning stops.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 150, 154), 0.0),
                stop(40.0, (160, 162, 166), 0.12),
                stop(75.0, (188, 190, 194), 0.28),
                stop(100.0, (214, 216, 220), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (96, 148, 235), 0.0),
                stop(1.0, (96, 148, 235), 0.32),
                stop(5.0, (80, 195, 210), 0.42),
                stop(20.0, (222, 195, 85), 0.5),
                stop(50.0, (225, 95, 85), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (235, 165, 90), 0.0),
                stop(5.0, (228, 150, 75), 0.32),
                stop(15.0, (215, 95, 80), 0.5),
            ]),
        },
        // Route: high-vis amber (yellow-300 family); conflicts in red-400.
        route: RouteTheme {
            line: srgb(255, 184, 60, 1.0),
            line_conflict: srgb(238, 82, 84, 1.0),
            handle_fill: srgb(255, 184, 60, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 184, 60, 0.12),
        },
        labels: LabelTheme {
            // Cool-neutral light gray (toward the UI's neutral-200), no halo.
            text: srgb(212, 212, 216, 0.95),
            halo: [0.0; 4],
        },
        // Fully neutral relief for a fully neutral theme.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x14, 0x14, 0x14),
            light_tint: tint_from_srgb8(0x82, 0x82, 0x82),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

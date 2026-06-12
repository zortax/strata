//! "Tokyo Night" — map theme paired with the Tokyo Night UI theme.
//!
//! Rationale: the UI's near-black indigo window (`background #1a1b26`,
//! `title_bar #161720`, `panel #292e42`) becomes a deep blue-violet ground
//! band (land `#15161f`, b ≈ r + 10) with the compressed Oldworld road
//! ramp re-tinted into the band. Airspace identity comes from the Tokyo
//! Night accents: controlled airspace in the signature blue `#7aa2f7`
//! (softened), CTR / restricted / prohibited in the red `#f7768e`, danger
//! a warmed blend toward the yellow, glider/para in the sand of `#e0af68`,
//! RMZ in the cyan `#7dcfff` and TMZ in the violet-grey of the muted
//! foreground `#565f89`. Weather stays semantic but leans on the same
//! accent set; terrain shadows sink into the indigo band.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Tokyo Night accent set (sRGB bytes, softened).
const BLUE: (u8, u8, u8) = (122, 156, 228); // #7aa2f7 — controlled
const FAINT_BLUE: (u8, u8, u8) = (134, 160, 212); // class E/F band
const CYAN: (u8, u8, u8) = (112, 176, 216); // #7dcfff — RMZ
const ROSE: (u8, u8, u8) = (226, 118, 138); // #f7768e — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (224, 140, 100); // danger (red warmed to yellow)
const SAND: (u8, u8, u8) = (212, 172, 110); // #e0af68 — glider / para-jump
const VIOLET_GREY: (u8, u8, u8) = (148, 150, 170); // #565f89 lifted — TMZ
const NEUTRAL: (u8, u8, u8) = (140, 142, 152);

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
    // Deep indigo ground a few channels under the UI background #1a1b26,
    // keeping its cool b > r balance; the whole basemap stays inside a
    // narrow blue-violet band so the accent overlays carry the contrast.
    let land = srgb8(0x15, 0x16, 0x1f);
    MapTheme {
        id: "tokyo-night",
        name: "Tokyo Night",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and bluer than land — water recedes, still cool.
            water: srgb8(0x11, 0x13, 0x1e),
            waterway: srgb8(0x1e, 0x26, 0x3a),
            // Landcover: ±1..3 channels around land, indigo throughout.
            forest: srgb8(0x13, 0x14, 0x1d),
            grass: srgb8(0x14, 0x15, 0x1e),
            farmland: srgb8(0x16, 0x17, 0x20),
            barren: srgb8(0x17, 0x18, 0x21),
            glacier: srgb8(0x19, 0x1b, 0x25),
            park: srgb8(0x14, 0x15, 0x1e),
            urban: srgb8(0x19, 0x1a, 0x24),
            urban_dense: srgb8(0x1c, 0x1d, 0x28),
            military: srgb8(0x18, 0x19, 0x22),
            aerodrome: srgb8(0x1a, 0x1b, 0x27),
            // Compressed road ramp: +13 motorway down to +2 path over land
            // (luma ratio ≈ 1.58) — faint texture, never competing lines.
            road_highway: srgb8(0x22, 0x23, 0x2c),
            road_major: srgb8(0x1f, 0x20, 0x29),
            road_medium: srgb8(0x1c, 0x1d, 0x26),
            road_minor: srgb8(0x19, 0x1a, 0x23),
            path: srgb8(0x17, 0x18, 0x21),
            rail: srgb8_a(0x1d, 0x1e, 0x28, 0.85),
            // Boundaries from the muted foreground #565f89 family.
            boundary_country: srgb8_a(0x5c, 0x62, 0x82, 0.55),
            boundary_region: srgb8_a(0x44, 0x49, 0x64, 0.30),
            place_label: srgb8(0x54, 0x5a, 0x78),
            country_label: srgb8(0x60, 0x66, 0x86),
            water_label: srgb8(0x3e, 0x4c, 0x6c),
        },
        airspace: AirspaceTheme {
            class_a: pair(BLUE, 0.05, 0.72),
            class_b: pair(BLUE, 0.05, 0.72),
            class_c: pair(BLUE, 0.07, 0.78),
            class_d: pair(BLUE, 0.05, 0.7),
            class_e: pair(FAINT_BLUE, 0.02, 0.35),
            class_f: pair(FAINT_BLUE, 0.018, 0.3),
            class_g: pair(NEUTRAL, 0.01, 0.18),
            ctr: pair(ROSE, 0.09, 0.8),
            rmz: pair(CYAN, 0.035, 0.68),
            tmz: pair(VIOLET_GREY, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(196, 200, 216, 1.0),
            glider: srgb(212, 180, 124, 1.0),
            navaid: srgb(148, 162, 196, 1.0),
            reporting: srgb(214, 218, 232, 1.0),
            obstacle: srgb(224, 128, 134, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(26, 28, 40, 1.0),
        },
        weather: WeatherTheme {
            // Tokyo Night accents, kept semantic: green / blue / red / magenta.
            vfr: srgb(130, 194, 116, 1.0),
            mvfr: srgb(122, 160, 240, 1.0),
            ifr: srgb(240, 122, 142, 1.0),
            lifr: srgb(194, 128, 210, 1.0),
            sigmet: srgb(224, 158, 104, 0.45),
            // Gridded overlays, muted with a faint cool cast.
            cloud_cover: Colormap::new(&[
                stop(10.0, (148, 152, 162), 0.0),
                stop(40.0, (156, 160, 170), 0.12),
                stop(75.0, (182, 186, 196), 0.28),
                stop(100.0, (208, 211, 219), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (108, 140, 222), 0.0),
                stop(1.0, (108, 140, 222), 0.32),
                stop(5.0, (100, 178, 200), 0.42),
                stop(20.0, (208, 184, 100), 0.5),
                stop(50.0, (212, 100, 96), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (222, 164, 104), 0.0),
                stop(5.0, (216, 148, 92), 0.32),
                stop(15.0, (210, 100, 96), 0.5),
            ]),
        },
        // Route: bright tokyo yellow (#e0af68 vivified); conflicts in #f7768e
        // red.
        route: RouteTheme {
            line: srgb(252, 196, 78, 1.0),
            line_conflict: srgb(238, 84, 94, 1.0),
            handle_fill: srgb(252, 196, 78, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(252, 196, 78, 0.12),
        },
        labels: LabelTheme {
            // Softened from the UI foreground #c0caf5; no halo (dark mode).
            text: srgb(196, 200, 228, 0.95),
            halo: [0.0; 4],
        },
        // Relief sinks into the indigo band; cool grey-violet lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x12, 0x13, 0x1e),
            light_tint: tint_from_srgb8(0x7e, 0x82, 0x94),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

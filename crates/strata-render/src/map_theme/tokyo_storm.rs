//! "Tokyo Storm" — map theme paired with the Tokyo Storm UI theme.
//!
//! Rationale: Storm is the lighter, stormier sibling of Tokyo Night — the
//! UI window (`background #24283b`, `title_bar #202435`, `panel #292e42`)
//! yields a slate-indigo ground band (land `#1c1f2e`) a clear notch above
//! Night's, with the same compressed road ramp re-tinted into it. The
//! accent set matches Night (blue `#7aa2f7`, red `#f7768e`, yellow
//! `#e0af68`, cyan `#7dcfff`) so controlled airspace, CTR/restricted,
//! danger, glider and RMZ keep the family identity — but Storm owns a real
//! magenta accent (`#b283f8`), which colors the TMZ and the LIFR weather
//! category and sets it apart from its siblings.

use super::{
    AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme, MapTheme,
    MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a,
};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Tokyo Storm accent set (sRGB bytes, softened).
const BLUE: (u8, u8, u8) = (124, 156, 224); // #7aa2f7 — controlled
const FAINT_BLUE: (u8, u8, u8) = (136, 160, 208); // class E/F band
const CYAN: (u8, u8, u8) = (114, 176, 214); // #7dcfff — RMZ
const ROSE: (u8, u8, u8) = (224, 120, 140); // #f7768e — CTR / ED-R / ED-P
const EMBER: (u8, u8, u8) = (222, 142, 102); // danger (red warmed to yellow)
const SAND: (u8, u8, u8) = (210, 172, 112); // #e0af68 — glider / para-jump
const VIOLET: (u8, u8, u8) = (168, 140, 200); // #b283f8 muted — TMZ
const NEUTRAL: (u8, u8, u8) = (142, 144, 154);

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
    // Slate-indigo ground a few channels under the UI background #24283b,
    // keeping its cool b > r balance — lighter than Tokyo Night's ground,
    // matching Storm's lifted window tone.
    let land = srgb8(0x1c, 0x1f, 0x2e);
    MapTheme {
        id: "tokyo-storm",
        name: "Tokyo Storm",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and bluer than land — water recedes, still cool.
            water: srgb8(0x18, 0x1c, 0x2c),
            waterway: srgb8(0x28, 0x32, 0x48),
            // Landcover: ±1..3 channels around land, slate throughout.
            forest: srgb8(0x1a, 0x1d, 0x2c),
            grass: srgb8(0x1b, 0x1e, 0x2d),
            farmland: srgb8(0x1e, 0x20, 0x2f),
            barren: srgb8(0x1f, 0x21, 0x2f),
            glacier: srgb8(0x21, 0x24, 0x35),
            park: srgb8(0x1b, 0x1e, 0x2d),
            urban: srgb8(0x21, 0x24, 0x34),
            urban_dense: srgb8(0x24, 0x27, 0x38),
            military: srgb8(0x20, 0x22, 0x32),
            aerodrome: srgb8(0x22, 0x24, 0x37),
            // Compressed road ramp: +16 motorway down to +2 path over land
            // (luma ratio ≈ 1.51) — faint texture, never competing lines.
            road_highway: srgb8(0x2c, 0x2f, 0x3f),
            road_major: srgb8(0x26, 0x29, 0x38),
            road_medium: srgb8(0x23, 0x26, 0x35),
            road_minor: srgb8(0x20, 0x23, 0x32),
            path: srgb8(0x1e, 0x21, 0x30),
            rail: srgb8_a(0x25, 0x28, 0x37, 0.85),
            // Boundaries from the muted foreground #565f89 family.
            boundary_country: srgb8_a(0x5f, 0x65, 0x86, 0.55),
            boundary_region: srgb8_a(0x48, 0x4d, 0x68, 0.30),
            place_label: srgb8(0x58, 0x5e, 0x7c),
            country_label: srgb8(0x64, 0x6a, 0x8a),
            water_label: srgb8(0x42, 0x50, 0x70),
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
            tmz: pair(VIOLET, 0.03, 0.75),
            danger: pair(EMBER, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(196, 200, 214, 1.0),
            glider: srgb(210, 178, 124, 1.0),
            navaid: srgb(150, 164, 198, 1.0),
            reporting: srgb(212, 216, 230, 1.0),
            obstacle: srgb(222, 128, 136, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(34, 38, 54, 1.0),
        },
        weather: WeatherTheme {
            // Tokyo accents kept semantic; LIFR rides Storm's own magenta.
            vfr: srgb(132, 192, 120, 1.0),
            mvfr: srgb(124, 158, 236, 1.0),
            ifr: srgb(238, 124, 144, 1.0),
            lifr: srgb(188, 130, 236, 1.0),
            sigmet: srgb(222, 156, 102, 0.45),
            // Gridded overlays, muted with a faint cool cast.
            cloud_cover: Colormap::new(&[
                stop(10.0, (150, 154, 164), 0.0),
                stop(40.0, (158, 162, 172), 0.12),
                stop(75.0, (184, 188, 198), 0.28),
                stop(100.0, (206, 209, 217), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (110, 142, 220), 0.0),
                stop(1.0, (110, 142, 220), 0.32),
                stop(5.0, (102, 178, 198), 0.42),
                stop(20.0, (206, 182, 102), 0.5),
                stop(50.0, (210, 102, 98), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (220, 162, 102), 0.0),
                stop(5.0, (214, 146, 94), 0.32),
                stop(15.0, (208, 102, 98), 0.5),
            ]),
        },
        // Route: bright tokyo yellow (#e0af68 vivified); conflicts in #f7768e
        // red.
        route: RouteTheme {
            line: srgb(250, 194, 80, 1.0),
            line_conflict: srgb(236, 86, 98, 1.0),
            handle_fill: srgb(250, 194, 80, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(250, 194, 80, 0.12),
        },
        labels: LabelTheme {
            // Softened from the UI foreground #c0caf5; no halo (dark mode).
            text: srgb(192, 198, 224, 0.95),
            halo: [0.0; 4],
        },
        // Relief sinks into the slate band; cool grey-violet lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x16, 0x18, 0x26),
            light_tint: tint_from_srgb8(0x84, 0x88, 0x9c),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

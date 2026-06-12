//! "Tokyo Moon" — map theme paired with the Tokyo Moon UI theme.
//!
//! Rationale: Moon (tokyonight "moon" flavor) is the softest sibling — the
//! UI window (`background #222436`, `title_bar #1e2030`, `panel #2d3149`)
//! becomes a moonlit indigo ground band (land `#1b1d2c`) between Night's
//! near-black and Storm's slate, with the compressed road ramp re-tinted
//! into it. Moon's accents are brighter and creamier than its siblings':
//! controlled airspace uses the periwinkle `#82aaff`, CTR / restricted /
//! prohibited the coral `#ff757f`, danger and glider/para the warm apricot
//! of `#ffc777`, RMZ the ice cyan `#86e1fc`, TMZ the lavender-grey of the
//! muted foreground `#6e738d`. Boundaries and labels sit a step lighter
//! than Night's, matching Moon's lifted muted tones.

use super::{AirspaceColors, AirspaceTheme, BasemapTheme, ColorStop, Colormap, LabelTheme,
            MapTheme, MapThemeMode, RouteTheme, SymbolTheme, WeatherTheme, srgb8, srgb8_a};
use crate::layers::style::srgb;
use crate::terrain::{TerrainStyle, tint_from_srgb8};

// Airspace hues from the Tokyo Moon accent set (sRGB bytes, softened).
const BLUE: (u8, u8, u8) = (130, 164, 236); // #82aaff — controlled
const FAINT_BLUE: (u8, u8, u8) = (140, 164, 218); // class E/F band
const CYAN: (u8, u8, u8) = (118, 184, 224); // #86e1fc — RMZ
const ROSE: (u8, u8, u8) = (232, 122, 132); // #ff757f — CTR / ED-R / ED-P
const APRICOT: (u8, u8, u8) = (230, 148, 104); // danger (coral toward #ffc777)
const SAND: (u8, u8, u8) = (220, 182, 122); // #ffc777 — glider / para-jump
const LAVENDER_GREY: (u8, u8, u8) = (152, 154, 178); // #6e738d lifted — TMZ
const NEUTRAL: (u8, u8, u8) = (144, 146, 156);

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
    // Moonlit indigo ground a few channels under the UI background
    // #222436, keeping its cool b > r balance — between Night's near-black
    // and Storm's slate.
    let land = srgb8(0x1b, 0x1d, 0x2c);
    MapTheme {
        id: "tokyo-moon",
        name: "Tokyo Moon",
        mode: MapThemeMode::Dark,
        basemap: BasemapTheme {
            land,
            // Barely darker and bluer than land — water recedes, still cool.
            water: srgb8(0x17, 0x1a, 0x2a),
            waterway: srgb8(0x26, 0x30, 0x46),
            // Landcover: ±1..3 channels around land, indigo throughout.
            forest: srgb8(0x19, 0x1b, 0x2a),
            grass: srgb8(0x1a, 0x1c, 0x2b),
            farmland: srgb8(0x1d, 0x1e, 0x2d),
            barren: srgb8(0x1e, 0x1f, 0x2d),
            glacier: srgb8(0x20, 0x22, 0x33),
            park: srgb8(0x1a, 0x1c, 0x2b),
            urban: srgb8(0x20, 0x22, 0x32),
            urban_dense: srgb8(0x23, 0x25, 0x36),
            military: srgb8(0x1f, 0x20, 0x30),
            aerodrome: srgb8(0x21, 0x22, 0x35),
            // Compressed road ramp: +16 motorway down to +2 path over land
            // (luma ratio ≈ 1.54) — faint texture, never competing lines.
            road_highway: srgb8(0x2b, 0x2d, 0x3d),
            road_major: srgb8(0x25, 0x27, 0x36),
            road_medium: srgb8(0x22, 0x24, 0x33),
            road_minor: srgb8(0x1f, 0x21, 0x30),
            path: srgb8(0x1d, 0x1f, 0x2e),
            rail: srgb8_a(0x24, 0x26, 0x35, 0.85),
            // Boundaries from the muted foreground #6e738d family — a step
            // lighter than Tokyo Night's, like Moon's UI muted tones.
            boundary_country: srgb8_a(0x68, 0x6e, 0x8c, 0.55),
            boundary_region: srgb8_a(0x4e, 0x53, 0x6e, 0.30),
            place_label: srgb8(0x5c, 0x62, 0x7e),
            country_label: srgb8(0x68, 0x6e, 0x8e),
            water_label: srgb8(0x46, 0x54, 0x74),
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
            tmz: pair(LAVENDER_GREY, 0.03, 0.75),
            danger: pair(APRICOT, 0.06, 0.7),
            restricted: pair(ROSE, 0.12, 0.8),
            prohibited: pair(ROSE, 0.16, 0.85),
            glider_sector: pair(SAND, 0.045, 0.75),
            para_jump: pair(SAND, 0.045, 0.7),
            other: pair(NEUTRAL, 0.02, 0.45),
        },
        symbols: SymbolTheme {
            airport: srgb(200, 204, 220, 1.0),
            glider: srgb(220, 186, 128, 1.0),
            navaid: srgb(152, 168, 204, 1.0),
            reporting: srgb(216, 220, 236, 1.0),
            obstacle: srgb(228, 130, 136, 1.0),
            weather_dot: [1.0, 1.0, 1.0, 1.0],
            weather_outline: srgb(32, 34, 50, 1.0),
        },
        weather: WeatherTheme {
            // Moon's brighter accents, kept semantic: green / blue / red /
            // magenta.
            vfr: srgb(162, 210, 130, 1.0),
            mvfr: srgb(130, 168, 248, 1.0),
            ifr: srgb(244, 126, 134, 1.0),
            lifr: srgb(200, 132, 216, 1.0),
            sigmet: srgb(236, 176, 120, 0.45),
            // Gridded overlays, muted with a faint cool cast, a touch
            // brighter than the siblings'.
            cloud_cover: Colormap::new(&[
                stop(10.0, (152, 156, 168), 0.0),
                stop(40.0, (160, 164, 176), 0.12),
                stop(75.0, (186, 190, 202), 0.28),
                stop(100.0, (212, 215, 223), 0.45),
            ]),
            precip_rate: Colormap::new(&[
                stop(0.1, (114, 148, 230), 0.0),
                stop(1.0, (114, 148, 230), 0.32),
                stop(5.0, (106, 186, 208), 0.42),
                stop(20.0, (216, 190, 104), 0.5),
                stop(50.0, (218, 104, 100), 0.58),
            ]),
            thunderstorm: Colormap::new(&[
                stop(1.0, (228, 172, 110), 0.0),
                stop(5.0, (222, 154, 96), 0.32),
                stop(15.0, (214, 104, 100), 0.5),
            ]),
        },
        // Route: bright tokyo yellow #ffc777 — high-vis over the moon indigo;
        // conflicts in #ff757f red.
        route: RouteTheme {
            line: srgb(255, 200, 85, 1.0),
            line_conflict: srgb(240, 86, 96, 1.0),
            handle_fill: srgb(255, 200, 85, 1.0),
            handle_outline: srgb(12, 12, 14, 1.0),
            corridor: srgb(255, 200, 85, 0.12),
        },
        labels: LabelTheme {
            // Softened from the UI foreground #c0caf5; no halo (dark mode).
            text: srgb(200, 204, 232, 0.95),
            halo: [0.0; 4],
        },
        // Relief sinks into the moonlit band; cool grey-violet lights.
        terrain: TerrainStyle {
            shadow_tint: tint_from_srgb8(0x15, 0x16, 0x24),
            light_tint: tint_from_srgb8(0x86, 0x8a, 0x9e),
            opacity: 0.45,
        },
        clear_color: land,
    }
}

//! Map color themes: every color the renderer paints with, as one plain
//! data struct ([`MapTheme`]) the style modules read from. Geometry rules —
//! stroke widths, dash patterns, zoom gates, label priorities — are *not*
//! themed; they live with the style code and are shared by all themes.
//!
//! Three hand-crafted reference themes ship, one module each:
//! [`MapTheme::oldworld`] (the default soft dark look — THE dark
//! reference), [`MapTheme::high_contrast`] (the original high-saturation
//! chart look, kept byte-exact) and [`MapTheme::pastel_light`] (THE light
//! reference). Every other built-in pairs with one UI theme from
//! `assets/themes/*.json` by *name* (see [`MapTheme::by_name`]) and lives
//! in its own module too; until its family agent crafts the real palette
//! it is a placeholder clone of the matching reference (crafting rules:
//! `docs/map-theme-crafting.md`).
//!
//! # Color spaces (per section — see each struct's docs)
//!
//! The renderer inherited two authoring conventions which the theme keeps,
//! field by field, so switching the style code over is value-identical:
//!
//! * [`BasemapTheme`] colors and [`MapTheme::clear_color`] are
//!   **premultiplied display-space sRGB** floats (authored == displayed; see
//!   the rationale on [`srgb8`]).
//! * [`AirspaceTheme`], [`SymbolTheme`], [`WeatherTheme`] and [`LabelTheme`]
//!   colors are **premultiplied linear** RGBA (authored in sRGB bytes via
//!   [`crate::layers::style::srgb`]).
//! * [`MapTheme::terrain`] tints are linear, alpha 1
//!   ([`crate::terrain::tint_from_srgb8`]).

mod colormap;
mod high_contrast;
mod oldworld;
mod pastel_light;

// UI-theme-paired themes (alphabetical; placeholders until crafted).
mod adventure;
mod adventure_time;
mod alduin;
mod asciinema;
mod ayu_dark;
mod ayu_light;
mod catppuccin_frappe;
mod catppuccin_latte;
mod catppuccin_macchiato;
mod catppuccin_mocha;
mod default_dark;
mod default_light;
mod everforest_dark;
mod everforest_light;
mod fahrenheit;
mod flexoki_dark;
mod flexoki_light;
mod gruvbox_dark;
mod gruvbox_light;
mod harper;
mod hybrid_dark;
mod hybrid_light;
mod jellybeans;
mod kibble;
mod macos_classic_dark;
mod macos_classic_light;
mod matrix;
mod mellifluous_dark;
mod mellifluous_light;
mod molokai_dark;
mod molokai_light;
mod solarized_dark;
mod solarized_light;
mod spaceduck;
mod tokyo_moon;
mod tokyo_night;
mod tokyo_storm;
mod twilight;

pub use colormap::{ColorStop, Colormap, MAX_COLORMAP_STOPS};

use crate::features::{AirspaceStyleKey, IcaoClass};
use crate::terrain::TerrainStyle;

/// Brightness family of a map theme. Drives the mode-aware palette
/// invariants (see the tests in this module) and the by-mode fallback when
/// resolving an "auto" map theme in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapThemeMode {
    Dark,
    Light,
}

impl MapThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, MapThemeMode::Dark)
    }
}

/// A complete map color theme. Plain data: constructing one performs no IO
/// and touches no GPU state; apply it via
/// [`crate::renderer::MapRenderer::set_map_theme`].
#[derive(Debug, Clone, PartialEq)]
pub struct MapTheme {
    /// Stable identifier (`"oldworld"`, `"high-contrast"`, `"pastel-light"`).
    pub id: &'static str,
    /// Human-readable name. For UI-theme-paired themes this matches the UI
    /// theme's name *exactly* (`"Catppuccin Mocha"`) — the app's "auto" map
    /// theme resolves through this equality.
    pub name: &'static str,
    /// Brightness family (dark or light basemap).
    pub mode: MapThemeMode,
    /// Vector basemap palette (ground, roads, boundaries, place labels).
    pub basemap: BasemapTheme,
    /// Airspace fill/border colors per [`AirspaceStyleKey`].
    pub airspace: AirspaceTheme,
    /// Point-symbol mesh colors per kind family.
    pub symbols: SymbolTheme,
    /// Weather colors: METAR flight categories and the SIGMET hatch.
    pub weather: WeatherTheme,
    /// Feature-label text defaults and the (optional) glyph halo.
    pub labels: LabelTheme,
    /// Flight-route colors (polyline, handles, corridor).
    pub route: RouteTheme,
    /// Hillshade tint (shadow/light tints + layer opacity).
    pub terrain: TerrainStyle,
    /// Renderer clear color, premultiplied display-space RGBA. Must match
    /// [`BasemapTheme::land`] so a not-yet-loaded tile fades in without a
    /// hard edge.
    pub clear_color: [f32; 4],
}

impl MapTheme {
    /// Ids of the built-in themes, in presentation order: the three
    /// hand-crafted originals first, then the UI-theme-paired catalog
    /// alphabetically.
    pub const BUILT_IN_IDS: [&'static str; 41] = [
        "oldworld",
        "high-contrast",
        "pastel-light",
        "adventure",
        "adventure-time",
        "alduin",
        "asciinema",
        "ayu-dark",
        "ayu-light",
        "catppuccin-frappe",
        "catppuccin-latte",
        "catppuccin-macchiato",
        "catppuccin-mocha",
        "default-dark",
        "default-light",
        "everforest-dark",
        "everforest-light",
        "fahrenheit",
        "flexoki-dark",
        "flexoki-light",
        "gruvbox-dark",
        "gruvbox-light",
        "harper",
        "hybrid-dark",
        "hybrid-light",
        "jellybeans",
        "kibble",
        "macos-classic-dark",
        "macos-classic-light",
        "matrix",
        "mellifluous-dark",
        "mellifluous-light",
        "molokai-dark",
        "molokai-light",
        "solarized-dark",
        "solarized-light",
        "spaceduck",
        "tokyo-moon",
        "tokyo-night",
        "tokyo-storm",
        "twilight",
    ];

    /// The default dark look: desaturated pastel hues (steel blue / dusty
    /// rose), slightly less contrast than [`Self::high_contrast`].
    pub fn oldworld() -> Self {
        oldworld::theme()
    }

    /// The original high-saturation dark chart look.
    pub fn high_contrast() -> Self {
        high_contrast::theme()
    }

    /// Light map: warm paper land, darker desaturated features, muted
    /// pastel airspaces, dark label text with a light halo.
    pub fn pastel_light() -> Self {
        pastel_light::theme()
    }

    /// A built-in theme by its [`id`](Self::id), if it exists.
    pub fn by_id(id: &str) -> Option<Self> {
        match id {
            "oldworld" => Some(Self::oldworld()),
            "high-contrast" => Some(Self::high_contrast()),
            "pastel-light" => Some(Self::pastel_light()),
            "adventure" => Some(adventure::theme()),
            "adventure-time" => Some(adventure_time::theme()),
            "alduin" => Some(alduin::theme()),
            "asciinema" => Some(asciinema::theme()),
            "ayu-dark" => Some(ayu_dark::theme()),
            "ayu-light" => Some(ayu_light::theme()),
            "catppuccin-frappe" => Some(catppuccin_frappe::theme()),
            "catppuccin-latte" => Some(catppuccin_latte::theme()),
            "catppuccin-macchiato" => Some(catppuccin_macchiato::theme()),
            "catppuccin-mocha" => Some(catppuccin_mocha::theme()),
            "default-dark" => Some(default_dark::theme()),
            "default-light" => Some(default_light::theme()),
            "everforest-dark" => Some(everforest_dark::theme()),
            "everforest-light" => Some(everforest_light::theme()),
            "fahrenheit" => Some(fahrenheit::theme()),
            "flexoki-dark" => Some(flexoki_dark::theme()),
            "flexoki-light" => Some(flexoki_light::theme()),
            "gruvbox-dark" => Some(gruvbox_dark::theme()),
            "gruvbox-light" => Some(gruvbox_light::theme()),
            "harper" => Some(harper::theme()),
            "hybrid-dark" => Some(hybrid_dark::theme()),
            "hybrid-light" => Some(hybrid_light::theme()),
            "jellybeans" => Some(jellybeans::theme()),
            "kibble" => Some(kibble::theme()),
            "macos-classic-dark" => Some(macos_classic_dark::theme()),
            "macos-classic-light" => Some(macos_classic_light::theme()),
            "matrix" => Some(matrix::theme()),
            "mellifluous-dark" => Some(mellifluous_dark::theme()),
            "mellifluous-light" => Some(mellifluous_light::theme()),
            "molokai-dark" => Some(molokai_dark::theme()),
            "molokai-light" => Some(molokai_light::theme()),
            "solarized-dark" => Some(solarized_dark::theme()),
            "solarized-light" => Some(solarized_light::theme()),
            "spaceduck" => Some(spaceduck::theme()),
            "tokyo-moon" => Some(tokyo_moon::theme()),
            "tokyo-night" => Some(tokyo_night::theme()),
            "tokyo-storm" => Some(tokyo_storm::theme()),
            "twilight" => Some(twilight::theme()),
            _ => None,
        }
    }

    /// A built-in theme by its exact [`name`](Self::name), if any. This is
    /// how the app's "auto" map theme follows the active UI theme: the
    /// UI-theme-paired map themes carry the UI theme's name verbatim.
    pub fn by_name(name: &str) -> Option<Self> {
        Self::BUILT_IN_IDS
            .iter()
            .filter_map(|id| Self::by_id(id))
            .find(|theme| theme.name == name)
    }
}

impl Default for MapTheme {
    fn default() -> Self {
        Self::oldworld()
    }
}

/// Basemap palette. All colors are **premultiplied display-space sRGB**
/// floats (see [`srgb8`]) — the basemap shaders output them verbatim.
///
/// Design constraints carried over from the original palette: the basemap is
/// *background* for the aero overlays, so contrast stays low — but a VFR
/// pilot still needs ground reference: water must read as water
/// (hue-separated from land), urban areas as distinct patches, motorways as
/// traceable lines, admin borders clearly visible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasemapTheme {
    /// Land ("earth"). Must equal [`MapTheme::clear_color`] (no-tile == land).
    pub land: [f32; 4],
    /// Water fills — chart convention: water recedes behind land.
    pub water: [f32; 4],
    /// Waterway centerlines (rivers, canals) and shoreline strokes.
    pub waterway: [f32; 4],
    // Landcover / landuse (subtle tints over `land`).
    pub forest: [f32; 4],
    pub grass: [f32; 4],
    pub farmland: [f32; 4],
    pub barren: [f32; 4],
    pub glacier: [f32; 4],
    pub park: [f32; 4],
    /// Residential / general urban fabric.
    pub urban: [f32; 4],
    /// Industrial / commercial / institutional.
    pub urban_dense: [f32; 4],
    pub military: [f32; 4],
    pub aerodrome: [f32; 4],
    // Road classes.
    pub road_highway: [f32; 4],
    pub road_major: [f32; 4],
    pub road_medium: [f32; 4],
    pub road_minor: [f32; 4],
    pub path: [f32; 4],
    pub rail: [f32; 4],
    // Boundaries.
    /// Country borders (admin_level 2), dashed.
    pub boundary_country: [f32; 4],
    /// Region borders (admin_level 4), dashed and fainter.
    pub boundary_region: [f32; 4],
    // Place labels.
    /// Locality / region names.
    pub place_label: [f32; 4],
    /// Country names — slightly brighter than localities.
    pub country_label: [f32; 4],
    /// Waterway names.
    pub water_label: [f32; 4],
}

/// Fill and border colors of one airspace category, **premultiplied
/// linear** RGBA. Stroke widths and dash patterns are shared by all themes
/// (see [`crate::layers::style::airspace_style`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirspaceColors {
    /// Translucent fill, so stacked airspaces read.
    pub fill: [f32; 4],
    pub border: [f32; 4],
}

/// Airspace colors per [`AirspaceStyleKey`] (German VFR / ICAO chart
/// conventions). One field per key so completeness is a compile-time
/// guarantee.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirspaceTheme {
    pub class_a: AirspaceColors,
    pub class_b: AirspaceColors,
    pub class_c: AirspaceColors,
    pub class_d: AirspaceColors,
    pub class_e: AirspaceColors,
    pub class_f: AirspaceColors,
    pub class_g: AirspaceColors,
    pub ctr: AirspaceColors,
    pub rmz: AirspaceColors,
    pub tmz: AirspaceColors,
    pub danger: AirspaceColors,
    pub restricted: AirspaceColors,
    pub prohibited: AirspaceColors,
    pub glider_sector: AirspaceColors,
    pub para_jump: AirspaceColors,
    pub other: AirspaceColors,
}

impl AirspaceTheme {
    /// The colors for one style key (exhaustive by construction).
    pub fn colors(&self, key: AirspaceStyleKey) -> AirspaceColors {
        match key {
            AirspaceStyleKey::IcaoClass(IcaoClass::A) => self.class_a,
            AirspaceStyleKey::IcaoClass(IcaoClass::B) => self.class_b,
            AirspaceStyleKey::IcaoClass(IcaoClass::C) => self.class_c,
            AirspaceStyleKey::IcaoClass(IcaoClass::D) => self.class_d,
            AirspaceStyleKey::IcaoClass(IcaoClass::E) => self.class_e,
            AirspaceStyleKey::IcaoClass(IcaoClass::F) => self.class_f,
            AirspaceStyleKey::IcaoClass(IcaoClass::G) => self.class_g,
            AirspaceStyleKey::Ctr => self.ctr,
            AirspaceStyleKey::Rmz => self.rmz,
            AirspaceStyleKey::Tmz => self.tmz,
            AirspaceStyleKey::Danger => self.danger,
            AirspaceStyleKey::Restricted => self.restricted,
            AirspaceStyleKey::Prohibited => self.prohibited,
            AirspaceStyleKey::GliderSector => self.glider_sector,
            AirspaceStyleKey::ParaJump => self.para_jump,
            AirspaceStyleKey::Other => self.other,
        }
    }
}

/// Point-symbol mesh colors per kind family, **premultiplied linear** RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SymbolTheme {
    /// Airports, airfields, heliports, ultralight sites.
    pub airport: [f32; 4],
    /// Glider sites.
    pub glider: [f32; 4],
    /// VOR / DME / NDB / TACAN.
    pub navaid: [f32; 4],
    /// VFR reporting points.
    pub reporting: [f32; 4],
    /// Obstacles (towers, masts).
    pub obstacle: [f32; 4],
    /// METAR dot base; tinted per instance by the flight-category color, so
    /// this is white in every sane theme.
    pub weather_dot: [f32; 4],
    /// METAR dot rim; also tinted by the flight-category color.
    pub weather_outline: [f32; 4],
}

/// Weather colors, **premultiplied linear** RGBA. Flight-category colors
/// must stay semantically recognizable in every theme: VFR greenish, MVFR
/// blueish, IFR reddish, LIFR magenta-ish (pastelized is fine).
///
/// The gridded-overlay colormaps follow the same semantic rules in every
/// theme — cloud cover a neutral gray/white ramp, precipitation the classic
/// blue → cyan → yellow → red radar ramp, thunderstorm potential
/// amber → red — but themes vary the *intensity*: muted (lower alpha,
/// desaturated) in Oldworld / Pastel Light, stronger in High Contrast.
/// Every map starts with a fully transparent stop, so values below the
/// first breakpoint render as nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherTheme {
    pub vfr: [f32; 4],
    pub mvfr: [f32; 4],
    pub ifr: [f32; 4],
    pub lifr: [f32; 4],
    /// Hatched-overlay color for SIGMET polygons (translucent).
    pub sigmet: [f32; 4],
    /// Cloud-cover colormap over **percent** (0–100); transparent below
    /// ~10 %, translucent neutral ramp above (peak alpha ≤ 0.55).
    pub cloud_cover: Colormap,
    /// Precipitation-rate colormap over **mm/h** with log-ish breakpoints
    /// at 0.1 / 1 / 5 / 20 (blue → cyan → yellow → red).
    pub precip_rate: Colormap,
    /// Thunderstorm-potential colormap over **J/kg** (LPI scale);
    /// transparent below ~1, then amber → red.
    pub thunderstorm: Colormap,
}

/// Flight-route colors, **premultiplied linear** RGBA (authored in sRGB
/// bytes via [`crate::layers::style::srgb`] like the other aero sections).
///
/// The route is the user's *own* object on the chart, so its line is each
/// theme's strongest free accent — high-visibility, never a hue already
/// carrying airspace meaning in that theme, and contrasting with both land
/// and water (the invariant tests in this module pin luma + hue floors).
/// Conflict legs use the theme's danger tone (red-dominant, hue-separated
/// from the line).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteTheme {
    /// Route polyline + direction ticks (opaque).
    pub line: [f32; 4],
    /// Tint of legs with active conflicts — the theme's danger tone (opaque).
    pub line_conflict: [f32; 4],
    /// Waypoint-handle fill (departure square, waypoint circle, destination
    /// flag, TOC/TOD and scrub markers).
    pub handle_fill: [f32; 4],
    /// Waypoint-handle outline / casing — land-toned so handles pop off the
    /// line (dark themes: near-black; light themes: near-paper).
    pub handle_outline: [f32; 4],
    /// Terrain-corridor stroke around the track (translucent).
    pub corridor: [f32; 4],
}

/// Label text defaults, **premultiplied linear** RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelTheme {
    /// Default text color for feature ident labels (airports, navaids, …).
    /// Basemap place labels and airspace band labels carry their own colors.
    pub text: [f32; 4],
    /// Glyph halo painted under *all* labels. Fully transparent disables the
    /// halo (dark themes); light themes use a light halo so dark text stays
    /// legible over colored fills.
    pub halo: [f32; 4],
}

/// sRGB byte channel → the display-space float the basemap shaders output.
fn channel(c: u8) -> f32 {
    f32::from(c) / 255.0
}

/// Opaque premultiplied **display-space** RGBA from sRGB bytes.
///
/// Basemap colors are authored in display space and the shaders output them
/// verbatim: the offscreen target is `Rgba8UnormSrgb`, so a fragment value
/// is sRGB-encoded on store and decoded back to the *same float* when gpui
/// samples it; gpui-ce composites in display space onto a non-sRGB
/// swapchain, so the sampled value hits the screen raw. Linearizing here
/// would *display* the linear values (everything collapses to near-black).
/// Blending consequently happens in sRGB space — the same convention as the
/// surrounding gpui UI.
pub fn srgb8(r: u8, g: u8, b: u8) -> [f32; 4] {
    srgb8_a(r, g, b, 1.0)
}

/// Premultiplied display-space RGBA from sRGB bytes and a straight alpha.
pub fn srgb8_a(r: u8, g: u8, b: u8, alpha: f32) -> [f32; 4] {
    let a = alpha.clamp(0.0, 1.0);
    [channel(r) * a, channel(g) * a, channel(b) * a, a]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::style::{ALL_STYLE_KEYS, srgb};
    use crate::terrain::tint_from_srgb8;

    /// The three hand-crafted reference palettes — original per-theme
    /// expectations stay pinned on these.
    const ORIGINAL_IDS: [&str; 3] = ["oldworld", "high-contrast", "pastel-light"];

    fn all_built_in() -> Vec<MapTheme> {
        MapTheme::BUILT_IN_IDS
            .iter()
            .map(|id| MapTheme::by_id(id).expect("built-in id resolves"))
            .collect()
    }

    fn luma(c: [f32; 4]) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    /// Linearize one display-space sRGB channel (the basemap convention).
    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG relative luminance of an opaque display-space basemap color.
    fn display_rel_luminance(c: [f32; 4]) -> f32 {
        0.2126 * srgb_to_linear(c[0])
            + 0.7152 * srgb_to_linear(c[1])
            + 0.0722 * srgb_to_linear(c[2])
    }

    /// WCAG relative luminance of a premultiplied *linear* color (labels):
    /// unpremultiply, then weight the already-linear channels.
    fn linear_rel_luminance(c: [f32; 4]) -> f32 {
        let a = c[3].max(1e-4);
        (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) / a
    }

    /// WCAG contrast ratio between two relative luminances.
    fn contrast_ratio(y1: f32, y2: f32) -> f32 {
        let (hi, lo) = if y1 > y2 { (y1, y2) } else { (y2, y1) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Hue angle in degrees (0..360) of an RGB color (achromatic → 0).
    fn hue_degrees(c: [f32; 4]) -> f32 {
        let (r, g, b) = (c[0], c[1], c[2]);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if delta <= f32::EPSILON {
            return 0.0;
        }
        let h = if max == r {
            (g - b) / delta
        } else if max == g {
            2.0 + (b - r) / delta
        } else {
            4.0 + (r - g) / delta
        };
        (h * 60.0).rem_euclid(360.0)
    }

    /// Shortest angular distance between two hues, in degrees.
    fn hue_distance(a: f32, b: f32) -> f32 {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    }

    fn assert_premultiplied(label: &str, color: [f32; 4]) {
        for c in color {
            assert!((0.0..=1.0).contains(&c), "{label}: channel out of range");
        }
        for c in &color[..3] {
            assert!(*c <= color[3] + 1e-6, "{label}: color not premultiplied");
        }
    }

    #[test]
    fn premultiplied_alpha_scales_all_channels() {
        let opaque = srgb8(0x8b, 0x86, 0x93);
        let half = srgb8_a(0x8b, 0x86, 0x93, 0.5);
        for i in 0..3 {
            assert!((half[i] - opaque[i] * 0.5).abs() < 1e-6);
        }
        assert!((half[3] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn built_in_ids_resolve_and_are_distinct() {
        for id in MapTheme::BUILT_IN_IDS {
            let theme = MapTheme::by_id(id).expect("built-in id resolves");
            assert_eq!(theme.id, id);
            assert!(!theme.name.is_empty());
        }
        let themes = all_built_in();
        for (i, a) in themes.iter().enumerate() {
            for b in &themes[i + 1..] {
                assert_ne!(a, b, "{} and {} must differ", a.id, b.id);
            }
        }
        // Presentation order: the three originals first, then alphabetical.
        assert_eq!(&MapTheme::BUILT_IN_IDS[..3], &ORIGINAL_IDS);
        let catalog = &MapTheme::BUILT_IN_IDS[3..];
        assert!(
            catalog.windows(2).all(|w| w[0] < w[1]),
            "catalog must stay sorted"
        );
        assert!(MapTheme::by_id("does-not-exist").is_none());
        // The default map theme is Oldworld (the soft pastel dark palette).
        assert_eq!(MapTheme::default(), MapTheme::oldworld());
    }

    /// Names are unique and `by_name` finds every built-in — the app's
    /// "auto" map theme follows the active UI theme through this lookup, so
    /// every UI-theme-paired map theme must carry the UI theme's exact name.
    #[test]
    fn by_name_resolves_every_built_in_uniquely() {
        for theme in all_built_in() {
            let found = MapTheme::by_name(theme.name).expect("name resolves");
            assert_eq!(found.id, theme.id, "duplicate theme name {:?}", theme.name);
        }
        assert!(MapTheme::by_name("No Such Theme").is_none());
        assert!(
            MapTheme::by_name("oldworld").is_none(),
            "by_name matches display names, not ids"
        );
    }

    /// Every airspace style key has well-formed colors in every theme.
    #[test]
    fn every_theme_covers_every_airspace_key() {
        for theme in all_built_in() {
            for key in ALL_STYLE_KEYS {
                let colors = theme.airspace.colors(key);
                assert!(
                    colors.fill[3] > 0.0 && colors.fill[3] <= 0.5,
                    "{} {key:?}: fill must be translucent, got alpha {}",
                    theme.id,
                    colors.fill[3]
                );
                assert!(
                    colors.border[3] > 0.0 && colors.border[3] <= 1.0,
                    "{} {key:?}: border alpha out of range",
                    theme.id
                );
                assert_premultiplied(&format!("{} {key:?} fill", theme.id), colors.fill);
                assert_premultiplied(&format!("{} {key:?} border", theme.id), colors.border);
            }
        }
    }

    /// All other theme sections stay well-formed (range + premultiplication)
    /// and the structural invariants hold in every theme.
    #[test]
    fn every_theme_is_well_formed() {
        for theme in all_built_in() {
            let b = theme.basemap;
            for (label, color) in [
                ("land", b.land),
                ("water", b.water),
                ("waterway", b.waterway),
                ("forest", b.forest),
                ("grass", b.grass),
                ("farmland", b.farmland),
                ("barren", b.barren),
                ("glacier", b.glacier),
                ("park", b.park),
                ("urban", b.urban),
                ("urban_dense", b.urban_dense),
                ("military", b.military),
                ("aerodrome", b.aerodrome),
                ("road_highway", b.road_highway),
                ("road_major", b.road_major),
                ("road_medium", b.road_medium),
                ("road_minor", b.road_minor),
                ("path", b.path),
                ("rail", b.rail),
                ("boundary_country", b.boundary_country),
                ("boundary_region", b.boundary_region),
                ("place_label", b.place_label),
                ("country_label", b.country_label),
                ("water_label", b.water_label),
                ("clear_color", theme.clear_color),
                ("symbols.airport", theme.symbols.airport),
                ("symbols.glider", theme.symbols.glider),
                ("symbols.navaid", theme.symbols.navaid),
                ("symbols.reporting", theme.symbols.reporting),
                ("symbols.obstacle", theme.symbols.obstacle),
                ("symbols.weather_dot", theme.symbols.weather_dot),
                ("symbols.weather_outline", theme.symbols.weather_outline),
                ("weather.sigmet", theme.weather.sigmet),
                ("labels.text", theme.labels.text),
                ("labels.halo", theme.labels.halo),
            ] {
                assert_premultiplied(&format!("{} {label}", theme.id), color);
            }
            // The clear color must equal land (no-tile == land).
            assert_eq!(theme.clear_color, b.land, "{}", theme.id);
            // Country borders read stronger than region borders.
            assert!(b.boundary_country[3] > b.boundary_region[3], "{}", theme.id);
            // Terrain shadows darker than lights, opacity sane.
            let t = theme.terrain;
            assert!((0.0..=1.0).contains(&t.opacity), "{}", theme.id);
            for (s, l) in t.shadow_tint[..3].iter().zip(&t.light_tint[..3]) {
                assert!(s < l, "{}: shadow not darker than light", theme.id);
            }
        }
    }

    /// Water/land relationships per brightness family: dark themes keep
    /// water darker than land; the light theme keeps water *visibly darker*
    /// than its paper land so it still reads as water.
    #[test]
    fn water_reads_as_water_in_every_theme() {
        for theme in all_built_in() {
            assert!(
                luma(theme.basemap.water) < luma(theme.basemap.land),
                "{}: water must recede behind land",
                theme.id
            );
        }
        // Light theme: land is actually light, and roads/borders darker than
        // ground so they read.
        let light = MapTheme::pastel_light();
        assert!(luma(light.basemap.land) > 0.7, "paper-like land");
        assert!(luma(light.basemap.road_major) < luma(light.basemap.land) * 0.75);
        assert!(luma(light.basemap.place_label) < luma(light.basemap.land) * 0.6);
        // Dark themes: urban lighter than land.
        for theme in [MapTheme::oldworld(), MapTheme::high_contrast()] {
            assert!(
                luma(theme.basemap.urban) > luma(theme.basemap.land),
                "{}",
                theme.id
            );
        }
        // High Contrast keeps motorways clearly traceable …
        let hc = MapTheme::high_contrast();
        assert!(luma(hc.basemap.road_highway) > luma(hc.basemap.land) * 2.0);
        // … while Oldworld deliberately compresses roads to faint texture:
        // strictly above ground, but well below the traceable-line ratio so
        // the network never competes with the airspace overlays.
        let ow = MapTheme::oldworld();
        let ratio = luma(ow.basemap.road_highway) / luma(ow.basemap.land);
        assert!(
            ratio > 1.2,
            "oldworld motorways must stay (faintly) visible"
        );
        assert!(ratio < 2.0, "oldworld roads must stay faint texture");
    }

    /// Mode-aware basemap band invariants (docs/map-theme-crafting.md):
    /// dark themes keep the ground near-black with motorways as faint
    /// texture barely above it; light themes are paper with roads slightly
    /// darker than ground; in both, water recedes (darker than land) and
    /// leans at least as cool. High Contrast — pinned byte-exact from the
    /// pre-theme constants — predates the low-contrast principles, so it is
    /// exempt from the dark road-ratio *upper* bound only.
    #[test]
    fn basemap_band_invariants_hold_in_every_mode() {
        for theme in all_built_in() {
            let id = theme.id;
            let b = theme.basemap;
            let land = luma(b.land);
            let ratio = luma(b.road_highway) / land.max(1e-4);
            match theme.mode {
                MapThemeMode::Dark => {
                    assert!(
                        land < 0.25,
                        "{id}: dark land must stay near-black, luma {land:.3}"
                    );
                    assert!(
                        ratio > 1.05,
                        "{id}: motorways must stay (faintly) visible, ratio {ratio:.2}"
                    );
                    assert!(
                        id == "high-contrast" || ratio < 2.2,
                        "{id}: dark roads must stay faint texture, ratio {ratio:.2}"
                    );
                }
                MapThemeMode::Light => {
                    assert!(
                        land > 0.55,
                        "{id}: light land must read as paper, luma {land:.3}"
                    );
                    assert!(
                        ratio > 0.5 && ratio < 0.95,
                        "{id}: light roads must sit slightly darker than ground, ratio {ratio:.2}"
                    );
                }
            }
            // Water: distinct from land, receding, and relatively cooler
            // (blue/red balance at least land's) in both modes.
            assert_ne!(b.water, b.land, "{id}: water must differ from land");
            assert!(luma(b.water) < land, "{id}: water must recede behind land");
            assert!(
                b.water[2] * b.land[0] >= b.water[0] * b.land[2] - 1e-6,
                "{id}: water must lean cooler than land"
            );
        }
    }

    /// Label legibility in every theme: feature label text keeps a
    /// WCAG-AA-ish contrast ratio against land, and the glyph halo follows
    /// the mode rule — fully transparent (disabled) in dark themes, present
    /// in light themes so dark text survives colored fills.
    #[test]
    fn labels_stay_readable_in_every_theme() {
        for theme in all_built_in() {
            let id = theme.id;
            let ratio = contrast_ratio(
                linear_rel_luminance(theme.labels.text),
                display_rel_luminance(theme.basemap.land),
            );
            assert!(
                ratio >= 4.5,
                "{id}: label/land contrast {ratio:.2} below 4.5"
            );
            match theme.mode {
                MapThemeMode::Dark => assert_eq!(
                    theme.labels.halo[3], 0.0,
                    "{id}: dark themes disable the glyph halo"
                ),
                MapThemeMode::Light => assert!(
                    theme.labels.halo[3] > 0.0,
                    "{id}: light themes need a light halo under dark text"
                ),
            }
        }
    }

    /// Unpremultiply a premultiplied *linear* color and encode it to
    /// display-space sRGB floats — for comparing aero-layer colors (route,
    /// airspace) against the display-space basemap palette.
    fn linear_premul_to_display(c: [f32; 4]) -> [f32; 4] {
        fn encode(c: f32) -> f32 {
            if c <= 0.003_130_8 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            }
        }
        let a = c[3].max(1e-4);
        [encode(c[0] / a), encode(c[1] / a), encode(c[2] / a), c[3]]
    }

    /// HSV-style saturation (achromatic → 0).
    fn saturation(c: [f32; 4]) -> f32 {
        let max = c[0].max(c[1]).max(c[2]);
        let min = c[0].min(c[1]).min(c[2]);
        if max <= f32::EPSILON {
            0.0
        } else {
            (max - min) / max
        }
    }

    /// Route line vs. ground: WCAG-ish luma-contrast floor.
    const ROUTE_GROUND_CONTRAST_FLOOR: f32 = 2.5;
    /// Route hue separation floor (line vs. ground, line vs. conflict).
    const ROUTE_HUE_FLOOR_DEG: f32 = 25.0;
    /// Grounds with less chroma than this are effectively achromatic — they
    /// cannot hue-clash with the route, only luma applies.
    const ACHROMATIC_SATURATION: f32 = 0.12;

    /// Route palette is well-formed in every theme: premultiplied colors,
    /// an opaque line/conflict pair, an opaque-ish handle pair and a
    /// translucent corridor.
    #[test]
    fn route_theme_is_well_formed_in_every_theme() {
        for theme in all_built_in() {
            let r = theme.route;
            for (label, color) in [
                ("route.line", r.line),
                ("route.line_conflict", r.line_conflict),
                ("route.handle_fill", r.handle_fill),
                ("route.handle_outline", r.handle_outline),
                ("route.corridor", r.corridor),
            ] {
                assert_premultiplied(&format!("{} {label}", theme.id), color);
            }
            assert_eq!(r.line[3], 1.0, "{}: route line is opaque", theme.id);
            assert_eq!(
                r.line_conflict[3], 1.0,
                "{}: conflict tint is opaque",
                theme.id
            );
            assert!(
                r.handle_fill[3] >= 0.9,
                "{}: handle fill near-opaque",
                theme.id
            );
            assert!(
                r.handle_outline[3] >= 0.9,
                "{}: handle outline near-opaque",
                theme.id
            );
            assert!(
                r.corridor[3] > 0.0 && r.corridor[3] <= 0.3,
                "{}: corridor must be a translucent stroke, got alpha {}",
                theme.id,
                r.corridor[3]
            );
        }
    }

    /// The route is the user's own object: its line must read against both
    /// land and water in every theme — a luma-contrast floor always, plus a
    /// hue-distance floor wherever the ground actually carries a hue.
    #[test]
    fn route_line_contrasts_with_land_and_water_in_every_theme() {
        for theme in all_built_in() {
            let id = theme.id;
            let line = linear_premul_to_display(theme.route.line);
            let line_y = linear_rel_luminance(theme.route.line);
            for (label, ground) in [("land", theme.basemap.land), ("water", theme.basemap.water)] {
                let ratio = contrast_ratio(line_y, display_rel_luminance(ground));
                assert!(
                    ratio >= ROUTE_GROUND_CONTRAST_FLOOR,
                    "{id}: route line/{label} contrast {ratio:.2} below \
                     {ROUTE_GROUND_CONTRAST_FLOOR}"
                );
                if saturation(ground) >= ACHROMATIC_SATURATION {
                    let d = hue_distance(hue_degrees(line), hue_degrees(ground));
                    assert!(
                        d >= ROUTE_HUE_FLOOR_DEG,
                        "{id}: route line and {label} hues only {d:.1}° apart"
                    );
                }
            }
        }
    }

    /// Conflict legs read as danger in every theme: a red-dominant tone,
    /// hue-separated from the route line.
    #[test]
    fn route_conflict_is_a_danger_tone_distinct_from_the_line() {
        for theme in all_built_in() {
            let id = theme.id;
            let line = linear_premul_to_display(theme.route.line);
            let conflict = linear_premul_to_display(theme.route.line_conflict);
            assert_ne!(theme.route.line, theme.route.line_conflict, "{id}");
            let d = hue_distance(hue_degrees(line), hue_degrees(conflict));
            assert!(
                d >= ROUTE_HUE_FLOOR_DEG,
                "{id}: conflict and line hues only {d:.1}° apart"
            );
            assert!(
                conflict[0] > conflict[1] && conflict[0] > conflict[2],
                "{id}: conflict tint must be red-dominant (a danger tone)"
            );
        }
    }

    /// The route line must read as "mine", not as airspace: in every theme
    /// it keeps a clear distance from every airspace border color.
    #[test]
    fn route_line_is_not_an_airspace_class_color() {
        for theme in all_built_in() {
            let line = linear_premul_to_display(theme.route.line);
            for key in ALL_STYLE_KEYS {
                let border = linear_premul_to_display(theme.airspace.colors(key).border);
                let dist = line[..3]
                    .iter()
                    .zip(&border[..3])
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f32>()
                    .sqrt();
                assert!(
                    dist >= 0.10,
                    "{} {key:?}: route line within {dist:.3} of the airspace border color",
                    theme.id
                );
            }
        }
    }

    /// Flight-category colors stay *pairwise distinguishable by hue* in
    /// every theme — the channel-dominance test below pins each color to
    /// its semantic family; this pins their mutual separation on the hue
    /// wheel (the references sit at ≥ 58°; 30° leaves crafting headroom).
    #[test]
    fn flight_category_hues_stay_pairwise_separated() {
        for theme in all_built_in() {
            let w = theme.weather;
            let labeled = [
                ("vfr", w.vfr),
                ("mvfr", w.mvfr),
                ("ifr", w.ifr),
                ("lifr", w.lifr),
            ];
            for (i, (name_a, a)) in labeled.iter().enumerate() {
                for (name_b, b) in &labeled[i + 1..] {
                    let d = hue_distance(hue_degrees(*a), hue_degrees(*b));
                    assert!(
                        d >= 30.0,
                        "{}: {name_a} and {name_b} hues only {d:.1}° apart",
                        theme.id
                    );
                }
            }
        }
    }

    /// Flight-category colors stay semantically recognizable in all themes:
    /// VFR greenish, MVFR blueish, IFR reddish, LIFR magenta-ish — and
    /// mutually distinct.
    #[test]
    fn flight_categories_stay_recognizable_in_every_theme() {
        for theme in all_built_in() {
            let w = theme.weather;
            let id = theme.id;
            // VFR: green dominates.
            assert!(
                w.vfr[1] > w.vfr[0] && w.vfr[1] > w.vfr[2],
                "{id}: VFR not greenish"
            );
            // MVFR: blue dominates.
            assert!(
                w.mvfr[2] > w.mvfr[0] && w.mvfr[2] > w.mvfr[1],
                "{id}: MVFR not blueish"
            );
            // IFR: red dominates.
            assert!(
                w.ifr[0] > w.ifr[1] && w.ifr[0] > w.ifr[2],
                "{id}: IFR not reddish"
            );
            // LIFR: red and blue together dominate green (magenta).
            assert!(
                w.lifr[0] > w.lifr[1] && w.lifr[2] > w.lifr[1],
                "{id}: LIFR not magenta-ish"
            );
            let all = [w.vfr, w.mvfr, w.ifr, w.lifr];
            for (i, a) in all.iter().enumerate() {
                assert_eq!(a[3], 1.0, "{id}: category dots are opaque");
                for b in &all[i + 1..] {
                    assert_ne!(a, b, "{id}: category colors must be distinct");
                }
            }
        }
    }

    /// Every theme carries complete, well-formed gridded-weather colormaps:
    /// strictly ascending stops, premultiplied colors, a fully transparent
    /// first stop (the threshold) and a translucent peak.
    #[test]
    fn every_theme_has_complete_gridded_weather_colormaps() {
        for theme in all_built_in() {
            let w = theme.weather;
            for (label, map) in [
                ("cloud_cover", w.cloud_cover),
                ("precip_rate", w.precip_rate),
                ("thunderstorm", w.thunderstorm),
            ] {
                let stops = map.stops();
                assert!(stops.len() >= 3, "{} {label}: too few stops", theme.id);
                assert!(
                    stops.windows(2).all(|s| s[0].value < s[1].value),
                    "{} {label}: stop values must be strictly ascending",
                    theme.id
                );
                assert_eq!(
                    stops[0].color[3], 0.0,
                    "{} {label}: first stop must be fully transparent",
                    theme.id
                );
                for (i, s) in stops.iter().enumerate() {
                    assert_premultiplied(&format!("{} {label} stop {i}", theme.id), s.color);
                }
                let peak = map.max_alpha();
                assert!(
                    peak > 0.2 && peak <= 0.7,
                    "{} {label}: peak alpha {peak} not a translucent overlay",
                    theme.id
                );
            }
            // Field-specific breakpoints shared by all themes.
            assert_eq!(w.cloud_cover.stops()[0].value, 10.0, "{}", theme.id);
            assert!(w.cloud_cover.max_alpha() <= 0.55, "{}", theme.id);
            let precip_values: Vec<f32> = w.precip_rate.stops().iter().map(|s| s.value).collect();
            assert_eq!(precip_values, [0.1, 1.0, 5.0, 20.0, 50.0], "{}", theme.id);
            assert_eq!(w.thunderstorm.stops()[0].value, 1.0, "{}", theme.id);
        }
    }

    /// The colormap *semantics* hold in every theme (pure-function
    /// breakpoint checks): clouds transparent below 10 %, precipitation
    /// ramps blue → cyan → yellow → red across 1/5/20/50 mm/h, storms
    /// transparent below threshold then amber → red. High Contrast renders
    /// stronger than the muted themes.
    #[test]
    fn gridded_colormap_breakpoints_keep_their_semantics_in_every_theme() {
        for theme in all_built_in() {
            let id = theme.id;
            let w = theme.weather;

            // Clouds: nothing below 10 %, neutral (low saturation) above.
            assert_eq!(w.cloud_cover.sample(0.0)[3], 0.0, "{id}: clear sky");
            assert_eq!(w.cloud_cover.sample(9.9)[3], 0.0, "{id}: few clouds");
            let overcast = w.cloud_cover.sample(100.0);
            assert!(overcast[3] > 0.2, "{id}: overcast must be visible");
            let (max_c, min_c) = (
                overcast[..3].iter().fold(0.0f32, |a, &b| a.max(b)),
                overcast[..3].iter().fold(1.0f32, |a, &b| a.min(b)),
            );
            assert!(
                max_c - min_c < 0.12 * overcast[3],
                "{id}: cloud ramp must stay neutral gray/white"
            );

            // Precipitation: classic radar hue ramp.
            assert_eq!(
                w.precip_rate.sample(0.05)[3],
                0.0,
                "{id}: drizzle below 0.1"
            );
            let light = w.precip_rate.sample(1.0);
            assert!(
                light[2] > light[0] && light[2] > light[1],
                "{id}: 1 mm/h blueish"
            );
            let moderate = w.precip_rate.sample(5.0);
            assert!(
                moderate[1] > moderate[0] && moderate[2] > moderate[0],
                "{id}: 5 mm/h cyanish"
            );
            let heavy = w.precip_rate.sample(20.0);
            assert!(
                heavy[0] > heavy[2] && heavy[1] > heavy[2],
                "{id}: 20 mm/h yellowish"
            );
            let extreme = w.precip_rate.sample(50.0);
            assert!(
                extreme[0] > extreme[1] && extreme[0] > extreme[2],
                "{id}: 50 mm/h reddish"
            );
            assert!(
                light[3] < moderate[3] && moderate[3] < extreme[3],
                "{id}: precip opacity must grow with intensity"
            );

            // Thunderstorms: threshold, then amber → red.
            assert_eq!(w.thunderstorm.sample(0.5)[3], 0.0, "{id}: below threshold");
            let amber = w.thunderstorm.sample(5.0);
            assert!(
                amber[0] > amber[1] && amber[1] > amber[2],
                "{id}: mid potential amberish (r > g > b)"
            );
            let severe = w.thunderstorm.sample(15.0);
            assert!(
                severe[0] > severe[1] * 2.0 && severe[0] > severe[2] * 2.0,
                "{id}: severe potential reddish"
            );
        }

        // Theme intent: High Contrast stronger, Oldworld/Pastel muted.
        let hc = MapTheme::high_contrast().weather;
        for muted in [
            MapTheme::oldworld().weather,
            MapTheme::pastel_light().weather,
        ] {
            assert!(hc.cloud_cover.max_alpha() > muted.cloud_cover.max_alpha());
            assert!(hc.precip_rate.max_alpha() > muted.precip_rate.max_alpha());
            assert!(hc.thunderstorm.max_alpha() > muted.thunderstorm.max_alpha());
        }
    }

    /// Chart-density conventions hold in every theme: prohibited reads
    /// denser than restricted, restricted denser than danger; class E stays
    /// more subtle than class C.
    #[test]
    fn airspace_density_conventions_hold_in_every_theme() {
        for theme in all_built_in() {
            let a = &theme.airspace;
            assert!(a.restricted.fill[3] > a.danger.fill[3], "{}", theme.id);
            assert!(a.prohibited.fill[3] > a.restricted.fill[3], "{}", theme.id);
            assert!(a.class_e.fill[3] < a.class_c.fill[3], "{}", theme.id);
        }
    }

    /// Regression guard: the high-contrast theme is the *exact* pre-theme
    /// constant set (a representative sample of every section, byte-exact
    /// against the original authoring expressions). This palette shipped
    /// under the name "Oldworld" before the rename.
    #[test]
    fn high_contrast_matches_the_original_constants() {
        let theme = MapTheme::high_contrast();
        assert_eq!(theme.id, "high-contrast");
        assert_eq!(theme.name, "High Contrast");
        // Basemap (display-space palette, premultiplied).
        assert_eq!(theme.basemap.land, srgb8(0x21, 0x21, 0x24));
        assert_eq!(theme.basemap.water, srgb8(0x18, 0x22, 0x30));
        assert_eq!(theme.basemap.waterway, srgb8(0x3f, 0x56, 0x80));
        assert_eq!(theme.basemap.road_highway, srgb8(0x6b, 0x5a, 0x45));
        assert_eq!(theme.basemap.rail, srgb8_a(0x38, 0x38, 0x3f, 0.9));
        assert_eq!(
            theme.basemap.boundary_country,
            srgb8_a(0x7d, 0x78, 0x86, 0.7)
        );
        assert_eq!(theme.basemap.place_label, srgb8(0x5f, 0x5b, 0x68));
        assert_eq!(theme.clear_color, srgb8(0x21, 0x21, 0x24));
        // Airspace (premultiplied linear; original base hues).
        let ctr = theme.airspace.colors(AirspaceStyleKey::Ctr);
        assert_eq!(ctr.fill, srgb(214, 48, 58, 0.1));
        assert_eq!(ctr.border, srgb(214, 48, 58, 0.9));
        let c = theme
            .airspace
            .colors(AirspaceStyleKey::IcaoClass(IcaoClass::C));
        assert_eq!(c.fill, srgb(64, 110, 205, 0.08));
        assert_eq!(c.border, srgb(64, 110, 205, 0.9));
        let glider = theme.airspace.colors(AirspaceStyleKey::GliderSector);
        assert_eq!(glider.border, srgb(228, 168, 50, 0.85));
        // Symbols.
        assert_eq!(theme.symbols.airport, srgb(206, 201, 190, 1.0));
        assert_eq!(theme.symbols.obstacle, srgb(204, 120, 110, 1.0));
        assert_eq!(theme.symbols.weather_dot, [1.0, 1.0, 1.0, 1.0]);
        // Weather.
        assert_eq!(theme.weather.vfr, srgb(0, 176, 92, 1.0));
        assert_eq!(theme.weather.mvfr, srgb(20, 122, 255, 1.0));
        assert_eq!(theme.weather.ifr, srgb(229, 48, 57, 1.0));
        assert_eq!(theme.weather.lifr, srgb(199, 42, 199, 1.0));
        assert_eq!(theme.weather.sigmet, srgb(236, 120, 44, 0.5));
        // Labels: original text color, no halo.
        assert_eq!(theme.labels.text, srgb(216, 211, 199, 0.95));
        assert_eq!(theme.labels.halo, [0.0; 4]);
        // Terrain: identical to the pre-theme `TerrainStyle::default()`.
        assert_eq!(theme.terrain, TerrainStyle::default());
        assert_eq!(theme.terrain.shadow_tint, tint_from_srgb8(0x1a, 0x12, 0x0c));
        assert_eq!(theme.terrain.light_tint, tint_from_srgb8(0x8c, 0x84, 0x78));
        assert_eq!(theme.terrain.opacity, 0.5);
    }

    /// Regression guard: the oldworld theme (the default) keeps its exact
    /// palette — a representative sample of every section, byte-exact
    /// against the authoring expressions. The airspace/symbol/weather/label
    /// colors are the former "Pastel Dark" palette; the basemap was retuned
    /// to a neutral dark grayscale (no greens/browns, low internal
    /// contrast) so the pastel overlays dominate.
    #[test]
    fn oldworld_matches_the_pinned_palette() {
        let theme = MapTheme::oldworld();
        assert_eq!(theme.id, "oldworld");
        assert_eq!(theme.name, "Oldworld");
        // Basemap (display-space palette, premultiplied) — near-black
        // neutral grayscale with a hard-compressed road ramp.
        assert_eq!(theme.basemap.land, srgb8(0x14, 0x14, 0x15));
        assert_eq!(theme.basemap.water, srgb8(0x10, 0x11, 0x14));
        assert_eq!(theme.basemap.waterway, srgb8(0x1e, 0x21, 0x26));
        assert_eq!(theme.basemap.road_highway, srgb8(0x22, 0x22, 0x23));
        assert_eq!(theme.basemap.rail, srgb8_a(0x1c, 0x1c, 0x1f, 0.85));
        assert_eq!(
            theme.basemap.boundary_country,
            srgb8_a(0x5f, 0x5f, 0x64, 0.55)
        );
        assert_eq!(theme.basemap.place_label, srgb8(0x56, 0x56, 0x5a));
        assert_eq!(theme.clear_color, srgb8(0x14, 0x14, 0x15));
        // Airspace (premultiplied linear; pastel base hues — dusty rose CTR,
        // steel-blue controlled, terracotta danger, sand glider).
        let ctr = theme.airspace.colors(AirspaceStyleKey::Ctr);
        assert_eq!(ctr.fill, srgb(198, 116, 120, 0.09));
        assert_eq!(ctr.border, srgb(198, 116, 120, 0.8));
        let c = theme
            .airspace
            .colors(AirspaceStyleKey::IcaoClass(IcaoClass::C));
        assert_eq!(c.fill, srgb(122, 144, 178, 0.07));
        assert_eq!(c.border, srgb(122, 144, 178, 0.78));
        let danger = theme.airspace.colors(AirspaceStyleKey::Danger);
        assert_eq!(danger.border, srgb(200, 124, 98, 0.7));
        let glider = theme.airspace.colors(AirspaceStyleKey::GliderSector);
        assert_eq!(glider.border, srgb(205, 178, 124, 0.75));
        // Symbols.
        assert_eq!(theme.symbols.airport, srgb(200, 196, 188, 1.0));
        assert_eq!(theme.symbols.obstacle, srgb(198, 130, 122, 1.0));
        assert_eq!(theme.symbols.weather_dot, [1.0, 1.0, 1.0, 1.0]);
        // Weather (pastelized but semantically intact).
        assert_eq!(theme.weather.vfr, srgb(96, 188, 138, 1.0));
        assert_eq!(theme.weather.mvfr, srgb(112, 150, 218, 1.0));
        assert_eq!(theme.weather.ifr, srgb(214, 108, 112, 1.0));
        assert_eq!(theme.weather.lifr, srgb(198, 122, 198, 1.0));
        assert_eq!(theme.weather.sigmet, srgb(218, 142, 92, 0.45));
        // Labels: soft warm grey, no halo.
        assert_eq!(theme.labels.text, srgb(208, 204, 196, 0.95));
        assert_eq!(theme.labels.halo, [0.0; 4]);
        // Terrain: slightly flatter relief than high-contrast.
        assert_eq!(theme.terrain.shadow_tint, tint_from_srgb8(0x19, 0x14, 0x12));
        assert_eq!(theme.terrain.light_tint, tint_from_srgb8(0x84, 0x7e, 0x76));
        assert_eq!(theme.terrain.opacity, 0.45);
    }
}

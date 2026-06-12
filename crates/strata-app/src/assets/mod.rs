//! Embedded assets (icons + themes) and the typed [`IconName`] enum.
//!
//! gpui's `Application::with_assets` *replaces* the asset source — it does
//! not chain — so [`Assets`] is the single installed source and falls back
//! to `gpui_component_assets::Assets` for the icons gpui-component widgets
//! request internally (chevrons, close buttons, …) that we don't re-bundle.

use gpui::{AssetSource, SharedString};
use gpui_component::IconNamed;
use rust_embed::RustEmbed;

/// Application asset bundle: lucide icons copied into `assets/icons/`.
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;

/// Theme JSON bundle (`assets/themes/*.json`), loaded into the
/// `ThemeRegistry` at startup.
#[derive(RustEmbed)]
#[folder = "../../assets/themes"]
#[include = "*.json"]
pub struct ThemeAssets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        if let Some(file) = Self::get(path) {
            return Ok(Some(file.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
        entries.extend(gpui_component_assets::Assets.list(path)?);
        entries.sort_unstable();
        entries.dedup();
        Ok(entries)
    }
}

/// Typed handle for every icon shipped in `assets/icons/`.
///
/// Maps to `icons/<kebab-case>.svg` via [`IconNamed`]. Adding an icon =
/// copy the lucide svg + add the variant. No string icon paths in UI code.
#[derive(strum::Display, strum::EnumIter, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[strum(serialize_all = "kebab-case")]
pub enum IconName {
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronsUpDown,
    CircleAlert,
    Cloud,
    CloudLightning,
    CloudRain,
    Cloudy,
    Copy,
    Crosshair,
    Database,
    Download,
    Eye,
    EyeOff,
    FileText,
    Globe,
    Info,
    Layers,
    LoaderCircle,
    LocateFixed,
    Map,
    Minus,
    Moon,
    Mountain,
    Navigation,
    OctagonAlert,
    Palette,
    Plane,
    Plus,
    RadioTower,
    RefreshCw,
    Search,
    Settings,
    Snowflake,
    Sun,
    TowerControl,
    TriangleAlert,
    Waypoints,
    Wind,
    X,
    ZoomIn,
    ZoomOut,
}

impl IconNamed for IconName {
    fn path(self) -> SharedString {
        SharedString::new(format!("icons/{self}.svg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator as _;

    #[test]
    fn every_icon_variant_has_an_embedded_svg() {
        for icon in IconName::iter() {
            let path = icon.path();
            assert!(
                Assets::get(path.as_ref()).is_some(),
                "missing embedded svg for IconName::{icon:?} at {path}"
            );
        }
    }

    #[test]
    fn every_embedded_svg_has_an_icon_variant() {
        let known: Vec<String> = IconName::iter().map(|i| i.path().to_string()).collect();
        for embedded in Assets::iter() {
            assert!(
                known.contains(&embedded.to_string()),
                "embedded {embedded} has no IconName variant"
            );
        }
    }

    #[test]
    fn theme_assets_contain_oldworld() {
        assert!(
            ThemeAssets::get("oldworld.json").is_some(),
            "assets/themes/oldworld.json not embedded"
        );
    }

    #[test]
    fn theme_assets_contain_pastel_light() {
        assert!(
            ThemeAssets::get("pastel-light.json").is_some(),
            "assets/themes/pastel-light.json not embedded"
        );
    }

    /// Every embedded theme file must parse as a gpui-component `ThemeSet`
    /// with at least one theme — the exact shape `load_embedded_themes`
    /// registers at startup (a broken file would only warn there).
    #[test]
    fn every_embedded_theme_parses_as_a_theme_set() {
        let mut names = Vec::new();
        for path in ThemeAssets::iter() {
            let file = ThemeAssets::get(path.as_ref()).expect("embedded file");
            let body = std::str::from_utf8(file.data.as_ref())
                .unwrap_or_else(|err| panic!("{path}: not UTF-8: {err}"));
            let set: gpui_component::ThemeSet = serde_json::from_str(body)
                .unwrap_or_else(|err| panic!("{path}: not a ThemeSet: {err}"));
            assert!(!set.themes.is_empty(), "{path} contains no themes");
            names.extend(set.themes.iter().map(|t| t.name.to_string()));
        }
        // The shipped catalog (gpui-component themes + our two) registers
        // the configured defaults plus a healthy selection.
        assert!(names.iter().any(|n| n == "Oldworld"));
        assert!(names.iter().any(|n| n == "Pastel Light"));
        assert!(names.iter().any(|n| n == "Catppuccin Latte"));
        assert!(
            names.len() > 20,
            "expected the full theme catalog, got {names:?}"
        );
    }
}

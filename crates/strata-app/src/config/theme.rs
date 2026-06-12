//! Theme-related config value types.

use serde::{Deserialize, Serialize};

/// Map theme used for dark mode when [`MapTheme::Auto`] finds no built-in
/// map theme named after the active UI theme.
pub const DEFAULT_DARK_MAP_THEME: &str = "oldworld";
/// Map theme used for light mode when [`MapTheme::Auto`] finds no built-in
/// map theme named after the active UI theme.
pub const DEFAULT_LIGHT_MAP_THEME: &str = "pastel-light";

/// Overall UI appearance. Serialized lowercase: `mode = "dark"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }
}

/// Map (renderer) theme selection.
///
/// Serialized as a plain string: `map_theme = "auto"` follows the active UI
/// theme by name (with [`DEFAULT_DARK_MAP_THEME`] /
/// [`DEFAULT_LIGHT_MAP_THEME`] as the by-mode fallback); any other string
/// is a named map theme (`map_theme = "oldworld"`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum MapTheme {
    #[default]
    Auto,
    Named(String),
}

impl MapTheme {
    /// The concrete map-theme id/name for the given UI mode and active UI
    /// theme name (the configured `ui_theme_dark` / `ui_theme_light` slot
    /// for that mode).
    ///
    /// `Auto` follows the UI theme: it picks the built-in map theme whose
    /// *name* equals the UI theme's name (every UI theme has a same-named
    /// map sibling, so map and chrome read as one design), falling back to
    /// the mode default when none matches. An explicit `Named` selection
    /// always wins.
    pub fn resolved(&self, mode: ThemeMode, ui_theme_name: &str) -> &str {
        match self {
            MapTheme::Auto => match strata_render::MapTheme::by_name(ui_theme_name) {
                Some(map_theme) => map_theme.id,
                None if mode.is_dark() => DEFAULT_DARK_MAP_THEME,
                None => DEFAULT_LIGHT_MAP_THEME,
            },
            MapTheme::Named(name) => name,
        }
    }
}

impl From<String> for MapTheme {
    fn from(value: String) -> Self {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            MapTheme::Auto
        } else {
            MapTheme::Named(trimmed.to_owned())
        }
    }
}

impl From<MapTheme> for String {
    fn from(value: MapTheme) -> Self {
        match value {
            MapTheme::Auto => "auto".to_owned(),
            MapTheme::Named(name) => name,
        }
    }
}

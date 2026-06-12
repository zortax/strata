//! App configuration, persisted as TOML.
//!
//! Location: `dirs::config_dir()/strata/config.toml` (Linux:
//! `~/.config/strata/config.toml`), overridable via the `STRATA_CONFIG`
//! environment variable (points at the *file*, not its directory — used by
//! tests and scripting).
//!
//! Robustness contract:
//! - missing file → [`Config::default`] (every field has a serde default,
//!   so partial files are fine too);
//! - unknown keys are tolerated (forward compatibility);
//! - unparsable file → defaults + `tracing::warn!`, the broken file is left
//!   untouched on disk;
//! - out-of-range numeric values are clamped on load (see the `*_RANGE`
//!   constants).
//!
//! Secrets: `openaip_api_key` is redacted in the manual [`fmt::Debug`] impl —
//! never log a `Config` any other way, and never log the key itself.

mod countries;
mod io;
mod theme;
#[cfg(test)]
mod tests;

use std::fmt;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use strata_data::domain::Country;

pub use countries::normalize_countries;
// The env-var/default-name constants are part of the module's documented
// surface for the settings modal (next phase); nothing else consumes them
// in-process yet.
#[allow(unused_imports)]
pub use io::{CONFIG_PATH_ENV, LEGACY_CONFIG_PATH_ENV};
#[allow(unused_imports)]
pub use theme::{DEFAULT_DARK_MAP_THEME, DEFAULT_LIGHT_MAP_THEME};
pub use theme::{MapTheme, ThemeMode};

/// Environment variable consulted by [`Config::openaip_api_key`] when the
/// config file carries no key. `main.rs` loads `.env` via dotenvy before
/// anything reads the config, so the pre-existing `.env` workflow keeps
/// working unchanged as the fallback.
pub const OPENAIP_API_KEY_ENV: &str = "OPENAIP_API_KEY";

/// Default gpui-component theme name for dark mode.
pub const DEFAULT_UI_THEME_DARK: &str = "Oldworld";
/// Default gpui-component theme name for light mode.
pub const DEFAULT_UI_THEME_LIGHT: &str = "Pastel Light";

/// Valid range for [`Config::basemap_detail_bias`]; values outside are
/// clamped on load.
pub const BASEMAP_DETAIL_BIAS_RANGE: RangeInclusive<f64> = -1.5..=0.5;
/// Valid range for [`IngestConfig::basemap_maxzoom`]; clamped on load.
pub const BASEMAP_MAXZOOM_RANGE: RangeInclusive<u8> = 8..=14;
/// Valid range for [`WeatherConfig::refresh_minutes`]; clamped on load.
pub const WEATHER_REFRESH_MINUTES_RANGE: RangeInclusive<u32> = 1..=60;
/// Valid range for [`ProfileDrawerConfig::height_px`]; clamped on load.
/// The lower bound is the drawer's resize minimum; the upper bound is an
/// absolute sanity cap — at runtime the drawer additionally clamps to a
/// fraction of the window height.
pub const PROFILE_DRAWER_HEIGHT_RANGE: RangeInclusive<f32> = 160.0..=1600.0;
/// Valid range for [`ProfileDrawerConfig::corridor_half_width_nm`]; clamped
/// on load (design §3.3: configurable ±2–5 NM, with headroom for hand-edited
/// configs).
pub const CORRIDOR_HALF_WIDTH_NM_RANGE: RangeInclusive<f64> = 1.0..=10.0;

/// Maximum entries kept in [`Config::recent_flights`].
pub const MAX_RECENT_FLIGHTS: usize = 8;

const DEFAULT_BASEMAP_DETAIL_BIAS: f64 = -0.5;

/// The app-wide user configuration. All fields have serde defaults; unknown
/// keys in the file are ignored.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// openAIP API key. Prefer [`Config::openaip_api_key`] over reading this
    /// field — it implements the config-then-env precedence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openaip_api_key: Option<String>,
    /// gpui-component theme applied when [`Config::mode`] is dark.
    pub ui_theme_dark: String,
    /// gpui-component theme applied when [`Config::mode`] is light.
    pub ui_theme_light: String,
    /// Dark or light UI.
    pub mode: ThemeMode,
    /// Map (renderer) theme; `Auto` follows [`Config::mode`].
    pub map_theme: MapTheme,
    /// Basemap level-of-detail bias in zoom levels; negative = less detail.
    /// Clamped to [`BASEMAP_DETAIL_BIAS_RANGE`].
    pub basemap_detail_bias: f64,
    /// Data directory override. `None` = default resolution
    /// (`$STRATA_DATA_DIR`, else `~/.local/share/strata` — owned by `state`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    /// `[countries]` section — enabled countries, stored as
    /// `enabled = ["DE", "AT"]` (ISO alpha-2 codes; see
    /// [`countries::section`] for the lenient load: unknown codes are
    /// dropped with a warning, never failing the whole file). Scopes
    /// **ingestion** (which countries' aero/basemap/terrain data are
    /// downloaded and kept current) and the live METAR/TAF fetch area —
    /// never rendering: the map stays viewport-driven over whatever the
    /// store holds. Default: Germany only; an explicit `enabled = []` is
    /// kept (nothing auto-ingested, the app still runs). Prefer
    /// [`Config::enabled_countries`] over reading this field — it
    /// normalizes (sorted, deduped).
    #[serde(with = "countries::section")]
    pub countries: Vec<Country>,
    pub ingest: IngestConfig,
    pub weather: WeatherConfig,
    /// `[profile_drawer]` section: planning-mode profile drawer chrome.
    pub profile_drawer: ProfileDrawerConfig,
    /// `[autorouter]` section: credentials for the NOTAM provider. Empty
    /// (the default) means no provider — the Briefing tab asks for them;
    /// see `state::briefing::build_notam_provider`.
    pub autorouter: AutorouterConfig,
    /// `[pilot]` section: pilot/operator data for ICAO FPL item 19
    /// (design §4 "pilot data from settings").
    pub pilot: strata_plan::fpl::PilotInfo,
    /// Recently opened flight files, most recent first (capped at
    /// [`MAX_RECENT_FLIGHTS`]). Maintained via [`Config::note_recent_flight`];
    /// entries whose file disappeared are dropped lazily by the consumer
    /// (the flight library/menu), not on load.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_flights: Vec<PathBuf>,
}

/// `[ingest]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    /// Run ingest automatically when data is missing or stale.
    pub auto: bool,
    /// Maximum basemap tile zoom to ingest. Clamped to
    /// [`BASEMAP_MAXZOOM_RANGE`].
    pub basemap_maxzoom: u8,
}

/// `[weather]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherConfig {
    /// Live-weather refresh period in minutes. Clamped to
    /// [`WEATHER_REFRESH_MINUTES_RANGE`].
    pub refresh_minutes: u32,
}

/// `[autorouter]` section — the autorouter.aero account credentials, as
/// documented at <https://www.autorouter.aero/wiki/api/authentication/>:
/// the account **email** and **password** are sent as the OAuth2
/// `client_id`/`client_secret` (see `strata_data::providers::autorouter`).
/// Both unset (the default) means the app has no NOTAM provider — the
/// Briefing tab shows its not-configured state.
///
/// **Stored in plain text** in `config.toml` (the settings UI says so
/// honestly). Secrets: redacted in `Debug` like the openAIP key — never
/// log either field.
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutorouterConfig {
    /// autorouter.aero account email (the OAuth2 `client_id`). The alias
    /// accepts the field's pre-release name.
    #[serde(alias = "client_id", skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// autorouter.aero account password (the OAuth2 `client_secret`),
    /// plain text. The alias accepts the field's pre-release name.
    #[serde(alias = "client_secret", skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl AutorouterConfig {
    /// `(email, password)`, when both are set and non-blank.
    pub fn credentials(&self) -> Option<(String, String)> {
        Some((
            non_blank(self.email.clone())?,
            non_blank(self.password.clone())?,
        ))
    }
}

// Manual impl: credentials must never reach logs.
impl fmt::Debug for AutorouterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutorouterConfig")
            .field("email", &self.email.as_ref().map(|_| Redacted))
            .field("password", &self.password.as_ref().map(|_| Redacted))
            .finish()
    }
}

/// `[profile_drawer]` section.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileDrawerConfig {
    /// Expanded drawer height in logical pixels (the drag-resize result,
    /// persisted across runs). Clamped to [`PROFILE_DRAWER_HEIGHT_RANGE`];
    /// non-finite values fall back to the default.
    pub height_px: f32,
    /// Corridor half-width the profile is computed over, in nautical miles
    /// (the drawer header's 2/3/5 NM select; design §3.3). Clamped to
    /// [`CORRIDOR_HALF_WIDTH_NM_RANGE`]; non-finite values fall back to the
    /// default (the planning core's 5 NM).
    pub corridor_half_width_nm: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            openaip_api_key: None,
            ui_theme_dark: DEFAULT_UI_THEME_DARK.to_owned(),
            ui_theme_light: DEFAULT_UI_THEME_LIGHT.to_owned(),
            mode: ThemeMode::Dark,
            map_theme: MapTheme::Auto,
            basemap_detail_bias: DEFAULT_BASEMAP_DETAIL_BIAS,
            data_dir: None,
            countries: Country::DEFAULT_ENABLED.to_vec(),
            ingest: IngestConfig::default(),
            weather: WeatherConfig::default(),
            profile_drawer: ProfileDrawerConfig::default(),
            autorouter: AutorouterConfig::default(),
            pilot: strata_plan::fpl::PilotInfo::default(),
            recent_flights: Vec::new(),
        }
    }
}

impl Default for ProfileDrawerConfig {
    fn default() -> Self {
        Self {
            height_px: crate::ui::profile_drawer::DEFAULT_EXPANDED_HEIGHT_PX,
            // The planning core's corridor default (5 NM half-width) — the
            // two defaults agreeing keeps "untouched config" and "untouched
            // ComputeParams" the same flight.
            corridor_half_width_nm: strata_plan::units::NauticalMiles::from_meters(
                strata_plan::corridor::CorridorParams::default().half_width,
            )
            .0,
        }
    }
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            auto: true,
            basemap_maxzoom: 13,
        }
    }
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self { refresh_minutes: 5 }
    }
}

impl Config {
    /// The effective openAIP API key: the config value wins; otherwise the
    /// `OPENAIP_API_KEY` environment variable (which dotenvy populates from
    /// `.env` in `main.rs`, so the `.env` path remains a working fallback).
    /// Empty / whitespace-only values count as unset.
    pub fn openaip_api_key(&self) -> Option<String> {
        self.openaip_api_key_with_env(std::env::var(OPENAIP_API_KEY_ENV).ok())
    }

    /// Pure core of [`Config::openaip_api_key`], testable without touching
    /// process environment.
    fn openaip_api_key_with_env(&self, env_value: Option<String>) -> Option<String> {
        non_blank(self.openaip_api_key.clone()).or_else(|| non_blank(env_value))
    }

    /// The effective enabled-country set: [`Config::countries`] normalized
    /// via [`normalize_countries`] (sorted, deduped). May be **empty** —
    /// the user can disable every country; nothing is auto-ingested then
    /// and the app runs on whatever data is present.
    pub fn enabled_countries(&self) -> Vec<Country> {
        normalize_countries(self.countries.clone())
    }

    /// Clamp every bounded numeric field into its documented range (and
    /// replace a non-finite detail bias with the default). Called by the
    /// loaders; also useful before persisting values coming from UI input.
    pub fn normalized(mut self) -> Self {
        self.basemap_detail_bias = if self.basemap_detail_bias.is_finite() {
            self.basemap_detail_bias.clamp(
                *BASEMAP_DETAIL_BIAS_RANGE.start(),
                *BASEMAP_DETAIL_BIAS_RANGE.end(),
            )
        } else {
            DEFAULT_BASEMAP_DETAIL_BIAS
        };
        self.ingest.basemap_maxzoom = self
            .ingest
            .basemap_maxzoom
            .clamp(*BASEMAP_MAXZOOM_RANGE.start(), *BASEMAP_MAXZOOM_RANGE.end());
        self.weather.refresh_minutes = self.weather.refresh_minutes.clamp(
            *WEATHER_REFRESH_MINUTES_RANGE.start(),
            *WEATHER_REFRESH_MINUTES_RANGE.end(),
        );
        self.profile_drawer.height_px = if self.profile_drawer.height_px.is_finite() {
            self.profile_drawer.height_px.clamp(
                *PROFILE_DRAWER_HEIGHT_RANGE.start(),
                *PROFILE_DRAWER_HEIGHT_RANGE.end(),
            )
        } else {
            ProfileDrawerConfig::default().height_px
        };
        self.profile_drawer.corridor_half_width_nm =
            if self.profile_drawer.corridor_half_width_nm.is_finite() {
                self.profile_drawer.corridor_half_width_nm.clamp(
                    *CORRIDOR_HALF_WIDTH_NM_RANGE.start(),
                    *CORRIDOR_HALF_WIDTH_NM_RANGE.end(),
                )
            } else {
                ProfileDrawerConfig::default().corridor_half_width_nm
            };
        self.countries = normalize_countries(std::mem::take(&mut self.countries));
        self.recent_flights.truncate(MAX_RECENT_FLIGHTS);
        self
    }

    /// Moves `path` to the front of the recent-flights list (deduplicated,
    /// capped at [`MAX_RECENT_FLIGHTS`]). Returns whether the list changed —
    /// callers persist via [`Config::save_if_changed`] only then.
    pub fn note_recent_flight(&mut self, path: &std::path::Path) -> bool {
        if self.recent_flights.first().is_some_and(|p| p == path) {
            return false;
        }
        self.recent_flights.retain(|p| p != path);
        self.recent_flights.insert(0, path.to_path_buf());
        self.recent_flights.truncate(MAX_RECENT_FLIGHTS);
        true
    }

    /// Removes `path` from the recent-flights list (the library prunes
    /// entries whose file disappeared). Returns whether the list changed.
    // Consumed by the flight library/menu phase.
    #[allow(dead_code)]
    pub fn forget_recent_flight(&mut self, path: &std::path::Path) -> bool {
        let before = self.recent_flights.len();
        self.recent_flights.retain(|p| p != path);
        self.recent_flights.len() != before
    }
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// Placeholder printed instead of the API key.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"***redacted***\"")
    }
}

// Manual impl: `openaip_api_key` must never reach logs. Keep every field
// listed here in sync with the struct.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field(
                "openaip_api_key",
                &self.openaip_api_key.as_ref().map(|_| Redacted),
            )
            .field("ui_theme_dark", &self.ui_theme_dark)
            .field("ui_theme_light", &self.ui_theme_light)
            .field("mode", &self.mode)
            .field("map_theme", &self.map_theme)
            .field("basemap_detail_bias", &self.basemap_detail_bias)
            .field("data_dir", &self.data_dir)
            .field("countries", &self.countries)
            .field("ingest", &self.ingest)
            .field("weather", &self.weather)
            .field("profile_drawer", &self.profile_drawer)
            .field("autorouter", &self.autorouter)
            .field("pilot", &self.pilot)
            .field("recent_flights", &self.recent_flights)
            .finish()
    }
}

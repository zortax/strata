//! Config file location, loading and atomic saving.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use strata_data::paths;

use crate::fsutil::{self, WriteOutcome, WriteTicket};

use super::Config;

/// Environment variable that overrides the config *file* path (tests,
/// scripting). Unset → `dirs::config_dir()/strata/config.toml`.
pub const CONFIG_PATH_ENV: &str = "STRATA_CONFIG";
/// Deprecated pre-rename variant of [`CONFIG_PATH_ENV`], honored with a
/// warning (assembled so the old name stays out of the repo).
pub const LEGACY_CONFIG_PATH_ENV: &str = concat!("GA", "MAP_CONFIG");

impl Config {
    /// The config file path: `$STRATA_CONFIG` (the pre-rename variable
    /// still works, with a deprecation warning), else
    /// `dirs::config_dir()/strata/config.toml`.
    pub fn path() -> PathBuf {
        if let Some(path) = paths::env_var_with_legacy(CONFIG_PATH_ENV, LEGACY_CONFIG_PATH_ENV) {
            return PathBuf::from(path);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(paths::DIR_NAME)
            .join("config.toml")
    }

    /// Load from [`Config::path`]. Never fails: missing file → defaults;
    /// unreadable or unparsable file → defaults + warning, file untouched.
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// [`Config::load`] from an explicit path. Out-of-range values are
    /// clamped (see [`Config::normalized`]).
    pub fn load_from(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Config::default();
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to read config file; using defaults"
                );
                return Config::default();
            }
        };
        match toml::from_str::<Config>(&text) {
            Ok(config) => config.normalized(),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "config file is not valid TOML; using defaults (file left untouched)"
                );
                Config::default()
            }
        }
    }

    /// Save to [`Config::path`] as pretty TOML. Atomic: written to a sibling
    /// temp file, fsynced, then renamed over the target. Parent directories
    /// are created as needed.
    ///
    /// (Unconditional save — for the settings modal; in-app writers so far
    /// use the diff-aware [`Config::save_if_changed`].)
    #[allow(dead_code)]
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path())
    }

    /// [`Config::save`] to an explicit path.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        fsutil::write_atomic(path, &self.to_toml()?)
    }

    /// Diff-aware save to [`Config::path`]: writes only when the rendered
    /// TOML differs from what is on disk (missing/unreadable/garbled file
    /// counts as different). Returns whether a write happened. The
    /// ordering ticket is captured at call time; detached background
    /// savers go through [`Config::save_if_changed_ordered`] with a ticket
    /// captured alongside their config snapshot (in-app writers all funnel
    /// through `AppState::persist_config` now, hence the allow).
    #[allow(dead_code)]
    pub fn save_if_changed(&self) -> anyhow::Result<bool> {
        self.save_to_if_changed(&Self::path())
    }

    /// [`Config::save_if_changed`] with a caller-captured [`WriteTicket`]:
    /// an older config snapshot completing late can never overwrite a
    /// newer one.
    pub fn save_if_changed_ordered(&self, ticket: WriteTicket) -> anyhow::Result<bool> {
        self.save_to_if_changed_ordered(&Self::path(), ticket)
    }

    /// [`Config::save_if_changed`] against an explicit path.
    pub fn save_to_if_changed(&self, path: &Path) -> anyhow::Result<bool> {
        self.save_to_if_changed_ordered(path, WriteTicket::next())
    }

    /// [`Config::save_if_changed_ordered`] against an explicit path.
    pub fn save_to_if_changed_ordered(
        &self,
        path: &Path,
        ticket: WriteTicket,
    ) -> anyhow::Result<bool> {
        let text = self.to_toml()?;
        if fs::read_to_string(path).is_ok_and(|on_disk| on_disk == text) {
            // Content already on disk: still claim the ticket, so an older
            // in-flight snapshot cannot rename stale content over it.
            fsutil::mark_committed(path, ticket);
            return Ok(false);
        }
        let outcome = fsutil::write_atomic_ordered(path, &text, ticket)?;
        Ok(outcome == WriteOutcome::Committed)
    }

    fn to_toml(&self) -> anyhow::Result<String> {
        toml::to_string_pretty(self).context("serialize config to TOML")
    }
}

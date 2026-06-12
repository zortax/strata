//! Per-user filesystem names shared by the app and the ingest CLI, plus a
//! one-shot migration from the project's pre-rename directories and
//! environment variables.
//!
//! The legacy identifiers are assembled with `concat!` on purpose: the old
//! project name must not appear verbatim anywhere in this repository, but
//! existing installations still use it on disk and in shells.

use std::ffi::OsString;
use std::path::Path;

/// Directory name under the XDG base dirs (`~/.local/share/<DIR_NAME>`,
/// `~/.config/<DIR_NAME>`, …).
pub const DIR_NAME: &str = "strata";
/// Pre-rename directory name, still found in existing installations.
pub const LEGACY_DIR_NAME: &str = concat!("ga", "map");

/// Environment variable overriding the data directory.
pub const DATA_DIR_ENV: &str = "STRATA_DATA_DIR";
/// Deprecated pre-rename data-dir variable, honored with a warning.
pub const LEGACY_DATA_DIR_ENV: &str = concat!("GA", "MAP_DATA_DIR");

/// Vector basemap archive file name inside the data dir. One shared
/// archive for all countries — per-country extracts merge into it (tiles
/// are globally addressed z/x/y).
pub const BASEMAP_FILE: &str = "basemap.mbtiles";
/// Pre-multi-country basemap file name (the archive was Germany-only and
/// carried the country in its name). Migrated by
/// [`migrate_legacy_basemap`].
pub const LEGACY_BASEMAP_FILE: &str = "basemap-de.mbtiles";

/// One-shot rename of the pre-multi-country basemap archive
/// (`basemap-de.mbtiles` → `basemap.mbtiles`) inside `data_dir` — same
/// semantics as [`migrate_legacy_dir`]: only when the old file exists and
/// the new one does not; failures are logged and leave the old file in
/// place. Returns whether a migration happened.
///
/// SQLite sidecars (`-wal`, `-shm`) are renamed along with the database:
/// SQLite associates them by file name, so leaving a hot `-wal` behind
/// would silently drop its committed-but-uncheckpointed tiles.
pub fn migrate_legacy_basemap(data_dir: &Path) -> bool {
    let old = data_dir.join(LEGACY_BASEMAP_FILE);
    let new = data_dir.join(BASEMAP_FILE);
    if !migrate_legacy_dir(&old, &new) {
        return false;
    }
    for suffix in ["-wal", "-shm"] {
        let old_sidecar = sidecar(&old, suffix);
        if old_sidecar.exists() {
            migrate_legacy_dir(&old_sidecar, &sidecar(&new, suffix));
        }
    }
    true
}

/// `path` with `suffix` appended to its file name (`db` → `db-wal`).
fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Reads `primary` from the process environment, falling back to the
/// deprecated `legacy` variable (with a `tracing::warn`) when only that one
/// is set. A set-but-empty `primary` still wins — callers keep their own
/// empty-value semantics.
pub fn env_var_with_legacy(primary: &str, legacy: &str) -> Option<OsString> {
    pick_env(
        primary,
        std::env::var_os(primary),
        legacy,
        std::env::var_os(legacy),
    )
}

/// Pure core of [`env_var_with_legacy`].
fn pick_env(
    primary_name: &str,
    primary: Option<OsString>,
    legacy_name: &str,
    legacy: Option<OsString>,
) -> Option<OsString> {
    if primary.is_some() {
        return primary;
    }
    let value = legacy?;
    tracing::warn!(
        deprecated = legacy_name,
        replacement = primary_name,
        "deprecated environment variable is set; please rename it"
    );
    Some(value)
}

/// One-shot path migration (works for directories and plain files):
/// renames `old` to `new` when `old` exists and `new` does not.
/// `std::fs::rename` only — same filesystem, instant, no copying; `new`'s
/// parent must already exist (true for the XDG base dirs this is used
/// with). Failures are logged and leave `old` untouched. Returns whether
/// a migration happened.
pub fn migrate_legacy_dir(old: &Path, new: &Path) -> bool {
    if new.exists() || !old.exists() {
        return false;
    }
    match std::fs::rename(old, new) {
        Ok(()) => {
            tracing::info!(
                from = %old.display(),
                to = %new.display(),
                "migrated legacy directory to its new location"
            );
            true
        }
        Err(error) => {
            tracing::warn!(
                from = %old.display(),
                to = %new.display(),
                %error,
                "failed to migrate legacy directory; leaving it in place"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn legacy_names_spell_the_old_project_name() {
        // The repo must not contain the old name verbatim, but the runtime
        // values must still match what existing installations use.
        assert_eq!(LEGACY_DIR_NAME.len(), 5);
        assert!(LEGACY_DIR_NAME.starts_with("ga"));
        assert!(LEGACY_DIR_NAME.ends_with("map"));
        assert_eq!(
            LEGACY_DATA_DIR_ENV,
            format!("{}_DATA_DIR", LEGACY_DIR_NAME.to_uppercase())
        );
    }

    #[test]
    fn pick_env_prefers_primary_even_when_empty() {
        let primary = Some(OsString::from(""));
        let legacy = Some(OsString::from("/legacy"));
        assert_eq!(
            pick_env("NEW", primary.clone(), "OLD", legacy),
            primary
        );
    }

    #[test]
    fn pick_env_falls_back_to_legacy() {
        let legacy = Some(OsString::from("/legacy"));
        assert_eq!(pick_env("NEW", None, "OLD", legacy.clone()), legacy);
        assert_eq!(pick_env("NEW", None, "OLD", None), None);
    }

    #[test]
    fn migrates_old_dir_when_new_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(LEGACY_DIR_NAME);
        let new = tmp.path().join(DIR_NAME);
        fs::create_dir(&old).unwrap();
        fs::write(old.join("store.sqlite"), b"data").unwrap();

        assert!(migrate_legacy_dir(&old, &new));
        assert!(!old.exists());
        assert_eq!(fs::read(new.join("store.sqlite")).unwrap(), b"data");
    }

    #[test]
    fn migration_is_a_noop_without_an_old_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(LEGACY_DIR_NAME);
        let new = tmp.path().join(DIR_NAME);
        assert!(!migrate_legacy_dir(&old, &new));
        assert!(!new.exists());
    }

    #[test]
    fn basemap_file_migrates_once_and_never_clobbers() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(LEGACY_BASEMAP_FILE);
        let new = tmp.path().join(BASEMAP_FILE);

        // Nothing to migrate.
        assert!(!migrate_legacy_basemap(tmp.path()));

        // Legacy archive present → renamed losslessly.
        fs::write(&old, b"tiles").unwrap();
        assert!(migrate_legacy_basemap(tmp.path()));
        assert!(!old.exists());
        assert_eq!(fs::read(&new).unwrap(), b"tiles");

        // Second call is a no-op; an existing new archive is never
        // clobbered even if a stray legacy file reappears.
        assert!(!migrate_legacy_basemap(tmp.path()));
        fs::write(&old, b"stray").unwrap();
        assert!(!migrate_legacy_basemap(tmp.path()));
        assert_eq!(fs::read(&new).unwrap(), b"tiles");
    }

    /// SQLite sidecars must follow the database file — a left-behind hot
    /// `-wal` would silently lose its committed tiles.
    #[test]
    fn basemap_migration_carries_wal_and_shm_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(LEGACY_BASEMAP_FILE);
        fs::write(&old, b"db").unwrap();
        fs::write(sidecar(&old, "-wal"), b"wal").unwrap();
        fs::write(sidecar(&old, "-shm"), b"shm").unwrap();

        assert!(migrate_legacy_basemap(tmp.path()));

        let new = tmp.path().join(BASEMAP_FILE);
        assert_eq!(fs::read(&new).unwrap(), b"db");
        assert_eq!(fs::read(sidecar(&new, "-wal")).unwrap(), b"wal");
        assert_eq!(fs::read(sidecar(&new, "-shm")).unwrap(), b"shm");
        assert!(!old.exists());
        assert!(!sidecar(&old, "-wal").exists());
        assert!(!sidecar(&old, "-shm").exists());
    }

    #[test]
    fn migration_never_clobbers_an_existing_new_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(LEGACY_DIR_NAME);
        let new = tmp.path().join(DIR_NAME);
        fs::create_dir(&old).unwrap();
        fs::write(old.join("marker"), b"old").unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(new.join("marker"), b"new").unwrap();

        assert!(!migrate_legacy_dir(&old, &new));
        assert_eq!(fs::read(old.join("marker")).unwrap(), b"old");
        assert_eq!(fs::read(new.join("marker")).unwrap(), b"new");
    }
}

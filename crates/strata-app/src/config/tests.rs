use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use super::*;

/// Serializes every test that mutates process environment variables
/// (`STRATA_CONFIG`, `OPENAIP_API_KEY`) — tests run in parallel threads.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn non_default_config() -> Config {
    Config {
        openaip_api_key: Some("test-key-123".into()),
        ui_theme_dark: "Midnight".into(),
        ui_theme_light: "Daylight".into(),
        mode: ThemeMode::Light,
        map_theme: MapTheme::Named("oldworld".into()),
        basemap_detail_bias: 0.25,
        data_dir: Some(PathBuf::from("/tmp/strata-data-elsewhere")),
        // Already in canonical (enum-declaration) order — `normalized()`
        // must be able to leave this config untouched.
        countries: vec![Country::DE, Country::AT],
        ingest: IngestConfig {
            auto: false,
            basemap_maxzoom: 11,
        },
        weather: WeatherConfig {
            refresh_minutes: 15,
        },
        profile_drawer: ProfileDrawerConfig {
            height_px: 320.0,
            corridor_half_width_nm: 3.0,
        },
        autorouter: AutorouterConfig {
            email: Some("pilot@example.com".into()),
            password: Some("hunter2".into()),
        },
        pilot: strata_plan::fpl::PilotInfo {
            pilot_in_command: "Test Pilot".into(),
            persons_on_board: Some(2),
            aircraft_color: Some("white blue".into()),
        },
        recent_flights: vec![PathBuf::from("/tmp/flights/a.strata-flight")],
    }
}

// --- defaults ---------------------------------------------------------

#[test]
fn defaults_match_spec() {
    let config = Config::default();
    assert_eq!(config.openaip_api_key, None);
    assert_eq!(config.ui_theme_dark, "Oldworld");
    assert_eq!(config.ui_theme_light, "Pastel Light");
    assert_eq!(config.mode, ThemeMode::Dark);
    assert_eq!(config.map_theme, MapTheme::Auto);
    assert_eq!(config.basemap_detail_bias, -0.5);
    assert_eq!(config.data_dir, None);
    // Germany enabled by default; everything else off.
    assert_eq!(config.countries, vec![Country::DE]);
    assert!(config.ingest.auto);
    assert_eq!(config.ingest.basemap_maxzoom, 13);
    assert_eq!(config.weather.refresh_minutes, 5);
    assert_eq!(
        config.profile_drawer.height_px,
        crate::ui::profile_drawer::DEFAULT_EXPANDED_HEIGHT_PX
    );
    // The corridor width default mirrors the planning core's 5 NM.
    assert_eq!(config.profile_drawer.corridor_half_width_nm, 5.0);
    assert!(config.recent_flights.is_empty());
}

// --- profile drawer ------------------------------------------------------

/// The drawer height clamps into its documented range on load; junk
/// values fall back to the default instead of breaking the drawer.
#[test]
fn profile_drawer_height_is_clamped_on_normalize() {
    let mut config = Config::default();
    config.profile_drawer.height_px = 40.0;
    assert_eq!(
        config.normalized().profile_drawer.height_px,
        *PROFILE_DRAWER_HEIGHT_RANGE.start()
    );

    let mut config = Config::default();
    config.profile_drawer.height_px = 99_999.0;
    assert_eq!(
        config.normalized().profile_drawer.height_px,
        *PROFILE_DRAWER_HEIGHT_RANGE.end()
    );

    let mut config = Config::default();
    config.profile_drawer.height_px = f32::NAN;
    assert_eq!(
        config.normalized().profile_drawer.height_px,
        ProfileDrawerConfig::default().height_px
    );

    // In-range values survive untouched.
    let mut config = Config::default();
    config.profile_drawer.height_px = 333.0;
    assert_eq!(config.normalized().profile_drawer.height_px, 333.0);
}

/// The corridor half-width clamps like the height: documented range on
/// load, junk values fall back to the default.
#[test]
fn corridor_half_width_is_clamped_on_normalize() {
    let clamp = |nm: f64| {
        let mut config = Config::default();
        config.profile_drawer.corridor_half_width_nm = nm;
        config.normalized().profile_drawer.corridor_half_width_nm
    };
    assert_eq!(clamp(0.1), *CORRIDOR_HALF_WIDTH_NM_RANGE.start());
    assert_eq!(clamp(99.0), *CORRIDOR_HALF_WIDTH_NM_RANGE.end());
    assert_eq!(
        clamp(f64::NAN),
        ProfileDrawerConfig::default().corridor_half_width_nm
    );
    // The select's choices survive untouched.
    for nm in [2.0, 3.0, 5.0] {
        assert_eq!(clamp(nm), nm);
    }
}

// --- recent flights ----------------------------------------------------

#[test]
fn note_recent_flight_fronts_dedupes_and_caps() {
    let mut config = Config::default();
    let path = |i: usize| PathBuf::from(format!("/flights/{i}.strata-flight"));

    assert!(config.note_recent_flight(&path(1)));
    assert!(config.note_recent_flight(&path(2)));
    assert_eq!(config.recent_flights, vec![path(2), path(1)]);

    // Re-noting the head is a no-op (no churn, no redundant config write).
    assert!(!config.note_recent_flight(&path(2)));
    assert_eq!(config.recent_flights, vec![path(2), path(1)]);

    // Re-noting an older entry moves it to the front, deduplicated.
    assert!(config.note_recent_flight(&path(1)));
    assert_eq!(config.recent_flights, vec![path(1), path(2)]);

    // The list caps at MAX_RECENT_FLIGHTS, dropping the oldest.
    for i in 3..=(MAX_RECENT_FLIGHTS + 2) {
        assert!(config.note_recent_flight(&path(i)));
    }
    assert_eq!(config.recent_flights.len(), MAX_RECENT_FLIGHTS);
    assert_eq!(config.recent_flights[0], path(MAX_RECENT_FLIGHTS + 2));
    assert!(!config.recent_flights.contains(&path(1)), "oldest dropped");
}

#[test]
fn forget_recent_flight_removes_only_matches() {
    let mut config = Config::default();
    let a = PathBuf::from("/flights/a.strata-flight");
    let b = PathBuf::from("/flights/b.strata-flight");
    config.note_recent_flight(&a);
    config.note_recent_flight(&b);

    assert!(config.forget_recent_flight(&a));
    assert_eq!(config.recent_flights, vec![b.clone()]);
    assert!(!config.forget_recent_flight(&a), "already gone");
    assert_eq!(config.recent_flights, vec![b]);
}

#[test]
fn recent_flights_round_trip_and_normalize_caps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = Config::default();
    config.note_recent_flight(&PathBuf::from("/flights/x.strata-flight"));
    config.save_to(&path).unwrap();
    assert_eq!(Config::load_from(&path), config);

    // An over-long persisted list (hand-edited file) is capped on load.
    let long = Config {
        recent_flights: (0..2 * MAX_RECENT_FLIGHTS)
            .map(|i| PathBuf::from(format!("/flights/{i}.strata-flight")))
            .collect(),
        ..Config::default()
    };
    assert_eq!(long.normalized().recent_flights.len(), MAX_RECENT_FLIGHTS);
}

#[test]
fn missing_file_loads_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = Config::load_from(&dir.path().join("does-not-exist.toml"));
    assert_eq!(loaded, Config::default());
}

#[test]
fn partial_file_fills_in_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "mode = \"light\"\n\n[weather]\nrefresh_minutes = 10\n",
    )
    .unwrap();
    let loaded = Config::load_from(&path);
    assert_eq!(loaded.mode, ThemeMode::Light);
    assert_eq!(loaded.weather.refresh_minutes, 10);
    // Everything else defaulted, including the untouched nested section.
    assert_eq!(loaded.ui_theme_dark, "Oldworld");
    assert_eq!(loaded.ingest, IngestConfig::default());
}

#[test]
fn unknown_keys_are_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "mode = \"light\"\nfuture_key = true\n\n[future_section]\nx = 1\n",
    )
    .unwrap();
    let loaded = Config::load_from(&path);
    assert_eq!(loaded.mode, ThemeMode::Light);
}

#[test]
fn garbage_file_loads_defaults_and_stays_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let garbage = "this is { not [ toml ===";
    fs::write(&path, garbage).unwrap();

    let loaded = Config::load_from(&path);
    assert_eq!(loaded, Config::default());
    assert_eq!(fs::read_to_string(&path).unwrap(), garbage);
}

// --- round trip & atomic writes ---------------------------------------

#[test]
fn save_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let config = non_default_config();
    config.save_to(&path).unwrap();
    assert_eq!(Config::load_from(&path), config);
}

#[test]
fn default_config_round_trips_with_none_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to(&path).unwrap();
    assert_eq!(Config::load_from(&path), Config::default());
    // None fields are omitted, not serialized as something weird.
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("openaip_api_key"), "{text}");
    assert!(!text.contains("data_dir"), "{text}");
}

#[test]
fn save_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    non_default_config().save_to(&path).unwrap();
    non_default_config().save_to(&path).unwrap(); // overwrite path too

    let entries: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["config.toml".to_owned()]);
}

#[test]
fn failed_save_cleans_up_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    // A directory squatting on the target path makes the final rename fail.
    let path = dir.path().join("config.toml");
    fs::create_dir(&path).unwrap();

    assert!(non_default_config().save_to(&path).is_err());

    let entries: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries,
        vec!["config.toml".to_owned()],
        "temp file left behind"
    );
}

#[test]
fn save_if_changed_is_diff_aware() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = non_default_config();

    assert!(
        config.save_to_if_changed(&path).unwrap(),
        "first save writes"
    );
    assert!(
        !config.save_to_if_changed(&path).unwrap(),
        "identical config skips the write"
    );

    let mut changed = config.clone();
    changed.weather.refresh_minutes = 30;
    assert!(changed.save_to_if_changed(&path).unwrap(), "change writes");
    assert_eq!(Config::load_from(&path), changed);
}

// --- clamping ----------------------------------------------------------

#[test]
fn out_of_range_values_are_clamped_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "basemap_detail_bias = 7.5\n\n[ingest]\nbasemap_maxzoom = 22\n\n[weather]\nrefresh_minutes = 0\n",
    )
    .unwrap();
    let loaded = Config::load_from(&path);
    assert_eq!(loaded.basemap_detail_bias, 0.5);
    assert_eq!(loaded.ingest.basemap_maxzoom, 14);
    assert_eq!(loaded.weather.refresh_minutes, 1);
}

#[test]
fn normalized_clamps_both_ends_and_nan() {
    let config = Config {
        basemap_detail_bias: -9.0,
        ingest: IngestConfig {
            basemap_maxzoom: 2,
            ..IngestConfig::default()
        },
        weather: WeatherConfig {
            refresh_minutes: 999,
        },
        ..Config::default()
    }
    .normalized();
    assert_eq!(config.basemap_detail_bias, -1.5);
    assert_eq!(config.ingest.basemap_maxzoom, 8);
    assert_eq!(config.weather.refresh_minutes, 60);

    let config = Config {
        basemap_detail_bias: f64::NAN,
        ..Config::default()
    };
    assert_eq!(config.normalized().basemap_detail_bias, -0.5);
}

#[test]
fn in_range_values_survive_normalized() {
    let config = non_default_config().normalized();
    assert_eq!(config, non_default_config());
}

// --- enabled countries ---------------------------------------------------

/// The enabled-country set normalizes to sorted/deduped; the empty set is
/// a legal state (nothing auto-ingested) and stays empty.
#[test]
fn enabled_countries_normalize_and_keep_the_empty_set() {
    assert_eq!(
        normalize_countries(vec![Country::AT, Country::DE, Country::AT]),
        vec![Country::DE, Country::AT]
    );
    assert_eq!(normalize_countries(Vec::new()), Vec::new());

    let config = Config {
        countries: vec![Country::CH, Country::CH, Country::AT],
        ..Config::default()
    };
    assert_eq!(
        config.enabled_countries(),
        vec![Country::AT, Country::CH],
        "accessor normalizes without mutating"
    );
    assert_eq!(
        config.normalized().countries,
        vec![Country::AT, Country::CH]
    );

    let empty = Config {
        countries: Vec::new(),
        ..Config::default()
    };
    assert_eq!(empty.enabled_countries(), Vec::new());
    assert_eq!(empty.normalized().countries, Vec::new());
}

/// Countries persist as a `[countries]` section with an `enabled` array of
/// ISO alpha-2 code strings; a config file from before the multi-country
/// feature (no `[countries]` section) loads as the Germany-only default —
/// the existing user store keeps working untouched.
#[test]
fn countries_round_trip_as_a_section_and_default_to_germany() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let config = Config {
        countries: vec![Country::DE, Country::AT],
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("[countries]"), "{text}");
    // Pretty TOML renders multi-element arrays one code per line.
    assert!(text.contains("enabled = ["), "{text}");
    assert!(text.contains("\"DE\""), "{text}");
    assert!(text.contains("\"AT\""), "{text}");
    assert_eq!(Config::load_from(&path), config);

    // Pre-multi-country file: no `[countries]` section → default (DE).
    fs::write(&path, "mode = \"light\"\n").unwrap();
    assert_eq!(Config::load_from(&path).countries, vec![Country::DE]);

    // A section without the `enabled` key defaults like a missing section.
    fs::write(&path, "[countries]\n").unwrap();
    assert_eq!(Config::load_from(&path).countries, vec![Country::DE]);
}

/// Unknown or malformed codes in `[countries] enabled` are dropped with a
/// warning — one typo can never invalidate the whole config file. Codes
/// parse case-insensitively and duplicates collapse.
#[test]
fn countries_unknown_codes_are_tolerated_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "mode = \"light\"\n\n[countries]\nenabled = [\"de\", \"XX\", \" at \", \"DE\", \"Atlantis\"]\n",
    )
    .unwrap();
    let loaded = Config::load_from(&path);
    assert_eq!(loaded.countries, vec![Country::DE, Country::AT]);
    // The rest of the file stayed intact (no fall-back-to-defaults).
    assert_eq!(loaded.mode, ThemeMode::Light);
}

/// An explicit `enabled = []` round-trips as the empty set — disabling
/// every country must survive a restart instead of resurrecting Germany.
#[test]
fn countries_explicit_empty_set_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config {
        countries: Vec::new(),
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("enabled = []"), "{text}");
    assert_eq!(Config::load_from(&path).countries, Vec::new());
}

// --- api key precedence ------------------------------------------------

#[test]
fn api_key_config_value_beats_env() {
    let config = non_default_config();
    assert_eq!(
        config.openaip_api_key_with_env(Some("env-key".into())),
        Some("test-key-123".into())
    );
}

#[test]
fn api_key_falls_back_to_env_when_config_unset_or_blank() {
    let mut config = Config::default();
    assert_eq!(
        config.openaip_api_key_with_env(Some("env-key".into())),
        Some("env-key".into())
    );
    config.openaip_api_key = Some("   ".into()); // blank counts as unset
    assert_eq!(
        config.openaip_api_key_with_env(Some("env-key".into())),
        Some("env-key".into())
    );
    assert_eq!(config.openaip_api_key_with_env(None), None);
}

#[test]
fn api_key_reads_process_env() {
    let _guard = env_guard();
    // SAFETY: single-threaded with respect to these vars — ENV_LOCK.
    unsafe { std::env::set_var(OPENAIP_API_KEY_ENV, "from-process-env") };
    assert_eq!(
        Config::default().openaip_api_key(),
        Some("from-process-env".into())
    );
    unsafe { std::env::remove_var(OPENAIP_API_KEY_ENV) };
    assert_eq!(Config::default().openaip_api_key(), None);
}

// --- path resolution / STRATA_CONFIG ------------------------------------

#[test]
fn strata_config_env_overrides_path_for_load_and_save() {
    let _guard = env_guard();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom.toml");
    // SAFETY: serialized via ENV_LOCK.
    unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };

    assert_eq!(Config::path(), path);
    let config = non_default_config();
    config.save().unwrap();
    assert_eq!(Config::load(), config);

    unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
}

#[test]
fn legacy_config_env_still_works_but_loses_to_the_new_name() {
    let _guard = env_guard();
    // SAFETY: serialized via ENV_LOCK.
    unsafe { std::env::set_var(LEGACY_CONFIG_PATH_ENV, "/legacy/config.toml") };
    assert_eq!(Config::path(), PathBuf::from("/legacy/config.toml"));

    unsafe { std::env::set_var(CONFIG_PATH_ENV, "/new/config.toml") };
    assert_eq!(Config::path(), PathBuf::from("/new/config.toml"));

    unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
    unsafe { std::env::remove_var(LEGACY_CONFIG_PATH_ENV) };
}

#[test]
fn default_path_is_config_dir_strata() {
    let _guard = env_guard();
    // SAFETY: serialized via ENV_LOCK.
    unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
    unsafe { std::env::remove_var(LEGACY_CONFIG_PATH_ENV) };
    let path = Config::path();
    assert!(
        path.ends_with("strata/config.toml"),
        "unexpected path {}",
        path.display()
    );
}

// --- secrets -----------------------------------------------------------

#[test]
fn debug_redacts_api_key_and_autorouter_credentials() {
    let config = non_default_config();
    let debug = format!("{config:?}");
    assert!(!debug.contains("test-key-123"), "{debug}");
    assert!(!debug.contains("pilot@example.com"), "{debug}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("***redacted***"), "{debug}");
    // Unset secrets show as None, not as redacted.
    let debug = format!("{:?}", Config::default());
    assert!(debug.contains("openaip_api_key: None"), "{debug}");
    assert!(debug.contains("email: None"), "{debug}");
    assert!(debug.contains("password: None"), "{debug}");
}

// --- autorouter credentials ---------------------------------------------

/// The `[autorouter]` section round-trips through TOML with the
/// documented field names, and `credentials()` requires both non-blank.
#[test]
fn autorouter_credentials_round_trip_and_require_both_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    non_default_config().save_to(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("[autorouter]"), "{text}");
    assert!(text.contains("email = \"pilot@example.com\""), "{text}");
    assert!(text.contains("password = \"hunter2\""), "{text}");

    let loaded = Config::load_from(&path);
    assert_eq!(
        loaded.autorouter.credentials(),
        Some(("pilot@example.com".to_owned(), "hunter2".to_owned()))
    );

    // Both fields are required; blank counts as unset.
    let mut partial = AutorouterConfig {
        email: Some("pilot@example.com".into()),
        password: None,
    };
    assert_eq!(partial.credentials(), None);
    partial.password = Some("  ".into());
    assert_eq!(partial.credentials(), None);
    assert_eq!(AutorouterConfig::default().credentials(), None);
}

/// Configs written under the fields' pre-release names
/// (`client_id`/`client_secret`) still load.
#[test]
fn autorouter_pre_release_field_names_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "[autorouter]\nclient_id = \"pilot@example.com\"\nclient_secret = \"hunter2\"\n",
    )
    .unwrap();
    let loaded = Config::load_from(&path);
    assert_eq!(
        loaded.autorouter.credentials(),
        Some(("pilot@example.com".to_owned(), "hunter2".to_owned()))
    );
}

// --- theme types --------------------------------------------------------

#[test]
fn map_theme_serializes_as_plain_string() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    Config::default().save_to(&path).unwrap();
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("map_theme = \"auto\"")
    );

    let config = Config {
        map_theme: MapTheme::Named("oldworld".into()),
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("map_theme = \"oldworld\"")
    );
    assert_eq!(Config::load_from(&path).map_theme, config.map_theme);
}

#[test]
fn map_theme_auto_is_case_insensitive_and_blank_is_auto() {
    assert_eq!(MapTheme::from("AUTO".to_owned()), MapTheme::Auto);
    assert_eq!(MapTheme::from("  ".to_owned()), MapTheme::Auto);
    assert_eq!(
        MapTheme::from("oldworld".to_owned()),
        MapTheme::Named("oldworld".into())
    );
}

#[test]
fn map_theme_auto_follows_the_ui_theme_name_with_mode_fallback() {
    // Auto: a built-in map theme named after the UI theme wins …
    assert_eq!(
        MapTheme::Auto.resolved(ThemeMode::Dark, "Oldworld"),
        "oldworld"
    );
    assert_eq!(
        MapTheme::Auto.resolved(ThemeMode::Dark, "Catppuccin Mocha"),
        "catppuccin-mocha"
    );
    assert_eq!(
        MapTheme::Auto.resolved(ThemeMode::Light, "Solarized Light"),
        "solarized-light"
    );
    // … and UI themes without a same-named sibling fall back by mode.
    assert_eq!(
        MapTheme::Auto.resolved(ThemeMode::Dark, "Some Custom Theme"),
        "oldworld"
    );
    assert_eq!(
        MapTheme::Auto.resolved(ThemeMode::Light, "Some Custom Theme"),
        "pastel-light"
    );
    // An explicit Named selection ignores mode and UI theme alike.
    assert_eq!(
        MapTheme::Named("custom".into()).resolved(ThemeMode::Dark, "Catppuccin Mocha"),
        "custom"
    );
    assert!(ThemeMode::Dark.is_dark());
    assert!(!ThemeMode::Light.is_dark());
}

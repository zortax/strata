//! Application bootstrap: gpui-component init, theme loading, main window.

mod root_view;

pub use root_view::RootView;
pub(crate) use root_view::panel_animation;

use gpui::{
    AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use gpui_component::{Root, Theme, ThemeMode, ThemeRegistry};

use crate::assets::{Assets, ThemeAssets};
use crate::config::Config;

pub const WINDOW_TITLE: &str = "Strata";

/// Boots the application. gpui's `application()` is a per-process
/// singleton, so this must be called at most once.
pub fn run() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(|cx| {
        // Pre-rename installations first, before anything touches disk.
        migrate_legacy_user_dirs();
        // `~/.config/strata/config.toml` (robust: missing/broken → defaults).
        let config = Config::load();
        gpui_component::init(cx);
        // Tokio bridge for the reqwest-based weather provider and the
        // in-process ingest runner.
        gpui_tokio::init(cx);
        load_embedded_themes(cx);
        apply_configured_themes(cx, &config);
        open_main_window(cx, config);
        cx.activate(true);
    });
}

/// One-shot migration of the per-user directories of a pre-rename
/// installation (`~/.local/share` data dir and `~/.config` config dir) to
/// their Strata names. Same-filesystem `fs::rename`, so the multi-GB data
/// dir moves instantly; a no-op once migrated (or on fresh installs).
fn migrate_legacy_user_dirs() {
    use strata_data::paths;
    for base in [dirs::data_dir(), dirs::config_dir()].into_iter().flatten() {
        paths::migrate_legacy_dir(&base.join(paths::LEGACY_DIR_NAME), &base.join(paths::DIR_NAME));
    }
}

/// Loads every JSON theme baked into [`ThemeAssets`] into the
/// `ThemeRegistry`. Failures are logged but non-fatal — the registry's
/// built-in light/dark themes remain available.
fn load_embedded_themes(cx: &mut gpui::App) {
    let registry = ThemeRegistry::global_mut(cx);
    for path in ThemeAssets::iter() {
        let Some(file) = ThemeAssets::get(path.as_ref()) else {
            continue;
        };
        let body = match std::str::from_utf8(file.data.as_ref()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(theme = %path, %err, "embedded theme is not UTF-8");
                continue;
            }
        };
        if let Err(err) = registry.load_themes_from_str(body) {
            tracing::warn!(theme = %path, %err, "failed to register embedded theme");
        }
    }
}

/// Stages both configured UI themes (`ui_theme_light` fills the `Theme`
/// light slot, `ui_theme_dark` the dark slot) and applies the configured
/// mode — the mode's theme is applied last so it wins at startup. After
/// this, `Theme::change(mode, …)` flips between the two. Missing theme
/// names fall back to the registry's built-ins.
///
/// Also reused by the settings modal: after editing `ui_theme_dark` /
/// `ui_theme_light` it re-stages both slots so the active mode's theme
/// applies (or re-applies) immediately.
pub(crate) fn apply_configured_themes(cx: &mut gpui::App, config: &Config) {
    let registry = ThemeRegistry::global(cx);
    let light = registry
        .themes()
        .get(config.ui_theme_light.as_str())
        .cloned()
        .unwrap_or_else(|| {
            tracing::warn!(
                theme = config.ui_theme_light,
                "light theme not registered; falling back to built-in light"
            );
            registry.default_light_theme().clone()
        });
    let dark = registry
        .themes()
        .get(config.ui_theme_dark.as_str())
        .cloned()
        .unwrap_or_else(|| {
            tracing::warn!(
                theme = config.ui_theme_dark,
                "dark theme not registered; falling back to built-in dark"
            );
            registry.default_dark_theme().clone()
        });
    let theme = Theme::global_mut(cx);
    if config.mode.is_dark() {
        theme.apply_config(&light); // stages the light slot
        theme.apply_config(&dark); // stages the dark slot and wins at startup
        theme.mode = ThemeMode::Dark;
    } else {
        theme.apply_config(&dark);
        theme.apply_config(&light);
        theme.mode = ThemeMode::Light;
    }
}

fn open_main_window(cx: &mut gpui::App, config: Config) {
    let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(WINDOW_TITLE.into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        window_min_size: Some(size(px(1100.), px(700.))),
        kind: gpui::WindowKind::Normal,
        #[cfg(target_os = "linux")]
        window_decorations: Some(gpui::WindowDecorations::Client),
        ..Default::default()
    };

    if let Err(err) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| RootView::new(config, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::error!(%err, "failed to open main window");
    }
}

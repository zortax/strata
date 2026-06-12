//! Theme controls: the title-bar sun/moon mode toggle and the layers-panel
//! map-theme picker.
//!
//! Mapping rule: the sun/moon toggle owns *both* themes — flipping the mode
//! applies the configured gpui theme for that mode (`ui_theme_dark` /
//! `ui_theme_light`) and re-resolves the configured map theme (`auto`
//! follows the active UI theme by name — every UI theme has a same-named
//! map sibling — falling back to `oldworld`/`pastel-light` by mode; a named
//! theme is sticky). The layers-panel picker explicitly overrides only the
//! map theme for this session (the gpui theme and the config stay put); the
//! next mode toggle re-applies the configured resolution, dropping the
//! override.

use gpui::{
    Anchor, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, TITLE_BAR_HEIGHT, Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use strata_render::MapTheme;

use crate::app::RootView;
use crate::assets::IconName;

/// Flip dark ⇄ light: applies the staged gpui theme for the mode (see
/// `app::apply_configured_themes`) and re-resolves the configured map theme
/// (dropping any layers-panel override). The settings modal's mode switch
/// runs the same path via `ui::settings`.
fn toggle_ui_mode(root: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    let dark = !root.app_state.read(cx).dark_mode;
    let theme_id = root.app_state.update(cx, |state, cx| {
        state.set_dark_mode(dark, cx); // also persists the mode to config
        state.map_theme_id = state.resolved_map_theme_id();
        cx.notify();
        state.map_theme_id
    });
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, Some(window), cx);
    if let Some(map_theme) = MapTheme::by_id(theme_id) {
        root.map_view
            .update(cx, |map, cx| map.set_map_theme(map_theme, cx));
    }
    cx.notify();
}

/// Sun/moon toggle overlaid on the title bar, left of the window controls.
///
/// Interactive children *inside* [`gpui_component::TitleBar`] sit in the CSD
/// drag area and Hyprland swallows their clicks (the search input had to
/// move out for this reason). The window-control buttons stay clickable
/// because they live outside the drag area and stop the mouse-down — this
/// toggle copies that recipe: absolutely positioned next to the controls,
/// `occlude()` so the drag hit-test never sees the spot, and the mouse-down
/// swallowed before the title bar's move handling runs.
pub fn render_theme_toggle(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let dark = root.app_state.read(cx).dark_mode;
    let (icon, tooltip) = if dark {
        (IconName::Sun, "Switch to light mode")
    } else {
        (IconName::Moon, "Switch to dark mode")
    };
    div()
        .id("theme-toggle-slot")
        .occlude()
        .absolute()
        .top_0()
        // Clear of the three window-control buttons (each TITLE_BAR_HEIGHT
        // wide) plus a small gap.
        .right(TITLE_BAR_HEIGHT * 3.0 + px(8.0))
        .h(TITLE_BAR_HEIGHT)
        .flex()
        .items_center()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(
            Button::new("theme-toggle")
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tooltip)
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    toggle_ui_mode(this, window, cx);
                })),
        )
}

/// Palette button for the layers panel: a popup listing the built-in map
/// themes. Picking one restyles only the renderer — an explicit override of
/// the mode-default mapping; the gpui (UI) theme is untouched and the next
/// sun/moon toggle re-applies the mode default.
pub fn render_map_theme_picker(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let current = root.app_state.read(cx).map_theme_id;
    let app_state = root.app_state.clone();
    let map_view = root.map_view.clone();
    Button::new("map-theme-picker")
        .ghost()
        .small()
        .icon(IconName::Palette)
        .text_color(cx.theme().muted_foreground)
        .tooltip("Map theme")
        // BottomLeft: the panel hugs the window's bottom edge, so the menu
        // must open upward.
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
            for id in MapTheme::BUILT_IN_IDS {
                let Some(theme) = MapTheme::by_id(id) else {
                    continue;
                };
                let app_state = app_state.clone();
                let map_view = map_view.clone();
                menu = menu.item(
                    PopupMenuItem::new(theme.name)
                        .checked(theme.id == current)
                        .on_click(move |_, _, cx| {
                            app_state.update(cx, |state, cx| {
                                state.map_theme_id = theme.id;
                                cx.notify();
                            });
                            map_view.update(cx, |map, cx| map.set_map_theme(theme.clone(), cx));
                        }),
                );
            }
            menu
        })
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    /// The toggle resolves the map theme through config: `auto` follows the
    /// active UI theme by name — the defaults ("Oldworld" dark, "Pastel
    /// Light" light) resolve to their same-named map themes (Oldworld is
    /// also `MapTheme::default()`, not High Contrast). Full resolution
    /// semantics live in `state::tests`.
    #[test]
    fn auto_map_theme_follows_the_mode_defaults() {
        use crate::config::ThemeMode;

        let mut config = Config::default(); // map_theme = auto, mode = dark
        assert_eq!(
            config
                .map_theme
                .resolved(config.mode, &config.ui_theme_dark),
            "oldworld"
        );
        assert_eq!(strata_render::MapTheme::default().id, "oldworld");
        assert_ne!(strata_render::MapTheme::high_contrast().id, "oldworld");

        config.mode = ThemeMode::Light;
        assert_eq!(
            config
                .map_theme
                .resolved(config.mode, &config.ui_theme_light),
            "pastel-light"
        );
    }
}

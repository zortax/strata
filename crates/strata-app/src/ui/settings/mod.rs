//! Settings modal: a gear button in the title bar opens a gpui-component
//! `Dialog` hosting the `Settings` page system (Appearance / Map / Data /
//! Data sources). Every change writes through the [`Config`] API
//! (`save_if_changed`, atomic) and applies live where possible.
//!
//! Secrets: the openAIP API key renders masked at rest
//! ([`format::mask_api_key`]), the autorouter password in a masked input
//! (eye toggle); neither is ever logged. Both are stored in plain text in
//! config.toml — the autorouter row says so explicitly.

mod countries;
mod format;
mod pages;

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, StyleRefinement, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, TITLE_BAR_HEIGHT, Theme, ThemeMode, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, InputState, NumberInputEvent, StepAction},
    setting::Settings,
    slider::{SliderEvent, SliderState},
};

use strata_data::providers::autorouter::AutorouterClient;

use crate::app::RootView;
use crate::assets::IconName;
use crate::config::{AutorouterConfig, BASEMAP_DETAIL_BIAS_RANGE, WEATHER_REFRESH_MINUTES_RANGE};
use crate::map_view::MapView;
use crate::state::AppState;

/// Width of the settings dialog (sidebar + one settings page).
const DIALOG_WIDTH_PX: f32 = 980.;
/// Fixed content height — the page list scrolls inside it.
const DIALOG_BODY_HEIGHT_PX: f32 = 700.;
/// Sidebar width inside the dialog (narrower than the 250px default).
const SIDEBAR_WIDTH_PX: f32 = 200.;

/// Gear button overlaid on the title bar, directly left of the sun/moon
/// toggle. Same CSD-safe recipe as `ui::theme::render_theme_toggle`:
/// absolutely positioned outside the drag area, `occlude()`, and the
/// mouse-down swallowed before the title bar's move handling runs.
pub fn render_settings_button(_root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    div()
        .id("settings-button-slot")
        .occlude()
        .absolute()
        .top_0()
        // Window controls (3 × TITLE_BAR_HEIGHT) + gap + the sun/moon
        // toggle (small button = 24px) + gap.
        .right(TITLE_BAR_HEIGHT * 3.0 + px(8.0 + 24.0 + 6.0))
        .h(TITLE_BAR_HEIGHT)
        .flex()
        .items_center()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(
            Button::new("open-settings")
                .ghost()
                .small()
                .icon(IconName::Settings)
                .tooltip("Settings")
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    open_settings_dialog(this, window, cx);
                })),
        )
}

/// Opens the settings dialog. The content view is created once per open
/// (the dialog builder runs every frame, so the entity is captured, not
/// constructed, inside it). Esc / clicking the overlay close it.
///
/// Headerless and zero-padded: no `.title()` means no header row,
/// `.close_button(false)` drops the ✕, and `.p_0()` zeroes the edge
/// paddings the dialog would otherwise wrap around its children — the
/// settings sidebar and page run edge-to-edge inside the rounded frame.
pub fn open_settings_dialog(root: &RootView, window: &mut Window, cx: &mut Context<RootView>) {
    let app_state = root.app_state.clone();
    let map_view = root.map_view.clone();
    let view = cx.new(|cx| SettingsView::new(app_state, map_view, window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        // Sit slightly above vertical center — the default tenth-of-viewport
        // top margin looks top-heavy at this size. The builder re-runs every
        // frame, so this tracks window resizes.
        let dialog_height = px(DIALOG_BODY_HEIGHT_PX + 2.); // body + 1px borders
        let free = (window.viewport_size().height - dialog_height).max(px(0.));
        let margin_top = (free * 0.45).max(TITLE_BAR_HEIGHT);
        dialog
            .w(px(DIALOG_WIDTH_PX))
            .p_0()
            .close_button(false)
            .margin_top(margin_top)
            .overlay_closable(true)
            .child(view.clone())
    });
}

/// Dialog content: owns the stateful inputs (API key buffer, detail-bias
/// slider) and builds the `Settings` pages each frame.
pub struct SettingsView {
    pub(crate) app_state: Entity<AppState>,
    pub(crate) map_view: Entity<MapView>,
    /// Editing buffer for the openAIP API key; only rendered while
    /// [`Self::api_key_editing`] (masked summary otherwise).
    pub(crate) api_key_input: Entity<InputState>,
    pub(crate) api_key_editing: bool,
    /// Basemap detail bias; live-applies on change, persists on release.
    pub(crate) bias_slider: Entity<SliderState>,
    /// Weather refresh interval (minutes). Owned here instead of using the
    /// stock `SettingField::number_input`: at this gpui-component rev the
    /// stepper path updates the text via `InputState::set_value`, which
    /// suppresses `InputEvent::Change`, so stepped values would never reach
    /// the config. The Step subscription below commits them directly.
    pub(crate) weather_input: Entity<InputState>,
    /// autorouter.aero account email (`[autorouter] email`); commits on
    /// Enter/blur like the password below.
    pub(crate) autorouter_email_input: Entity<InputState>,
    /// autorouter.aero account password (`[autorouter] password`) —
    /// rendered masked (eye toggle reveals); stored in plain text in
    /// config.toml, which the settings row says honestly.
    pub(crate) autorouter_password_input: Entity<InputState>,
    /// Search box of the Countries page; filters the 36-row country list
    /// live (every change re-renders, the page builder filters).
    pub(crate) countries_filter: Entity<InputState>,
    /// Inline outcome of the "Test connection" button.
    pub(crate) autorouter_test: AutorouterTest,
    /// The running connection test (replacing it cancels the previous).
    autorouter_test_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// State of the autorouter "Test connection" button.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum AutorouterTest {
    #[default]
    Idle,
    Running,
    Success,
    Failed(String),
}

impl SettingsView {
    pub fn new(
        app_state: Entity<AppState>,
        map_view: Entity<MapView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let api_key_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste your openAIP API key"));
        let bias = app_state.read(cx).config.basemap_detail_bias;
        let bias_slider = cx.new(|_| {
            SliderState::new()
                .min(*BASEMAP_DETAIL_BIAS_RANGE.start() as f32)
                .max(*BASEMAP_DETAIL_BIAS_RANGE.end() as f32)
                .step(0.1)
                .default_value(bias as f32)
        });
        let refresh_minutes = app_state.read(cx).config.weather.refresh_minutes;
        let weather_input =
            cx.new(|cx| InputState::new(window, cx).default_value(refresh_minutes.to_string()));
        let autorouter = app_state.read(cx).config.autorouter.clone();
        let autorouter_email_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(autorouter.email.clone().unwrap_or_default())
                .placeholder("you@example.com")
        });
        let autorouter_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .default_value(autorouter.password.clone().unwrap_or_default())
                .placeholder("Account password")
        });
        let countries_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search countries…"));
        let subscriptions = vec![
            cx.subscribe_in(&api_key_input, window, Self::on_api_key_event),
            cx.subscribe(&bias_slider, Self::on_bias_slider_event),
            cx.subscribe_in(&weather_input, window, Self::on_weather_step),
            cx.subscribe(&weather_input, Self::on_weather_change),
            cx.subscribe_in(
                &autorouter_email_input,
                window,
                Self::on_autorouter_input_event,
            ),
            cx.subscribe_in(
                &autorouter_password_input,
                window,
                Self::on_autorouter_input_event,
            ),
            cx.subscribe(&countries_filter, Self::on_countries_filter_change),
            // Re-render on app-state changes (ingest running/idle flips the
            // download buttons, config edits move the value labels).
            cx.observe(&app_state, |_, _, cx| cx.notify()),
        ];
        Self {
            app_state,
            map_view,
            api_key_input,
            api_key_editing: false,
            bias_slider,
            weather_input,
            autorouter_email_input,
            autorouter_password_input,
            countries_filter,
            autorouter_test: AutorouterTest::Idle,
            autorouter_test_task: None,
            _subscriptions: subscriptions,
        }
    }

    // --- API key ----------------------------------------------------------

    /// Switch the key row into edit mode, prefilled with the current key.
    pub(crate) fn begin_api_key_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editing = true;
        let current = self
            .app_state
            .read(cx)
            .config
            .openaip_api_key
            .clone()
            .unwrap_or_default();
        self.api_key_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Commit the edit buffer (Enter, blur, or the Save button): blank
    /// removes the key, anything else stores it. Persisted via
    /// `save_if_changed`; takes effect on the next ingest run.
    pub(crate) fn commit_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.api_key_editing {
            return;
        }
        self.api_key_editing = false;
        let value = self.api_key_input.read(cx).value().trim().to_string();
        self.app_state.update(cx, |state, cx| {
            state.config.openaip_api_key = (!value.is_empty()).then_some(value);
            persist_config(state, cx);
        });
        // Don't keep the secret in the (hidden) edit buffer.
        self.api_key_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn on_api_key_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => self.commit_api_key(window, cx),
            // Commit on blur too — the task-switch / Save-click path.
            InputEvent::Blur => self.commit_api_key(window, cx),
            _ => {}
        }
    }

    // --- autorouter credentials ---------------------------------------------

    /// Commit both credential inputs (Enter, blur, or before a connection
    /// test): blank removes a credential, anything else stores it. On a
    /// change the config persists and the app's NOTAM provider is rebuilt
    /// immediately — the Briefing tab follows without a restart.
    pub(crate) fn commit_autorouter_credentials(&mut self, cx: &mut Context<Self>) {
        let non_blank = |value: String| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        };
        let next = AutorouterConfig {
            email: non_blank(self.autorouter_email_input.read(cx).value().to_string()),
            password: non_blank(self.autorouter_password_input.read(cx).value().to_string()),
        };
        let changed = self.app_state.update(cx, |state, cx| {
            if state.config.autorouter == next {
                return false;
            }
            state.config.autorouter = next;
            persist_config(state, cx);
            state.rebuild_notam_provider();
            true
        });
        if changed {
            // A previous test result says nothing about the new credentials.
            self.autorouter_test = AutorouterTest::Idle;
            cx.notify();
        }
    }

    fn on_autorouter_input_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
            self.commit_autorouter_credentials(cx);
        }
    }

    // --- countries ----------------------------------------------------------

    /// The Countries page's search box changed — re-render so the page
    /// builder re-filters the list (the query lives in the input state).
    fn on_countries_filter_change(
        &mut self,
        _input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    }

    /// The "Test connection" button: commit the inputs, then authenticate
    /// against api.autorouter.aero and run one cheap authenticated call
    /// on the tokio bridge. The outcome lands inline next to the button.
    pub(crate) fn run_autorouter_test(&mut self, cx: &mut Context<Self>) {
        self.commit_autorouter_credentials(cx);
        let Some((email, password)) = self.app_state.read(cx).config.autorouter.credentials()
        else {
            self.autorouter_test =
                AutorouterTest::Failed("Enter the account email and password first.".to_owned());
            cx.notify();
            return;
        };
        self.autorouter_test = AutorouterTest::Running;
        cx.notify();
        let check = gpui_tokio::Tokio::spawn_result(cx, async move {
            AutorouterClient::new(email, password)
                .test_connection()
                .await
                .map_err(anyhow::Error::from)
        });
        self.autorouter_test_task = Some(cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.autorouter_test = match result {
                    Ok(()) => AutorouterTest::Success,
                    Err(err) => AutorouterTest::Failed(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    // --- weather refresh interval ---------------------------------------------

    /// Stepper (±) on the weather interval: step, clamp, write the text
    /// back, and commit — `set_value` is event-silent at this rev, so the
    /// commit must happen here rather than via `InputEvent::Change`.
    fn on_weather_step(
        &mut self,
        input: &Entity<InputState>,
        event: &NumberInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let NumberInputEvent::Step(action) = event;
        let current = input.read(cx).value().parse::<i64>().unwrap_or(i64::from(
            self.app_state.read(cx).config.weather.refresh_minutes,
        ));
        let stepped = match action {
            StepAction::Increment => current + 1,
            StepAction::Decrement => current - 1,
        };
        let minutes = clamp_refresh_minutes(stepped);
        input.update(cx, |input, cx| {
            input.set_value(minutes.to_string(), window, cx);
        });
        self.commit_refresh_minutes(minutes, cx);
    }

    /// Typed edits commit on every parseable change (the diff-aware save
    /// makes that cheap); unparsable intermediate states are left alone.
    fn on_weather_change(
        &mut self,
        input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        if let Ok(value) = input.read(cx).value().trim().parse::<i64>() {
            self.commit_refresh_minutes(clamp_refresh_minutes(value), cx);
        }
    }

    fn commit_refresh_minutes(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            if state.config.weather.refresh_minutes != minutes {
                state.config.weather.refresh_minutes = minutes;
                persist_config(state, cx);
            }
        });
    }

    // --- basemap detail bias ------------------------------------------------

    /// Live-apply while dragging; persist once on release (a click also
    /// releases, so every change ends in a save).
    fn on_bias_slider_event(
        &mut self,
        _slider: Entity<SliderState>,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SliderEvent::Change(value) => {
                // Quantize to the slider's 0.1 lattice — the raw f32 would
                // serialize as e.g. -1.100000023841858 in config.toml.
                let bias = (f64::from(value.end()) * 10.0).round() / 10.0;
                self.map_view
                    .update(cx, |map, cx| map.set_basemap_detail_bias(bias, cx));
                self.app_state.update(cx, |state, cx| {
                    state.config.basemap_detail_bias = bias;
                    cx.notify();
                });
            }
            SliderEvent::Release(_) => {
                self.app_state.update(cx, persist_config);
            }
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `self` is leased during render — the page builders get the
        // handles directly and must never `view.read(cx)` (double-lease
        // panic); `view` is only for event-time `update`s.
        let pages = pages::build(self, &cx.entity(), window, cx);
        // gpui clips rectangularly, so the dialog's rounded corners don't
        // mask children. The sidebar is the only child painting its own
        // background into a corner — round its left side to match the
        // dialog frame (minus the 1px border it sits inside).
        let corner_radius = (cx.theme().radius_lg - px(1.)).max(px(0.));
        let sidebar_style = StyleRefinement::default().rounded_l(corner_radius);
        div().w_full().h(px(DIALOG_BODY_HEIGHT_PX)).child(
            Settings::new("strata-settings")
                .sidebar_width(px(SIDEBAR_WIDTH_PX))
                .sidebar_style(&sidebar_style)
                .pages(pages),
        )
    }
}

// --- shared appliers (used by the page builders) ----------------------------

/// Normalize + persist the current config off the UI thread (diff-aware
/// atomic write, ordered). The in-memory change already happened.
pub(crate) fn persist_config(state: &mut AppState, cx: &mut Context<AppState>) {
    state.config = state.config.clone().normalized();
    state.persist_config("settings change", cx);
    cx.notify();
}

/// Mode switch shared semantics with the title-bar sun/moon toggle: persist
/// `config.mode`, swap the staged gpui theme, re-resolve the map theme.
pub(crate) fn apply_mode(
    app_state: &Entity<AppState>,
    map_view: &Entity<MapView>,
    dark: bool,
    cx: &mut gpui::App,
) {
    let theme_id = app_state.update(cx, |state, cx| {
        state.set_dark_mode(dark, cx); // persists config.mode
        state.map_theme_id = state.resolved_map_theme_id();
        state.map_theme_id
    });
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, None, cx);
    apply_map_theme_id(map_view, theme_id, cx);
    cx.refresh_windows();
}

/// Store a UI theme name for one mode slot and restage both slots so the
/// active mode's theme (re-)applies immediately. An `auto` map theme
/// follows the active UI theme by name, so the map re-resolves too (a
/// change to the inactive slot or a named map theme is a no-op there).
pub(crate) fn apply_ui_theme(
    app_state: &Entity<AppState>,
    map_view: &Entity<MapView>,
    dark_slot: bool,
    name: &str,
    cx: &mut gpui::App,
) {
    let (config, theme_id) = app_state.update(cx, |state, cx| {
        if dark_slot {
            state.config.ui_theme_dark = name.to_owned();
        } else {
            state.config.ui_theme_light = name.to_owned();
        }
        persist_config(state, cx);
        state.map_theme_id = state.resolved_map_theme_id();
        (state.config.clone(), state.map_theme_id)
    });
    crate::app::apply_configured_themes(cx, &config);
    apply_map_theme_id(map_view, theme_id, cx);
    cx.refresh_windows();
}

/// Store the configured map theme ("auto" or a renderer theme id) and apply
/// the resolved renderer theme live.
pub(crate) fn apply_map_theme(
    app_state: &Entity<AppState>,
    map_view: &Entity<MapView>,
    value: &str,
    cx: &mut gpui::App,
) {
    let theme_id = app_state.update(cx, |state, cx| {
        state.config.map_theme = crate::config::MapTheme::from(value.to_owned());
        persist_config(state, cx);
        state.map_theme_id = state.resolved_map_theme_id();
        state.map_theme_id
    });
    apply_map_theme_id(map_view, theme_id, cx);
}

fn apply_map_theme_id(map_view: &Entity<MapView>, theme_id: &str, cx: &mut gpui::App) {
    if let Some(theme) = strata_render::MapTheme::by_id(theme_id) {
        map_view.update(cx, |map, cx| map.set_map_theme(theme, cx));
    }
}

/// Clamp a (possibly stepped/typed) interval into the documented range.
fn clamp_refresh_minutes(minutes: i64) -> u32 {
    minutes.clamp(
        i64::from(*WEATHER_REFRESH_MINUTES_RANGE.start()),
        i64::from(*WEATHER_REFRESH_MINUTES_RANGE.end()),
    ) as u32
}

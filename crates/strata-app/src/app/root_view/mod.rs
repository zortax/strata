//! The window's top-level view: title bar, the embedded map (with the
//! floating search box, layers panel, and info panel overlaid), status bar,
//! and the Root overlay layers (dialogs, sheets, notifications).

pub(crate) mod panel_animation;

use std::time::Duration;

use strata_data::store::{Feature, SearchHit};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Task, Window, div, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::notification::{Notification, NotificationType};
use gpui_component::slider::{SliderEvent, SliderState};
use gpui_component::{
    ActiveTheme as _, Icon, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::assets::IconName;
use crate::config::Config;
use crate::map_view::MapView;
use crate::ui::context_tabs::ContextPanel;
use crate::ui::profile_drawer::ProfileDrawer;
use crate::state::weather_time::{MAX_OFFSET_MINUTES, MIN_OFFSET_MINUTES, STEP_MINUTES};
use crate::state::{AppState, AppStateEvent, NoticeLevel};
use crate::ui;
use panel_animation::{PANEL_UNMOUNT_DELAY, PanelAnimation, PanelVisibility};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
const SEARCH_LIMIT: usize = 12;

/// Top-level view wrapped by `gpui_component::Root`.
pub struct RootView {
    pub(crate) app_state: Entity<AppState>,
    pub(crate) map_view: Entity<MapView>,
    /// The planning-mode right panel (tabbed context panel); renders
    /// nothing in explorer mode, where the info panel takes the slot.
    pub(crate) context_panel: Entity<ContextPanel>,
    /// The planning-mode bottom profile drawer (Profile | Nav Log).
    /// Renders nothing in explorer mode; also THE source of the planning
    /// chrome insets every surface above it lifts by.
    pub(crate) profile_drawer: Entity<ProfileDrawer>,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) search_results: Vec<SearchHit>,
    pub(crate) search_open: bool,
    pub(crate) taf_expanded: bool,
    pub(crate) banner_dismissed: bool,
    /// Mount/animation lifecycle of the info panel.
    pub(crate) panel_anim: PanelAnimation,
    /// Last non-empty selection; outlives `AppState::selection` so the info
    /// panel still has content to render during its exit animation.
    pub(crate) panel_selection: Vec<Feature>,
    /// Mount/animation lifecycle of the left flight panel (planning mode).
    pub(crate) flight_panel_anim: PanelAnimation,
    /// The flight panel's inputs/snapshot; `Some` while the panel is on
    /// screen (created on `FlightOpened`, dropped after the exit
    /// animation — the snapshot inside outlives `AppState::flight`).
    pub(crate) flight_panel: Option<ui::flight_panel::FlightPanelState>,
    /// Track state of the weather time slider (value = minutes from "now").
    pub(crate) time_slider: Entity<SliderState>,
    /// Mount/animation lifecycle of the weather time slider pill (visible
    /// while any gridded weather layer is on).
    pub(crate) slider_anim: PanelAnimation,
    /// Mount/animation lifecycle of the ingest progress panel (visible
    /// while `AppState::ingest_progress` says so).
    pub(crate) progress_anim: PanelAnimation,
    /// One-shot glide compensating the column reflow when the slider pill
    /// (un)mounts below the progress panel; finished glides idle at offset
    /// 0 and are replaced (re-keyed) by the next slider transition.
    pub(crate) progress_glide: Option<ui::progress_panel::Glide>,
    search_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app_state = cx.new(|cx| AppState::new(config, cx));
        let map_view = {
            let app_state = app_state.clone();
            cx.new(|cx| MapView::new(window, app_state, cx))
        };
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search airports, navaids, airspace…")
        });
        // Planning-mode bottom drawer; drives its own mount animation off
        // the flight lifecycle events. Its Profile tab hosts the
        // custom-painted profile view through the child-slot seam.
        let profile_drawer = {
            let app_state = app_state.clone();
            cx.new(|cx| ProfileDrawer::new(app_state, window, cx))
        };
        let profile_view = {
            let app_state = app_state.clone();
            cx.new(|cx| ui::profile_view::ProfileView::new(app_state, cx))
        };
        profile_drawer.update(cx, |drawer, cx| {
            drawer.set_profile_child(Some(profile_view.into()), cx)
        });
        // Planning-mode right panel; drives its own mount animation off the
        // flight lifecycle events. Reads the drawer for its bottom inset.
        let context_panel = {
            let app_state = app_state.clone();
            let profile_drawer = profile_drawer.clone();
            cx.new(|cx| ContextPanel::new(app_state, profile_drawer, window, cx))
        };
        // Weather time slider: minutes relative to "now" on the −2 h…+24 h
        // window, snapped to the 5-minute lattice.
        let time_slider = cx.new(|_| {
            SliderState::new()
                .min(MIN_OFFSET_MINUTES)
                .max(MAX_OFFSET_MINUTES)
                .step(STEP_MINUTES as f32)
                .default_value(0.0f32)
        });

        // Compositor-initiated close (xdg toplevel close — e.g. Super+Q):
        // consult the same dirty guard the CSD X button uses. Returning
        // false keeps the window; the guard's continuations close it via
        // `remove_window()`, which never consults this callback again, so
        // there is no loop.
        let root = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            root.update(cx, |this, cx| {
                if ui::flight_menu::window_close_needs_guard(this, cx) {
                    ui::flight_menu::request_close_window(this, window, cx);
                    false
                } else {
                    true
                }
            })
        });

        let subscriptions = vec![
            // AppState drives the status bar / info panel / banner.
            cx.observe(&app_state, |_, _, cx| cx.notify()),
            // The context panel's unmount hands the right slot back to the
            // info panel — re-render when its state changes.
            cx.observe(&context_panel, |_, _, cx| cx.notify()),
            // Drawer chrome changes (mount, expand/collapse, drag-resize)
            // move the planning insets — the column and the flight panel
            // re-derive them in the next render.
            cx.observe(&profile_drawer, |_, _, cx| cx.notify()),
            // Selection changes drive the info-panel animation state
            // machine; weather-time changes sync the slider thumb (which
            // needs the window, hence subscribe_in).
            cx.subscribe_in(&app_state, window, Self::on_app_state_event),
            cx.subscribe_in(&search_input, window, Self::on_search_event),
            cx.subscribe(&time_slider, Self::on_time_slider_event),
        ];

        Self {
            app_state,
            map_view,
            context_panel,
            profile_drawer,
            search_input,
            search_results: Vec::new(),
            search_open: false,
            taf_expanded: false,
            banner_dismissed: false,
            panel_anim: PanelAnimation::default(),
            panel_selection: Vec::new(),
            flight_panel_anim: PanelAnimation::default(),
            flight_panel: None,
            time_slider,
            slider_anim: PanelAnimation::default(),
            progress_anim: PanelAnimation::default(),
            progress_glide: None,
            search_task: None,
            _subscriptions: subscriptions,
        }
    }

    // --- info panel lifecycle -------------------------------------------------

    /// Feeds selection changes into the info panel's animation state
    /// machine (a new selection (re-)opens the panel or re-keys its
    /// content, a cleared selection starts the exit animation) and keeps
    /// the time-slider thumb in sync with the weather-time state.
    fn on_app_state_event(
        &mut self,
        app_state: &Entity<AppState>,
        event: &AppStateEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::SelectionChanged => {
                let selection = app_state.read(cx).selection.clone();
                if selection.is_empty() {
                    if let Some(epoch) = self.panel_anim.close_requested() {
                        self.schedule_panel_unmount(epoch, cx);
                    }
                } else {
                    self.panel_selection = selection;
                    self.panel_anim.open_requested();
                }
                cx.notify();
            }
            AppStateEvent::WeatherTimeChanged | AppStateEvent::WeatherTimeReanchored => {
                // Re-anchor / step buttons / clamping move the selection
                // outside the slider; push the new offset into the thumb.
                // A drag round-trips here with an identical value, which
                // the epsilon check turns into a no-op.
                let offset = app_state.read(cx).weather_time.offset_minutes();
                let current = self.time_slider.read(cx).value().end();
                if (current - offset).abs() > 0.01 {
                    self.time_slider
                        .update(cx, |slider, cx| slider.set_value(offset, window, cx));
                }
                cx.notify();
            }
            AppStateEvent::IngestProgressChanged => {
                let show = app_state.read(cx).ingest_progress.visible;
                let was_closed = self.progress_anim.visibility() == PanelVisibility::Closed;
                if let Some(epoch) =
                    ui::progress_panel::drive_visibility(&mut self.progress_anim, show)
                {
                    self.schedule_progress_unmount(epoch, cx);
                }
                if was_closed && self.progress_anim.visibility() == PanelVisibility::Open {
                    // Fresh mount: never replay a glide from a previous run.
                    self.progress_glide = None;
                }
                cx.notify();
            }
            // Flight lifecycle: push the route into the renderer's route
            // layer (`set_route` keeps identical routes idle, so forwarding
            // every flight event is cheap), drive the left flight panel's
            // mount/snapshot/field sync, and re-render the title-bar
            // strip/menu. Closing pushes `None` — the explorer stays clean.
            AppStateEvent::FlightOpened
            | AppStateEvent::FlightChanged
            | AppStateEvent::FlightComputed
            | AppStateEvent::FlightClosed => {
                let route = app_state.read(cx).flight_render_route();
                self.map_view.update(cx, |map, cx| map.set_route(route, cx));
                self.drive_flight_panel(event, window, cx);
                cx.notify();
            }
            // The aircraft dropdown rebuilds its items from the library.
            AppStateEvent::AircraftLibraryChanged => {
                self.drive_flight_panel(event, window, cx);
                cx.notify();
            }
            AppStateEvent::IngestNotice(notice) => {
                let kind = match notice.level {
                    NoticeLevel::Info => NotificationType::Info,
                    NoticeLevel::Success => NotificationType::Success,
                    NoticeLevel::Warning => NotificationType::Warning,
                    NoticeLevel::Error => NotificationType::Error,
                };
                window.push_notification(
                    Notification::new()
                        .message(notice.message.clone())
                        .with_type(kind),
                    cx,
                );
            }
            // FPL/PDF exports surface as window notifications, like the
            // ingest notices (state has no `Window`).
            AppStateEvent::ExportFinished(notice) => {
                use crate::state::briefing::ExportNotice;
                let (kind, message) = match notice {
                    ExportNotice::FplSaved(path) => (
                        NotificationType::Success,
                        format!("ICAO FPL saved to {}", path.display()),
                    ),
                    ExportNotice::FplFailed(err) => {
                        (NotificationType::Error, format!("FPL export failed: {err}"))
                    }
                    ExportNotice::PdfSaved(path) => (
                        NotificationType::Success,
                        format!("Briefing PDF saved to {}", path.display()),
                    ),
                    ExportNotice::PdfFailed(err) => (
                        NotificationType::Error,
                        format!("Briefing PDF export failed: {err}"),
                    ),
                };
                window.push_notification(
                    Notification::new().message(message).with_type(kind),
                    cx,
                );
            }
            _ => {}
        }
    }

    /// Slider drags/clicks land in AppState (which fans out to the renderer
    /// blend time and the fetch scheduler via `WeatherTimeChanged`).
    fn on_time_slider_event(
        &mut self,
        _slider: Entity<SliderState>,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        if let SliderEvent::Change(value) = event {
            let minutes = value.end();
            self.app_state
                .update(cx, |state, cx| state.set_weather_time_offset(minutes, cx));
        }
    }

    // --- weather time slider lifecycle ---------------------------------------

    /// Re-evaluates the slider pill's visibility after a layer toggle:
    /// shown while any gridded weather layer is on, hidden (via the exit
    /// animation) when the last one turns off.
    pub(crate) fn sync_time_slider_visibility(&mut self, cx: &mut Context<Self>) {
        let any_on = self.map_view.read(cx).any_gridded_layer_enabled();
        let slider_before = self.slider_anim.visibility();
        if let Some(epoch) = ui::time_slider::drive_visibility(&mut self.slider_anim, any_on) {
            self.schedule_slider_unmount(epoch, cx);
        }
        // A pill mounting below the visible progress panel reflows the
        // column; a one-shot glide moves the panel there smoothly. (The
        // unmount counterpart lives in `schedule_slider_unmount` — layout
        // only changes once the pill actually unmounts.)
        if let Some(glide) = ui::progress_panel::glide_for_slider_open(
            slider_before,
            self.slider_anim.visibility(),
            self.slider_anim.open_generation(),
            self.progress_anim.visibility() != PanelVisibility::Closed,
        ) {
            self.progress_glide = Some(glide);
        }
        cx.notify();
    }

    /// Slider twin of [`Self::schedule_panel_unmount`] (same epoch guard).
    fn schedule_slider_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.slider_anim.animation_done(epoch) {
                    // The pill unmounting reflows the column under the
                    // progress panel — glide it down to the new slot.
                    if let Some(glide) = ui::progress_panel::glide_for_slider_unmount(
                        epoch,
                        this.progress_anim.visibility() != PanelVisibility::Closed,
                    ) {
                        this.progress_glide = Some(glide);
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Progress-panel twin of [`Self::schedule_slider_unmount`].
    fn schedule_progress_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.progress_anim.animation_done(epoch) {
                    this.progress_glide = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Unmounts the info panel once its exit animation has played. gpui's
    /// `with_animation` has no completion callback, so a timer matching the
    /// exit duration stands in; the epoch guard inside `animation_done`
    /// makes stale timers (the panel re-opened meanwhile) harmless.
    fn schedule_panel_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.panel_anim.animation_done(epoch) {
                    this.panel_selection.clear();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // --- search -------------------------------------------------------------

    fn on_search_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = self.search_input.read(cx).value().to_string();
                self.spawn_search(query, cx);
            }
            InputEvent::PressEnter { .. } => {
                if !self.search_results.is_empty() {
                    self.activate_search_hit(0, window, cx);
                }
            }
            InputEvent::Focus => {
                self.search_open = true;
                cx.notify();
            }
            InputEvent::Blur => {
                self.search_open = false;
                cx.notify();
            }
        }
    }

    fn spawn_search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            self.search_results.clear();
            self.search_task = None;
            cx.notify();
            return;
        }
        let Some(store) = self.app_state.read(cx).store.clone() else {
            return;
        };
        // Replacing the task cancels the previous debounce/search.
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let hits = cx
                .background_spawn(async move {
                    store.search(&query, SEARCH_LIMIT).unwrap_or_else(|err| {
                        tracing::warn!(%err, "search failed");
                        Vec::new()
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.search_results = hits;
                this.search_open = true;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fly to a search result and select it in the info panel.
    pub(crate) fn activate_search_hit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hit) = self.search_results.get(index).cloned() else {
            return;
        };
        let zoom = ui::feature_fly_zoom(&hit.feature);
        self.map_view.update(cx, |map, cx| {
            map.fly_to(hit.position.lat(), hit.position.lon(), zoom, cx)
        });
        self.app_state
            .update(cx, |state, cx| state.set_selection(vec![hit.feature], cx));
        self.search_results.clear();
        self.search_open = false;
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Appends a search result to the open flight's route (the result
    /// row's "+" in planning mode). The dropdown stays open — adding
    /// several waypoints in a row is the expected flow; fly-to remains the
    /// row's primary action.
    pub(crate) fn append_search_hit_to_route(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(point) = self
            .search_results
            .get(index)
            .and_then(|hit| ui::search::route_point_for_hit(&hit.feature))
        else {
            return;
        };
        self.app_state
            .update(cx, |state, cx| state.append_waypoint(point, cx));
    }

    // --- chrome -------------------------------------------------------------

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // No interactive children INSIDE the TitleBar: Wayland CSD drag
        // handling swallows title-bar clicks, so inputs/buttons there never
        // focus. The theme toggle, the settings gear and the Flight menu are
        // overlaid OUTSIDE the drag area instead (see
        // `ui::theme::render_theme_toggle` / `ui::settings` /
        // `ui::flight_menu`). The flight strip is non-interactive text and
        // therefore safe inside the TitleBar — dragging works over it.
        div()
            .relative()
            .flex_shrink_0()
            .child(
                TitleBar::new()
                    // The default X button calls `window.remove_window()`
                    // directly — bypassing the platform should-close
                    // callback — so route it through the same dirty guard
                    // as the compositor close.
                    .on_close_window(cx.listener(|this, _, window, cx| {
                        ui::flight_menu::request_close_window(this, window, cx);
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::TowerControl).small())
                            .child(div().text_sm().child(crate::app::WINDOW_TITLE)),
                    )
                    .children(ui::flight_menu::render_flight_strip(self, cx))
                    .text_color(cx.theme().foreground),
            )
            .child(ui::flight_menu::render_flight_menu(self, cx))
            .child(ui::theme::render_theme_toggle(self, cx))
            .child(ui::settings::render_settings_button(self, cx))
    }

    /// Non-blocking "no data" banner under the title bar. The no-data case
    /// offers to download the data in-app (in-process ingest); while a run
    /// is in flight the progress panel takes over and the banner hides.
    fn render_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.app_state.read(cx);
        let banner = banner_state(
            state.store_error.as_deref(),
            state.has_data(),
            state.ingest_running(),
            self.banner_dismissed,
        )?;
        let (message, download_button) = match banner {
            BannerState::StoreError(err) => (format!("Data store unavailable: {err}"), None),
            BannerState::NoData => (
                format!(
                    "No aeronautical data in {} yet — download it now?",
                    state.data_dir.display()
                ),
                Some(
                    Button::new("banner-ingest")
                        .warning()
                        .xsmall()
                        .label("Download data")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app_state
                                .update(cx, |state, cx| state.run_needed_ingest(cx));
                        })),
                ),
            ),
        };
        Some(
            h_flex()
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .flex_shrink_0()
                .bg(cx.theme().warning.opacity(0.12))
                .border_b_1()
                .border_color(cx.theme().border)
                .text_sm()
                .text_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).small())
                .child(message)
                .children(download_button)
                .child(div().flex_1())
                .child(
                    Button::new("dismiss-banner")
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.banner_dismissed = true;
                            cx.notify();
                        })),
                ),
        )
    }

    /// THE planning-chrome insets for this frame, derived once from the
    /// profile drawer (design §3.6): the flight panel, the bottom-left
    /// column and (through its own read of the same entity) the context
    /// panel all lift their bottom edge by this — no per-panel constants.
    pub(crate) fn planning_insets(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> ui::profile_drawer::insets::PlanningChromeInsets {
        self.profile_drawer.read(cx).chrome_insets(window)
    }

    fn render_main_area(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The map fills the full window width; search (top-left), layers
        // panel (bottom-left) with the weather time slider above it, and
        // info panel (right) float above it.
        // In planning mode the search card lives inside the flight panel
        // (for as long as that is on screen — see `search_placement`);
        // in explorer mode it floats over the map exactly as it always has.
        let insets = self.planning_insets(window, cx);
        let search_overlay = (ui::search::search_placement(self.flight_panel_anim.visibility())
            == ui::search::SearchPlacement::Explorer)
            .then(|| ui::search::render_search(self, cx));
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.map_view.clone())
            .children(search_overlay)
            .child(
                // Shared bottom-left column (top to bottom: ingest progress
                // panel?, weather time slider?, layers panel): the layers
                // panel's intrinsic width drives the column, and the other
                // cards stretch to it (default cross-axis stretch) so they
                // always align regardless of how many toggles the panel
                // gains. The gap constant is shared with the progress
                // panel's glide distance. The left inset glides right of
                // the flight panel while planning mode is on; the inner
                // lift wrapper glides the column up by the profile
                // drawer's height (immediate during a drag-resize).
                ui::flight_panel::shift_bottom_left_column(
                    v_flex().absolute().bottom_3().child(
                        ui::profile_drawer::insets::lift_relative(
                            v_flex()
                                .gap(px(ui::time_slider::BOTTOM_LEFT_COLUMN_GAP_PX))
                                .children(ui::progress_panel::render_progress_panel(self, cx))
                                .children(ui::time_slider::render_time_slider(self, cx))
                                .child(ui::layers_panel::render_layers_panel(self, cx)),
                            &insets,
                            "bottom-left-column-lift",
                        ),
                    ),
                    &self.flight_panel_anim,
                ),
            )
            .children(ui::flight_panel::render_flight_panel(self, &insets, cx))
            // Right slot: in planning mode the tabbed context panel takes
            // over (including while it animates out); the explorer's
            // selection panel otherwise.
            .children(
                (!self.context_panel.read(cx).is_mounted())
                    .then(|| ui::info_panel::render_info_panel(self, cx))
                    .flatten(),
            )
            .child(self.context_panel.clone())
            // Bottom edge: the profile drawer paints over the panels while
            // the planning chrome animates in/out.
            .child(self.profile_drawer.clone())
    }
}

/// What the under-title-bar banner shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BannerState {
    /// The store failed to open — always shown (even mid-ingest; a broken
    /// store is unrelated to download progress).
    StoreError(String),
    /// No aero dataset ingested yet: offer the in-app download.
    NoData,
}

/// Pure banner visibility/content decision. The no-data banner yields to a
/// running ingest (the progress panel is the status surface then) and stays
/// dismissible; a store error outranks everything except dismissal.
fn banner_state(
    store_error: Option<&str>,
    has_data: bool,
    ingest_running: bool,
    dismissed: bool,
) -> Option<BannerState> {
    if dismissed {
        return None;
    }
    if let Some(err) = store_error {
        return Some(BannerState::StoreError(err.to_owned()));
    }
    if !has_data && !ingest_running {
        return Some(BannerState::NoData);
    }
    None
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .relative()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .bg(cx.theme().background)
                    .text_color(cx.theme().foreground)
                    .child(self.render_title_bar(cx))
                    .children(self.render_banner(cx))
                    .child(self.render_main_area(window, cx))
                    .child(ui::status_bar::render_status_bar(self, cx)),
            )
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_offers_download_only_without_data_and_without_a_running_ingest() {
        assert_eq!(
            banner_state(None, false, false, false),
            Some(BannerState::NoData)
        );
        // A running ingest replaces the banner with the progress panel.
        assert_eq!(banner_state(None, false, true, false), None);
        // Data present → nothing to nag about.
        assert_eq!(banner_state(None, true, false, false), None);
    }

    #[test]
    fn store_errors_outrank_the_no_data_offer_even_mid_ingest() {
        assert_eq!(
            banner_state(Some("locked"), true, true, false),
            Some(BannerState::StoreError("locked".into()))
        );
    }

    #[test]
    fn dismissal_silences_every_banner() {
        assert_eq!(banner_state(Some("locked"), false, false, true), None);
        assert_eq!(banner_state(None, false, false, true), None);
    }
}

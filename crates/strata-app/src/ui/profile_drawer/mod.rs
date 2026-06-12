//! The planning-mode profile drawer (design §3.3): a frosted card
//! floating along the bottom edge of the map area — the other panels'
//! `_3` inset from the left/right/bottom edges, full border, rounded
//! corners — mounted with the established [`PanelAnimation`] machinery
//! while a flight is open. Two height states — a collapsed ~40 px summary strip
//! (total distance/ETE, the mini elevation sparkline, conflict badge
//! glyphs, expand affordance) and the
//! expanded drawer (default ~280 px, drag-resizable between
//! [`state::MIN_EXPANDED_HEIGHT_PX`] and 60 % of the window via the grab
//! handle on its top edge; the height persists in config). Header tabs:
//! **Profile** (hosting the custom-painted profile view through the
//! [`ProfileDrawer::set_profile_child`] seam) and **Nav Log** (the PLOG
//! table, see [`navlog`]).
//!
//! Everything that floats above the drawer — the flight panel, the
//! context panel, the bottom-left overlay column — lifts its bottom edge
//! through the one shared [`insets::PlanningChromeInsets`] source derived
//! here ([`ProfileDrawer::chrome_insets`]): animated on expand/collapse
//! and mount/unmount, immediate during a drag-resize.
//!
//! [`PanelAnimation`]: crate::app::panel_animation::PanelAnimation

pub mod insets;
pub mod state;

pub(crate) mod navlog;

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyView, Context, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    ease_out_quint, px, quadratic,
};
use gpui_component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};

use strata_render::MapTheme;

use crate::app::panel_animation::{
    PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PANEL_UNMOUNT_DELAY, PanelAnimation,
    PanelVisibility,
};
use crate::assets::IconName;
use crate::state::flight::PlanningFocus;
use crate::state::{AppState, AppStateEvent};
use crate::ui::flight_panel::model::{self, BadgeTone};
use crate::ui::profile_view::{ProfileSeries, sparkline};

use insets::PlanningChromeInsets;
use navlog::NotesInputs;
use state::{DrawerMode, DrawerState, LiftKind, LiftToggle};

pub use state::DEFAULT_EXPANDED_HEIGHT_PX;

/// Height of the grab-handle strip on the expanded drawer's top edge.
/// Deliberately taller than the 3 px pill it shows: the whole transparent
/// strip is the pointer target (a ~10 px target was too fiddly to grab).
const HANDLE_HEIGHT_PX: f32 = 18.;

/// Corridor half-width choices of the header's compact select, in NM
/// (design §3.3: "configurable ±2–5 NM").
const CORRIDOR_WIDTH_CHOICES_NM: [f64; 3] = [2.0, 3.0, 5.0];

/// Width of one header tab (Profile | Nav Log). The tabs are fixed-width
/// so the segmented bar sizes to its content — equal buttons, no dead
/// track beside them.
const TAB_WIDTH_PX: f32 = 104.;

/// The drawer's header tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerTab {
    #[default]
    Profile,
    NavLog,
}

impl DrawerTab {
    pub const ALL: [Self; 2] = [Self::Profile, Self::NavLog];

    pub fn label(self) -> &'static str {
        match self {
            Self::Profile => "Profile",
            Self::NavLog => "Nav Log",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Profile => 0,
            Self::NavLog => 1,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// The bottom profile drawer of planning mode.
pub struct ProfileDrawer {
    pub(crate) app_state: Entity<AppState>,
    /// Mount/animation lifecycle (planning mode on/off).
    anim: PanelAnimation,
    /// Chrome state: collapsed/expanded, height, drag, lift toggles.
    state: DrawerState,
    active_tab: DrawerTab,
    /// The Profile tab's child slot — the seam for the custom-painted
    /// profile view (its own module). `None` renders the placeholder.
    profile_child: Option<AnyView>,
    /// The collapsed strip's mini-sparkline series — the same
    /// [`ProfileSeries`] the full chart flattens, cached per compute
    /// generation (flight events clear it, the strip rebuilds lazily).
    strip_series: Option<Rc<ProfileSeries>>,
    /// Nav Log notes column inputs, in lockstep with the route.
    notes: NotesInputs,
    _subscriptions: Vec<Subscription>,
}

impl ProfileDrawer {
    pub fn new(app_state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.subscribe_in(&app_state, window, Self::on_app_state_event)];
        let height = app_state.read(cx).config.profile_drawer.height_px;
        let mut this = Self {
            app_state,
            anim: PanelAnimation::default(),
            state: DrawerState::new(height),
            active_tab: DrawerTab::default(),
            profile_child: None,
            strip_series: None,
            notes: NotesInputs::new(),
            _subscriptions: subscriptions,
        };
        // Defensive: a flight could already be open by construction time.
        if this.app_state.read(cx).planning_mode() {
            this.anim.open_requested();
            this.state.mounted(window_height(window));
            this.sync_notes(window, cx);
        }
        this
    }

    /// Installs (or clears) the Profile tab's child view — the
    /// integration seam for the custom-painted profile view module
    /// (`RootView` installs the [`ProfileView`] entity here). Until it is
    /// wired the tab shows a placeholder.
    ///
    /// [`ProfileView`]: crate::ui::profile_view::ProfileView
    pub fn set_profile_child(&mut self, child: Option<AnyView>, cx: &mut Context<Self>) {
        self.profile_child = child;
        cx.notify();
    }

    /// THE planning-chrome insets source (design §3.6 / task item 5):
    /// what every surface above the drawer derives its bottom lift from.
    /// `RootView` reads it for the flight panel and the bottom-left
    /// column; the context panel reads it for itself — one derivation,
    /// no per-panel constants. The state machine speaks drawer *heights*;
    /// the floating card's own bottom inset is folded in here
    /// ([`insets::lift_for_height`]), so consumers always clear the full
    /// band the card occupies.
    pub fn chrome_insets(&self, window: &Window) -> PlanningChromeInsets {
        match self.anim.visibility() {
            PanelVisibility::Closed => PlanningChromeInsets::NONE,
            PanelVisibility::Open | PanelVisibility::Closing => {
                let toggle = self.state.lift_toggle().map(|t| LiftToggle {
                    from_px: insets::lift_for_height(t.from_px),
                    to_px: insets::lift_for_height(t.to_px),
                    ..t
                });
                PlanningChromeInsets {
                    lift_px: toggle.map_or_else(
                        || insets::lift_for_height(self.state.height_px(window_height(window))),
                        |t| t.to_px,
                    ),
                    toggle,
                }
            }
        }
    }

    // --- lifecycle -----------------------------------------------------------

    fn on_app_state_event(
        &mut self,
        _app_state: &Entity<AppState>,
        event: &AppStateEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::FlightOpened => {
                self.anim.open_requested();
                self.state.mounted(window_height(window));
                self.strip_series = None;
                self.sync_notes(window, cx);
                cx.notify();
            }
            AppStateEvent::FlightClosed => {
                if let Some(epoch) = self.anim.close_requested() {
                    self.state.unmounting(window_height(window));
                    self.schedule_unmount(epoch, cx);
                }
                self.strip_series = None;
                cx.notify();
            }
            AppStateEvent::FlightChanged | AppStateEvent::FlightComputed => {
                self.strip_series = None;
                self.sync_notes(window, cx);
                cx.notify();
            }
            // A Terrain/Airspace badge navigated here (design §3.1): make
            // the Profile tab visible — the scrub already sits on the
            // conflict (set by `request_planning_focus` before the event).
            AppStateEvent::PlanningFocusRequested(PlanningFocus::Profile { .. }) => {
                self.active_tab = DrawerTab::Profile;
                self.state
                    .set_mode(DrawerMode::Expanded, window_height(window));
                cx.notify();
            }
            // Header controls reflect the new corridor / overlay state.
            AppStateEvent::CorridorChanged | AppStateEvent::ProfileWeatherChanged => cx.notify(),
            _ => {}
        }
    }

    /// Unmount timer twin of the other panels' (same epoch guard — see
    /// `RootView::schedule_panel_unmount`).
    fn schedule_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.anim.animation_done(epoch) {
                    this.notes.clear();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Brings the notes inputs in line with the open document.
    fn sync_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(route) = self
            .app_state
            .read(cx)
            .flight
            .as_ref()
            .map(|f| f.doc.route.clone())
        else {
            return;
        };
        self.notes.sync(&route, window, cx);
    }

    // --- chrome interactions ----------------------------------------------------

    fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_mode(window_height(window));
        cx.notify();
    }

    fn on_handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .state
            .begin_resize(f32::from(event.position.y), window_height(window))
        {
            cx.notify();
        }
    }

    fn on_overlay_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_resizing() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            // Released outside the window: treat re-entry as the release.
            self.finish_resize(cx);
            return;
        }
        if self
            .state
            .resize_to(f32::from(event.position.y), window_height(window))
        {
            cx.notify();
        }
    }

    fn on_overlay_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_resize(cx);
    }

    /// Ends an active drag and persists the resulting height.
    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(height) = self.state.end_resize() {
            self.app_state
                .update(cx, |state, cx| state.set_profile_drawer_height(height, cx));
            cx.notify();
        }
    }

    // --- rendering -----------------------------------------------------------

    /// The collapsed strip: mini summary (totals + badge glyphs), the mini
    /// elevation sparkline (design §3.3 "slim summary strip (mini elevation
    /// sparkline + conflict badges)") and the expand affordance; the whole
    /// strip is clickable.
    fn render_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // Lazy per-compute-generation cache: flight events clear it, the
        // first collapsed render after a compute rebuilds it.
        if self.strip_series.is_none() {
            let state = self.app_state.read(cx);
            self.strip_series = state.flight.as_ref().and_then(|flight| {
                let computed = flight.computed.as_deref()?;
                Some(Rc::new(ProfileSeries::build(&flight.doc, computed)))
            });
        }
        let state = self.app_state.read(cx);
        let computed = state.flight.as_ref().and_then(|f| f.computed.clone());
        let (distance, ete) = computed.as_deref().map_or_else(
            || (navlog::EM_DASH.to_owned(), navlog::EM_DASH.to_owned()),
            |c| {
                (
                    format!("{} NM", model::fmt_nm(c.navlog.totals.distance)),
                    model::fmt_minutes(c.navlog.totals.ete),
                )
            },
        );
        let notam = model::notam_badge_vm(
            state.notam_badge(),
            state.flight.as_ref().and_then(|f| f.briefing.as_ref()),
        );
        let badges = model::badge_row(computed.as_deref().map(|c| c.conflicts.as_slice()), notam);

        // The sparkline matches the full chart's tints (active map theme).
        let map_theme = MapTheme::by_id(state.map_theme_id).unwrap_or_default();
        let (terrain_fill, terrain_stroke, planned) = sparkline::sparkline_colors(&map_theme);
        let spark = self
            .strip_series
            .as_ref()
            .map(|series| sparkline::sparkline(series, terrain_fill, terrain_stroke, planned));

        h_flex()
            .id("profile-drawer-strip")
            .size_full()
            .px_3()
            .gap_3()
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx)))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("PROFILE"),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(distance),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("ETE {ete}")),
            )
            .child(h_flex().gap_2().children(
                badges.into_iter().map(|badge| {
                    let dot: AnyElement = match badge.tone {
                        BadgeTone::Unknown => div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(navlog::EM_DASH)
                            .into_any_element(),
                        tone => div()
                            .size_2()
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(match tone {
                                BadgeTone::Unknown => cx.theme().muted_foreground,
                                BadgeTone::Ok => cx.theme().success,
                                BadgeTone::Caution => cx.theme().warning,
                                BadgeTone::Alert => cx.theme().danger,
                            })
                            .into_any_element(),
                    };
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(dot)
                        .child(div().text_xs().child(badge.label))
                        .into_any_element()
                }),
            ))
            // The mini elevation sparkline fills the strip's free middle —
            // passive (no listeners), so the strip stays one click target.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(24.))
                    .children(spark),
            )
            // Passive affordance: the whole strip is the click target (a
            // button here would double-fire through the strip's handler).
            .child(
                gpui_component::Icon::new(IconName::ChevronUp)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .into_any_element()
    }

    /// The expanded drawer: grab handle, tab header, tab body.
    fn render_expanded(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let handle = h_flex()
            .id("profile-drawer-resize")
            .w_full()
            .h(px(HANDLE_HEIGHT_PX))
            .flex_shrink_0()
            .justify_center()
            .items_center()
            .cursor_row_resize()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_handle_mouse_down))
            .child(
                div()
                    .w_10()
                    .h(px(3.))
                    .rounded_full()
                    .bg(cx.theme().muted_foreground.opacity(0.4)),
            );

        // Fixed-width tabs (equal buttons) in an auto-width bar: the
        // segmented container hugs its two buttons instead of stretching
        // them across a wider track (no dead area right of "Nav Log").
        let tab_bar = TabBar::new("profile-drawer-tabs")
            .segmented()
            .selected_index(self.active_tab.index())
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                if let Some(tab) = DrawerTab::from_index(*index) {
                    this.active_tab = tab;
                    cx.notify();
                }
            }))
            .children(
                DrawerTab::ALL
                    .into_iter()
                    .map(|tab| Tab::new().label(tab.label()).w(px(TAB_WIDTH_PX))),
            );

        let header = h_flex()
            .px_2()
            .pb_1()
            .gap_2()
            .items_center()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(tab_bar)
            .child(div().flex_1())
            .children(self.render_weather_controls(cx))
            .children(self.render_corridor_controls(cx))
            .child(
                Button::new("drawer-collapse")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .tooltip("Collapse to the summary strip")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            );

        let body: AnyElement = match self.active_tab {
            DrawerTab::Profile => self.render_profile_tab(cx),
            DrawerTab::NavLog => navlog::render_navlog_tab(self, cx),
        };

        v_flex()
            .size_full()
            .child(handle)
            .child(header)
            .child(div().flex_1().min_h_0().min_w_0().child(body))
            .into_any_element()
    }

    /// The Profile tab's weather-overlay toggles (design §3.3 "weather
    /// overlays (toggle)"): the freezing-level line and the forecast
    /// cloud-base band, both on by default. Session view state on
    /// [`AppState::profile_weather`] — deliberately not persisted. `None`
    /// on the Nav Log tab.
    fn render_weather_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.active_tab != DrawerTab::Profile {
            return None;
        }
        let overlays = self.app_state.read(cx).profile_weather;
        let freezing = Button::new("overlay-freezing")
            .ghost()
            .xsmall()
            .icon(IconName::Snowflake)
            .selected(overlays.freezing)
            .tooltip("Freezing-level line (ISA estimate from leg winds)")
            .on_click(cx.listener(|this, _, _, cx| {
                this.app_state.update(cx, |state, cx| {
                    let mut overlays = state.profile_weather;
                    overlays.freezing = !overlays.freezing;
                    state.set_profile_weather(overlays, cx);
                });
            }));
        let cloud_base = Button::new("overlay-cloud-base")
            .ghost()
            .xsmall()
            .icon(IconName::Cloud)
            .selected(overlays.cloud_base)
            .tooltip("Forecast cloud-base band (draws once forecast data is wired)")
            .on_click(cx.listener(|this, _, _, cx| {
                this.app_state.update(cx, |state, cx| {
                    let mut overlays = state.profile_weather;
                    overlays.cloud_base = !overlays.cloud_base;
                    state.set_profile_weather(overlays, cx);
                });
            }));
        Some(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("WEATHER"),
                )
                .child(freezing)
                .child(cloud_base)
                .into_any_element(),
        )
    }

    /// The Profile tab's header controls (design §3.2 corridor outline /
    /// §3.3 corridor width): the ±2/3/5 NM half-width select feeding
    /// [`ComputeParams`] through the config, and the eye toggling the map's
    /// corridor outline. `None` on the Nav Log tab — they describe the
    /// profile.
    ///
    /// [`ComputeParams`]: strata_plan::compute::ComputeParams
    fn render_corridor_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.active_tab != DrawerTab::Profile {
            return None;
        }
        let state = self.app_state.read(cx);
        let visible = state.corridor_visible;
        let current_nm = state.config.profile_drawer.corridor_half_width_nm;

        let width_select = ButtonGroup::new("corridor-width")
            .compact()
            .outline()
            .xsmall()
            .children(CORRIDOR_WIDTH_CHOICES_NM.into_iter().map(|nm| {
                Button::new(("corridor-width-nm", nm as u64))
                    .label(format!("±{nm:.0}"))
                    .selected((current_nm - nm).abs() < 1e-9)
                    .tooltip(format!("Corridor half-width ±{nm:.0} NM (recomputes)"))
            }))
            .on_click(cx.listener(|this, clicks: &Vec<usize>, _, cx| {
                let Some(nm) = clicks
                    .first()
                    .and_then(|&index| CORRIDOR_WIDTH_CHOICES_NM.get(index).copied())
                else {
                    return;
                };
                this.app_state
                    .update(cx, |state, cx| state.set_corridor_half_width_nm(nm, cx));
            }));

        let eye = Button::new("corridor-eye")
            .ghost()
            .xsmall()
            .icon(if visible {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .selected(visible)
            .tooltip("Show the corridor outline on the map")
            .on_click(cx.listener(|this, _, _, cx| {
                this.app_state.update(cx, |state, cx| {
                    let next = !state.corridor_visible;
                    state.set_corridor_visible(next, cx);
                });
            }));

        Some(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("CORRIDOR"),
                )
                .child(width_select)
                .child(eye)
                .into_any_element(),
        )
    }

    /// The Profile tab: the installed child view at full size, or the
    /// placeholder until the profile-view module is wired in.
    fn render_profile_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.profile_child {
            Some(child) => div().size_full().child(child.clone()).into_any_element(),
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("The vertical profile mounts here — its view module is on the way."),
                )
                .into_any_element(),
        }
    }
}

impl Render for ProfileDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visibility = self.anim.visibility();
        if visibility == PanelVisibility::Closed {
            // Out of planning mode: occupy nothing.
            return div().absolute().into_any_element();
        }

        let window_height = window_height(window);
        let height = self.state.height_px(window_height);
        let toggle = self.state.lift_toggle();
        let resizing = self.state.is_resizing();

        let content: AnyElement = if self.state.is_expanded() {
            self.render_expanded(cx)
        } else {
            self.render_strip(cx)
        };

        // The frosted card — the floating-panel recipe of the flight/
        // context panels (full border, all corners rounded; the wrapper
        // below owns the `_3` window insets). Expand/collapse animates its
        // height with the very toggle that drives every lifted surface
        // (see `insets`), so the chrome moves as one; drag-resize bypasses
        // the animation (toggle = None, layout follows the pointer).
        let card = v_flex()
            .occlude()
            .w_full()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .bg(cx.theme().background.opacity(0.78))
            .backdrop_blur(px(18.))
            .shadow_lg()
            .overflow_hidden()
            .child(content);
        let card: AnyElement = match toggle {
            Some(toggle) if toggle.kind == LiftKind::Mode => {
                let (from, to) = (toggle.from_px, toggle.to_px);
                card.with_animation(
                    ("profile-drawer-height", toggle.generation),
                    insets::toggle_animation(&toggle),
                    move |card, delta| card.h(px(from + (to - from) * delta)),
                )
                .into_any_element()
            }
            // Mount/unmount keep the card at full height — the enter/exit
            // slide below moves it in from the bottom edge instead.
            _ => card.h(px(height)).into_any_element(),
        };

        // Enter/exit: the established one-shot slide, vertical, over the
        // card's full height plus its bottom inset (it starts fully off
        // screen) — the card's top edge therefore tracks the mount/unmount
        // lift toggle exactly (same travel, same timing, same easing).
        let travel = height + insets::DRAWER_INSET_PX;
        let shell = div().relative().w_full().child(card);
        let shell: AnyElement = match visibility {
            PanelVisibility::Closed => unreachable!("handled above"),
            PanelVisibility::Open => shell
                .with_animation(
                    ("profile-drawer-enter", self.anim.open_generation()),
                    Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
                    move |shell, delta| shell.top(px(travel * (1. - delta))),
                )
                .into_any_element(),
            PanelVisibility::Closing => shell
                .with_animation(
                    ("profile-drawer-exit", self.anim.close_epoch()),
                    Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
                    move |shell, delta| shell.top(px(travel * delta)),
                )
                .into_any_element(),
        };

        // Root: a passive full-area wrapper (no listeners — map input
        // passes through) hosting the floating card (the other panels'
        // `_3` inset from the left/right/bottom edges) and, mid-drag, the
        // window-wide capture overlay that keeps resize tracking alive
        // however fast the pointer leaves the grab handle. The overlay
        // is the LAST child: later siblings paint (and hit-test) on top,
        // so the release reaches it even when the pointer ends up over
        // the occluding card.
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .left(px(insets::DRAWER_INSET_PX))
                    .right(px(insets::DRAWER_INSET_PX))
                    .bottom(px(insets::DRAWER_INSET_PX))
                    .child(shell),
            )
            .when(resizing, |el| {
                el.child(
                    div()
                        .id("profile-drawer-resize-overlay")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .cursor_row_resize()
                        .on_mouse_move(cx.listener(Self::on_overlay_mouse_move))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(Self::on_overlay_mouse_up),
                        ),
                )
            })
            .into_any_element()
    }
}

/// Window height in logical px (the drawer's resize bounds reference).
fn window_height(window: &Window) -> f32 {
    f32::from(window.viewport_size().height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawer_tabs_round_trip_their_indices() {
        for tab in DrawerTab::ALL {
            assert_eq!(DrawerTab::from_index(tab.index()), Some(tab));
        }
        assert_eq!(DrawerTab::from_index(2), None);
        assert_eq!(DrawerTab::default(), DrawerTab::Profile);
        assert_eq!(DrawerTab::Profile.label(), "Profile");
        assert_eq!(DrawerTab::NavLog.label(), "Nav Log");
    }
}

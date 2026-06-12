//! The planning-mode context panel (design §3.4): the explorer's right
//! info-panel slot topped with a segmented tab bar — Inspect | Weather |
//! Loading | Fuel | Briefing. Inspect embeds the explorer's selection
//! cards unchanged; the other tabs are planning surfaces over the open
//! flight.
//!
//! Mounted while a flight is open: [`ContextPanel`] subscribes to the
//! flight lifecycle events and drives the shared [`PanelAnimation`]
//! machine; in explorer mode it renders nothing and `RootView` shows the
//! classic info panel instead.

pub(crate) mod briefing;
mod envelope;
mod fuel;
mod loading;
mod tabs;
mod weather;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, ease_out_quint, px, quadratic,
};
use gpui_component::{
    ActiveTheme as _,
    tab::{Tab, TabBar},
    v_flex,
};
use strata_plan::compute::ComputedFlight;
use strata_plan::fuel::FuelLadder;
use strata_plan::{AircraftProfile, FlightDoc};
use strata_render::MapTheme;

use crate::app::panel_animation::{
    PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PANEL_UNMOUNT_DELAY, PanelAnimation,
    PanelVisibility,
};
use crate::state::flight::PlanningFocus;
use crate::state::{AppState, AppStateEvent, ComputeState};
use crate::ui::info_panel::{self, CardCallback, card};
use crate::ui::profile_drawer::{ProfileDrawer, insets};

use fuel::PolicyInputs;
use loading::LoadingInputs;
pub(crate) use tabs::{ContextTab, tab_after_selection_change};

/// Panel width in planning mode (design §3.4: ~400 px; the explorer's
/// selection panel stays at its 360 px).
const PANEL_WIDTH_PX: f32 = 400.;
/// Resting inset from the window edge (matches `right_3`).
const PANEL_INSET_PX: f32 = 12.;
/// Horizontal travel of the enter/exit animation.
const PANEL_SLIDE_PX: f32 = 20.;

/// Everything a tab renders from, snapshotted once per frame so no
/// `AppState` borrow is held while listeners are built.
struct FlightView {
    doc: FlightDoc,
    aircraft: Option<AircraftProfile>,
    computed: Option<Arc<ComputedFlight>>,
    /// Copy of the computed fuel ladder (it is `Copy`-sized).
    ladder: Option<FuelLadder>,
    /// The derived NOTAM briefing list (see `state::briefing`).
    briefing: Option<crate::state::briefing::BriefingRelevance>,
    /// User-facing line for "why is there no computed data" states;
    /// `None` when the latest compute landed.
    compute_hint: Option<String>,
    /// Whether `computed` reflects the newest document edit (the latest
    /// scheduled compute landed). While `false` — the debounce/run window
    /// of a slider drag — the Loading tab renders its synchronous W&B
    /// preview instead of the stale report.
    computed_current: bool,
}

/// The tabbed right panel of planning mode.
pub struct ContextPanel {
    app_state: Entity<AppState>,
    /// The shared planning-chrome insets source: the panel's bottom edge
    /// lifts above the profile drawer (design §3.6, one source for all
    /// panels — see [`insets`]).
    profile_drawer: Entity<ProfileDrawer>,
    active_tab: ContextTab,
    anim: PanelAnimation,
    /// TAF expansion of the Inspect tab's airport cards (the explorer
    /// panel keeps its own flag on `RootView`).
    inspect_taf_expanded: bool,
    /// Expanded TAF stations on the Weather tab, by ICAO id.
    expanded_tafs: HashSet<String>,
    /// Expanded raw-text blocks on the Briefing tab, by NOTAM id.
    expanded_notams: HashSet<String>,
    /// Loading tab editor state (rebuilt when the aircraft changes).
    loading: LoadingInputs,
    /// Fuel tab policy editor state.
    policy: PolicyInputs,
    _subscriptions: Vec<Subscription>,
}

impl ContextPanel {
    pub fn new(
        app_state: Entity<AppState>,
        profile_drawer: Entity<ProfileDrawer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&app_state, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app_state, window, Self::on_app_state_event),
            // Drawer chrome changes move the panel's bottom inset.
            cx.observe(&profile_drawer, |_, _, cx| cx.notify()),
        ];
        let loading = LoadingInputs::new(window, cx);
        let policy = PolicyInputs::new(window, cx);
        let mut this = Self {
            app_state,
            profile_drawer,
            active_tab: ContextTab::default(),
            anim: PanelAnimation::default(),
            inspect_taf_expanded: false,
            expanded_tafs: HashSet::new(),
            expanded_notams: HashSet::new(),
            loading,
            policy,
            _subscriptions: subscriptions,
        };
        // Defensive: a flight could already be open by construction time.
        if this.app_state.read(cx).planning_mode() {
            this.anim.open_requested();
            this.sync_inputs(window, cx);
        }
        this
    }

    /// Whether the panel occupies the right slot (open or animating out)
    /// — `RootView` suppresses the explorer info panel while it does.
    pub fn is_mounted(&self) -> bool {
        self.anim.visibility() != PanelVisibility::Closed
    }

    /// The one funnel for tab-side document edits: routes through
    /// [`AppState::edit_flight_doc`], so dirty tracking and the debounced
    /// recompute flow automatically.
    fn commit_doc_edit(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut FlightDoc) -> bool,
    ) {
        self.app_state.update(cx, |state, cx| {
            state.edit_flight_doc(cx, edit);
        });
    }

    fn on_app_state_event(
        &mut self,
        app_state: &Entity<AppState>,
        event: &AppStateEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::FlightOpened => {
                self.anim.open_requested();
                self.expanded_tafs.clear();
                self.expanded_notams.clear();
                self.sync_inputs(window, cx);
                cx.notify();
            }
            AppStateEvent::FlightClosed => {
                if let Some(epoch) = self.anim.close_requested() {
                    self.schedule_unmount(epoch, cx);
                }
                cx.notify();
            }
            AppStateEvent::FlightChanged | AppStateEvent::FlightComputed => {
                self.sync_inputs(window, cx);
                cx.notify();
            }
            AppStateEvent::SelectionChanged => {
                // A fresh map selection pulls the panel to Inspect (the
                // click means "show me this"); clearing stays put.
                let selection_is_empty = app_state.read(cx).selection.is_empty();
                self.active_tab = tab_after_selection_change(self.active_tab, selection_is_empty);
                cx.notify();
            }
            // A W&B/Fuel/NOTAM badge navigated here (design §3.1); the
            // profile focus belongs to the drawer.
            AppStateEvent::PlanningFocusRequested(focus) => {
                let tab = match focus {
                    PlanningFocus::Loading => Some(ContextTab::Loading),
                    PlanningFocus::Fuel => Some(ContextTab::Fuel),
                    PlanningFocus::Briefing => Some(ContextTab::Briefing),
                    PlanningFocus::Profile { .. } => None,
                };
                if let Some(tab) = tab {
                    self.active_tab = tab;
                    cx.notify();
                }
            }
            // NOTAM fetch state / relevance list moved — the Briefing tab
            // and its snapshot header re-render.
            AppStateEvent::BriefingChanged
            | AppStateEvent::WeatherUpdated
            | AppStateEvent::StationsLoaded
            | AppStateEvent::DataReloaded => cx.notify(),
            _ => {}
        }
    }

    /// Pushes the document state into the loading/policy inputs
    /// (focus/epsilon-guarded; rebuilds the station rows when the aircraft
    /// profile changed).
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((doc, aircraft)) = ({
            let state = self.app_state.read(cx);
            state
                .flight
                .as_ref()
                .map(|f| (f.doc.clone(), state.flight_aircraft().cloned()))
        }) else {
            return;
        };
        self.loading.sync(aircraft.as_ref(), &doc, window, cx);
        self.policy.sync(&doc.fuel_policy, window, cx);
    }

    /// Unmount timer twin of the info panel's (same epoch guard — see
    /// `RootView::schedule_panel_unmount`).
    fn schedule_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.anim.animation_done(epoch) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Per-frame snapshot of the open flight for the planning tabs.
    fn flight_view(&self, cx: &Context<Self>) -> Option<FlightView> {
        let state = self.app_state.read(cx);
        let flight = state.flight.as_ref()?;
        let compute_hint = match &flight.compute_state {
            ComputeState::Pending => Some("Computing…".to_owned()),
            ComputeState::Computed => None,
            ComputeState::NotComputable(reason) => Some(format!("Plan incomplete: {reason}.")),
            ComputeState::Failed(error) => Some(format!("Compute failed: {error}")),
        };
        Some(FlightView {
            doc: flight.doc.clone(),
            aircraft: state.flight_aircraft().cloned(),
            ladder: flight.computed.as_ref().map(|c| c.fuel),
            computed: flight.computed.clone(),
            briefing: flight.briefing.clone(),
            compute_hint,
            computed_current: flight.compute_generation.is_current(),
        })
    }

    /// The Inspect tab: the explorer's selection cards, unchanged.
    fn render_inspect_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.app_state.read(cx);
        let features = state.selection.clone();
        if features.is_empty() {
            return card(cx)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Click the map to inspect airspaces, airports, navaids…"),
                )
                .into_any_element();
        }
        let weather = info_panel::station_weather(state, &features);
        let map_theme = MapTheme::by_id(state.map_theme_id).unwrap_or_default();

        let view = cx.entity().downgrade();
        let on_toggle_taf: CardCallback = Rc::new(move |_, cx| {
            view.update(cx, |this, cx| {
                this.inspect_taf_expanded = !this.inspect_taf_expanded;
                cx.notify();
            })
            .ok();
        });

        v_flex()
            .gap_3()
            .children(info_panel::selection_cards(
                &features,
                &weather,
                &map_theme,
                self.inspect_taf_expanded,
                &on_toggle_taf,
                cx,
            ))
            .into_any_element()
    }
}

impl Render for ContextPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visibility = self.anim.visibility();
        if visibility == PanelVisibility::Closed {
            // Out of planning mode: occupy nothing (the element sits in
            // the map container's flow).
            return div().absolute().into_any_element();
        }
        let chrome_insets = self.profile_drawer.read(cx).chrome_insets(window);

        let flight_view = self.flight_view(cx);
        let content: AnyElement = match (self.active_tab, &flight_view) {
            (ContextTab::Inspect, _) => self.render_inspect_tab(cx),
            // Mid exit animation the flight is already gone — keep the
            // frame, fade with empty content.
            (_, None) => div().into_any_element(),
            (ContextTab::Weather, Some(flight)) => weather::render_weather_tab(self, flight, cx),
            (ContextTab::Loading, Some(flight)) => loading::render_loading_tab(self, flight, cx),
            (ContextTab::Fuel, Some(flight)) => fuel::render_fuel_tab(self, flight, cx),
            (ContextTab::Briefing, Some(flight)) => {
                briefing::render_briefing_tab(self, flight, cx)
            }
        };

        let tab_bar = TabBar::new("context-tabs")
            .segmented()
            .w_full()
            .selected_index(self.active_tab.index())
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                if let Some(tab) = ContextTab::from_index(*index) {
                    this.active_tab = tab;
                    cx.notify();
                }
            }))
            .children(
                ContextTab::ALL
                    .into_iter()
                    .map(|tab| Tab::new().label(tab.label()).flex_1()),
            );

        let panel = v_flex()
            .occlude()
            .relative()
            .size_full()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background.opacity(0.78))
            .backdrop_blur(px(18.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(tab_bar),
            )
            .child(
                div()
                    // Per-tab element id: each tab keeps its own scroll
                    // offset and the swap resets cleanly.
                    .id(("context-tab-content", self.active_tab.index()))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(content),
            );

        // The info panel's enter/exit recipe, keyed by the shared
        // animation machine's counters. The slide is a relative offset —
        // the outer frame below owns the absolute insets, with its bottom
        // lifted above the profile drawer by the shared planning-chrome
        // source (animated by the drawer's own toggle; one element cannot
        // carry two `with_animation`s).
        let panel: AnyElement = match visibility {
            PanelVisibility::Closed => unreachable!("handled above"),
            PanelVisibility::Open => panel
                .with_animation(
                    ("context-panel-enter", self.anim.open_generation()),
                    Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
                    |panel, delta| {
                        panel
                            .left(px(PANEL_SLIDE_PX * (1. - delta)))
                            .opacity(delta)
                    },
                )
                .into_any_element(),
            PanelVisibility::Closing => panel
                .with_animation(
                    ("context-panel-exit", self.anim.close_epoch()),
                    Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
                    |panel, delta| {
                        panel
                            .left(px(PANEL_SLIDE_PX * delta))
                            .opacity(1. - delta)
                    },
                )
                .into_any_element(),
        };

        let frame = div()
            .absolute()
            .top_3()
            .right(px(PANEL_INSET_PX))
            .w(px(PANEL_WIDTH_PX))
            .child(panel);
        insets::lift_panel_bottom(frame, &chrome_insets, "context-panel-lift")
    }
}

/// Shared "this tab has nothing to show yet" card: a primary line plus the
/// compute-state hint when there is one.
fn not_computable_hint(
    primary: &str,
    hint: Option<String>,
    cx: &gpui::App,
) -> AnyElement {
    card(cx)
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(primary.to_string()),
        )
        .children(hint.map(|hint| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.8))
                .child(hint)
        }))
        .into_any_element()
}

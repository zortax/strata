//! Stateful core of the flight panel: the input/select/picker entities,
//! their subscriptions into the [`AppState`] document-mutation API, and
//! the snapshot the panel renders from.
//!
//! The snapshot (doc + computed outputs) is refreshed on every flight
//! event and deliberately outlives [`AppState::flight`]: during the exit
//! animation the live flight is already `None`, exactly like the info
//! panel's `panel_selection`. Field sync follows one rule: **focused
//! inputs are never overwritten** (the user is typing; the document
//! already has their text via the change handlers), unfocused ones are
//! brought to the canonical text.
//!
//! [`AppState`]: crate::state::AppState

use std::rc::Rc;
use std::sync::Arc;

use chrono::Utc;
use gpui::{AppContext as _, Context, Entity, Focusable as _, SharedString, Subscription, Window};
use gpui_component::calendar::Date;
use gpui_component::date_picker::{DatePickerEvent, DatePickerState};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::select::{SelectEvent, SelectItem, SelectState};
use strata_plan::FlightDoc;
use strata_plan::compute::ComputedFlight;

use crate::app::RootView;
use crate::app::panel_animation::PANEL_UNMOUNT_DELAY;
use crate::state::{AppStateEvent, ComputeState};

use super::model::{
    self, AltitudeEdit, MANAGE_AIRCRAFT_VALUE, aircraft_choice_title, altitude_text, time_text,
};

/// Callback seam for the "Manage aircraft…" select item: the panel only
/// reports the request; opening the aircraft-manager dialog is wired by
/// `RootView` (the dialog belongs to its own workflow/module).
pub type ManageAircraftSeam = Rc<dyn Fn(&mut RootView, &mut Window, &mut Context<RootView>)>;

/// One aircraft-selector entry: a profile (value = its id) or the
/// trailing "Manage aircraft…" action item
/// ([`MANAGE_AIRCRAFT_VALUE`]).
#[derive(Clone)]
pub struct AircraftChoice {
    title: SharedString,
    value: SharedString,
}

impl SelectItem for AircraftChoice {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SharedString {
        &self.value
    }
}

/// What the panel renders: the last seen document and computed outputs.
pub struct FlightSnapshot {
    pub doc: FlightDoc,
    pub computed: Option<Arc<ComputedFlight>>,
    pub compute_state: ComputeState,
}

/// Entities and subscriptions backing the mounted flight panel. Created
/// when planning mode starts, dropped after the exit animation.
pub struct FlightPanelState {
    pub snapshot: FlightSnapshot,
    pub name_input: Entity<InputState>,
    pub cruise_input: Entity<InputState>,
    pub time_input: Entity<InputState>,
    pub date_picker: Entity<DatePickerState>,
    pub aircraft_select: Entity<SelectState<Vec<AircraftChoice>>>,
    /// One compact altitude field per leg (`0..route.len()-1`), kept in
    /// lockstep with the route by [`RootView::sync_flight_panel_fields`].
    pub leg_altitude_inputs: Vec<Entity<InputState>>,
    on_manage_aircraft: ManageAircraftSeam,
    /// Select-items fingerprint: the aircraft ids+titles the dropdown was
    /// last built from (rebuilds only on actual library changes; `None` =
    /// never built — an empty library still needs its "Manage aircraft…"
    /// item installed).
    library_key: Option<Vec<String>>,
    /// Per-leg input subscriptions, parallel to `leg_altitude_inputs`.
    leg_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl FlightPanelState {
    fn new(
        on_manage_aircraft: ManageAircraftSeam,
        window: &mut Window,
        cx: &mut Context<RootView>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Flight name"));
        let cruise_input = cx.new(|cx| InputState::new(window, cx).placeholder("3000"));
        let time_input = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM"));
        let date_picker = cx.new(|cx| DatePickerState::new(window, cx).date_format("%Y-%m-%d"));
        let aircraft_select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&name_input, window, RootView::on_flight_name_input),
            cx.subscribe_in(&cruise_input, window, RootView::on_flight_cruise_input),
            cx.subscribe_in(&time_input, window, RootView::on_flight_time_input),
            cx.subscribe_in(&date_picker, window, RootView::on_flight_date_picker),
            cx.subscribe_in(&aircraft_select, window, RootView::on_flight_aircraft_select),
        ];

        Self {
            snapshot: FlightSnapshot {
                doc: FlightDoc::default(),
                computed: None,
                compute_state: ComputeState::Pending,
            },
            name_input,
            cruise_input,
            time_input,
            date_picker,
            aircraft_select,
            leg_altitude_inputs: Vec::new(),
            on_manage_aircraft,
            library_key: None,
            leg_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        }
    }
}

// --- RootView lifecycle + handlers -------------------------------------------

impl RootView {
    /// Routes the flight events into the panel: mounts/creates on open,
    /// re-syncs fields on document changes, refreshes computed outputs,
    /// and plays the exit animation (+ delayed teardown) on close.
    pub(crate) fn drive_flight_panel(
        &mut self,
        event: &AppStateEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppStateEvent::FlightOpened => {
                if self.flight_panel.is_none() {
                    self.flight_panel = Some(FlightPanelState::new(
                        Rc::new(Self::open_aircraft_manager),
                        window,
                        cx,
                    ));
                }
                self.refresh_flight_panel_snapshot(cx);
                self.sync_flight_panel_aircraft_items(window, cx);
                self.sync_flight_panel_fields(true, window, cx);
                self.flight_panel_anim.open_requested();
            }
            AppStateEvent::FlightChanged => {
                self.refresh_flight_panel_snapshot(cx);
                self.sync_flight_panel_fields(false, window, cx);
            }
            AppStateEvent::FlightComputed => {
                self.refresh_flight_panel_snapshot(cx);
            }
            AppStateEvent::FlightClosed => {
                if let Some(epoch) = self.flight_panel_anim.close_requested() {
                    self.schedule_flight_panel_unmount(epoch, cx);
                }
            }
            AppStateEvent::AircraftLibraryChanged => {
                self.sync_flight_panel_aircraft_items(window, cx);
            }
            _ => {}
        }
    }

    /// Flight-panel twin of `RootView::schedule_panel_unmount`: drops the
    /// panel state once the exit animation has played; the epoch guard
    /// voids the timer when the panel re-opened meanwhile.
    fn schedule_flight_panel_unmount(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANEL_UNMOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                if this.flight_panel_anim.animation_done(epoch) {
                    this.flight_panel = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Copies the open flight into the panel's render snapshot (a closed
    /// flight keeps the last snapshot for the exit animation).
    fn refresh_flight_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.flight_panel else {
            return;
        };
        let state = self.app_state.read(cx);
        let Some(flight) = &state.flight else {
            return;
        };
        panel.snapshot = FlightSnapshot {
            doc: flight.doc.clone(),
            computed: flight.computed.clone(),
            compute_state: flight.compute_state.clone(),
        };
    }

    /// Brings every header/leg field to the document's canonical text.
    /// `force` overwrites focused fields too (fresh mounts); otherwise the
    /// field being typed in is left alone.
    fn sync_flight_panel_fields(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.flight_panel else {
            return;
        };
        let doc = panel.snapshot.doc.clone();
        let name_input = panel.name_input.clone();
        let cruise_input = panel.cruise_input.clone();
        let time_input = panel.time_input.clone();
        let date_picker = panel.date_picker.clone();
        let aircraft_select = panel.aircraft_select.clone();

        set_input_text(&name_input, &doc.name, force, window, cx);
        set_input_text(
            &cruise_input,
            &altitude_text(doc.cruise_altitude),
            force,
            window,
            cx,
        );
        set_input_text(
            &time_input,
            &time_text(doc.departure_time),
            force,
            window,
            cx,
        );
        date_picker.update(cx, |picker, cx| {
            let date = Date::Single(doc.departure_time.map(|t| t.date_naive()));
            if picker.date() != date {
                picker.set_date(date, window, cx);
            }
        });

        // Selected aircraft (the items themselves sync on library events).
        aircraft_select.update(cx, |select, cx| match &doc.aircraft_id {
            Some(id) => select.set_selected_value(&SharedString::from(id.to_string()), window, cx),
            None => select.set_selected_index(None, window, cx),
        });

        self.sync_flight_panel_leg_inputs(&doc, force, window, cx);
    }

    /// Keeps one altitude field per leg, in route order: grows/shrinks the
    /// vec, then syncs text (canonical altitude) and placeholder (the
    /// inherited cruise value) on every unfocused field.
    fn sync_flight_panel_leg_inputs(
        &mut self,
        doc: &FlightDoc,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let legs = doc.route.len().saturating_sub(1);
        // Grow: each new field subscribes with its (stable) leg index.
        while self
            .flight_panel
            .as_ref()
            .is_some_and(|p| p.leg_altitude_inputs.len() < legs)
        {
            let index = self
                .flight_panel
                .as_ref()
                .map_or(0, |p| p.leg_altitude_inputs.len());
            let input = cx.new(|cx| InputState::new(window, cx));
            let subscription = cx.subscribe_in(
                &input,
                window,
                move |this: &mut RootView, input, event, window, cx| {
                    this.on_flight_leg_altitude_input(index, input, event, window, cx);
                },
            );
            if let Some(panel) = &mut self.flight_panel {
                panel.leg_altitude_inputs.push(input);
                panel.leg_subscriptions.push(subscription);
            }
        }
        let Some(panel) = &mut self.flight_panel else {
            return;
        };
        panel.leg_altitude_inputs.truncate(legs);
        panel.leg_subscriptions.truncate(legs);

        // The placeholder shows what an empty field inherits — the
        // flight's cruise altitude.
        let placeholder = match doc.cruise_altitude {
            Some(cruise) => model::altitude_label(cruise),
            None => "—".to_owned(),
        };
        let inputs = panel.leg_altitude_inputs.clone();
        for (index, input) in inputs.iter().enumerate() {
            let target = altitude_text(doc.route.get(index).and_then(|w| w.leg_altitude));
            set_input_text(input, &target, force, window, cx);
            input.update(cx, |input, cx| {
                input.set_placeholder(placeholder.clone(), window, cx);
            });
        }
    }

    /// Rebuilds the aircraft dropdown from the library (plus the trailing
    /// "Manage aircraft…" action item) when the library actually changed,
    /// then re-selects the document's aircraft.
    fn sync_flight_panel_aircraft_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.flight_panel else {
            return;
        };
        let profiles = &self.app_state.read(cx).aircraft_library;
        let key: Vec<String> = profiles
            .iter()
            .map(|p| format!("{}\u{1f}{}", p.id, aircraft_choice_title(p)))
            .collect();
        let mut items: Vec<AircraftChoice> = profiles
            .iter()
            .map(|p| AircraftChoice {
                title: aircraft_choice_title(p).into(),
                value: p.id.to_string().into(),
            })
            .collect();
        items.push(AircraftChoice {
            title: "Manage aircraft…".into(),
            value: MANAGE_AIRCRAFT_VALUE.into(),
        });
        let select = panel.aircraft_select.clone();
        let selected = panel.snapshot.doc.aircraft_id.clone();
        if panel.library_key.as_ref() != Some(&key) {
            if let Some(panel) = &mut self.flight_panel {
                panel.library_key = Some(key);
            }
            select.update(cx, |select, cx| {
                select.set_items(items, window, cx);
                match &selected {
                    Some(id) => {
                        select.set_selected_value(&SharedString::from(id.to_string()), window, cx)
                    }
                    None => select.set_selected_index(None, window, cx),
                }
            });
        }
    }

    // --- field handlers -----------------------------------------------------

    /// Name edits apply live — the title-bar strip and the library show
    /// the name as it is typed.
    fn on_flight_name_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = event {
            let name = input.read(cx).value().to_string();
            self.app_state.update(cx, |state, cx| {
                state.set_flight_name(name, cx);
            });
        }
    }

    /// Cruise quick-set: valid feet/FL values apply live (the debounced
    /// compute makes this cheap), an emptied field clears the default;
    /// blur re-canonicalizes whatever the field ended up holding.
    fn on_flight_cruise_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => match model::parse_altitude(&input.read(cx).value()) {
                AltitudeEdit::Set(altitude) => {
                    self.app_state.update(cx, |state, cx| {
                        state.set_cruise_altitude(Some(altitude), cx);
                    });
                }
                AltitudeEdit::Clear => {
                    self.app_state.update(cx, |state, cx| {
                        state.set_cruise_altitude(None, cx);
                    });
                }
                AltitudeEdit::Invalid => {}
            },
            InputEvent::Blur => {
                let canonical = altitude_text(
                    self.app_state
                        .read(cx)
                        .flight
                        .as_ref()
                        .and_then(|f| f.doc.cruise_altitude),
                );
                set_input_text(input, &canonical, true, window, cx);
            }
            _ => {}
        }
    }

    /// Per-leg altitude field (leg `index` = from waypoint `index` to its
    /// successor). Same live-apply/blur-canonicalize contract as cruise.
    fn on_flight_leg_altitude_input(
        &mut self,
        index: usize,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => match model::parse_altitude(&input.read(cx).value()) {
                AltitudeEdit::Set(altitude) => {
                    self.app_state.update(cx, |state, cx| {
                        state.set_leg_altitude(index, Some(altitude), cx);
                    });
                }
                AltitudeEdit::Clear => {
                    self.app_state.update(cx, |state, cx| {
                        state.set_leg_altitude(index, None, cx);
                    });
                }
                AltitudeEdit::Invalid => {}
            },
            InputEvent::Blur => {
                let canonical = altitude_text(
                    self.app_state
                        .read(cx)
                        .flight
                        .as_ref()
                        .and_then(|f| f.doc.route.get(index))
                        .and_then(|w| w.leg_altitude),
                );
                set_input_text(input, &canonical, true, window, cx);
            }
            _ => {}
        }
    }

    /// Departure *time* (UTC) commits on Enter/blur: live-applying a
    /// half-typed "1" as 01:00 would jump the ETAs around. Invalid text
    /// reverts to the document on blur.
    fn on_flight_time_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                let current = self
                    .app_state
                    .read(cx)
                    .flight
                    .as_ref()
                    .and_then(|f| f.doc.departure_time);
                match model::parse_time_utc(&input.read(cx).value()) {
                    Some(Some(time)) => {
                        let departure = model::departure_with_time(
                            current,
                            time,
                            Utc::now().date_naive(),
                        );
                        self.app_state.update(cx, |state, cx| {
                            state.set_departure_time(Some(departure), cx);
                        });
                    }
                    Some(None) => {
                        self.app_state.update(cx, |state, cx| {
                            state.set_departure_time(None, cx);
                        });
                    }
                    None => {}
                }
                // Canonicalize (also reverts unparseable text).
                let canonical = time_text(
                    self.app_state
                        .read(cx)
                        .flight
                        .as_ref()
                        .and_then(|f| f.doc.departure_time),
                );
                set_input_text(input, &canonical, true, window, cx);
            }
            _ => {}
        }
    }

    /// Departure *date* (UTC) from the picker; keeps the time-of-day.
    fn on_flight_date_picker(
        &mut self,
        _picker: &Entity<DatePickerState>,
        event: &DatePickerEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let DatePickerEvent::Change(date) = event;
        let current = self
            .app_state
            .read(cx)
            .flight
            .as_ref()
            .and_then(|f| f.doc.departure_time);
        match date {
            Date::Single(Some(date)) => {
                let departure = model::departure_with_date(current, *date);
                self.app_state.update(cx, |state, cx| {
                    state.set_departure_time(Some(departure), cx);
                });
            }
            Date::Single(None) => {
                self.app_state.update(cx, |state, cx| {
                    state.set_departure_time(None, cx);
                });
            }
            Date::Range(..) => {}
        }
    }

    /// Aircraft selection — a profile id, or the "Manage aircraft…"
    /// action item, which fires the dialog seam and restores the real
    /// selection.
    fn on_flight_aircraft_select(
        &mut self,
        select: &Entity<SelectState<Vec<AircraftChoice>>>,
        event: &SelectEvent<Vec<AircraftChoice>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(value) = event;
        let Some(value) = value else {
            return;
        };
        if value.as_ref() == MANAGE_AIRCRAFT_VALUE {
            // Restore the trigger label before the dialog opens.
            let selected = self
                .app_state
                .read(cx)
                .flight
                .as_ref()
                .and_then(|f| f.doc.aircraft_id.clone());
            select.update(cx, |select, cx| match &selected {
                Some(id) => {
                    select.set_selected_value(&SharedString::from(id.to_string()), window, cx)
                }
                None => select.set_selected_index(None, window, cx),
            });
            let seam = self
                .flight_panel
                .as_ref()
                .map(|panel| panel.on_manage_aircraft.clone());
            if let Some(seam) = seam {
                seam(self, window, cx);
            }
            return;
        }
        let id = self
            .app_state
            .read(cx)
            .aircraft_library
            .iter()
            .find(|p| p.id.to_string() == value.as_ref())
            .map(|p| p.id.clone());
        if let Some(id) = id {
            self.app_state.update(cx, |state, cx| {
                state.set_flight_aircraft(Some(id), cx);
            });
        }
    }
}

/// Sets `input` to `text` unless the user is typing in it (`force`
/// overrides, e.g. blur-canonicalization or a fresh mount) — and never
/// touches an input that already shows the target (no event churn, no
/// cursor jumps).
fn set_input_text(
    input: &Entity<InputState>,
    text: &str,
    force: bool,
    window: &mut Window,
    cx: &mut Context<RootView>,
) {
    if !force && input.read(cx).focus_handle(cx).is_focused(window) {
        return;
    }
    if input.read(cx).value().as_ref() == text {
        return;
    }
    let text = text.to_owned();
    input.update(cx, |input, cx| input.set_value(text, window, cx));
}

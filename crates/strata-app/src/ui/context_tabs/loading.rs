//! The Loading tab (design §3.4 "Loading"): per-station W&B loading editor
//! (slider + numeric entry per station), the fuel quantity entry honoring
//! tank tabs, live mass/CG totals, and the CG envelope plot
//! ([`super::envelope`]).
//!
//! Edits are pure [`FlightDoc`] mutations on the loading scenario; the
//! debounced recompute flows through `AppState::edit_flight_doc`
//! automatically. The totals and the envelope do **not** wait for it:
//! while the latest compute is in flight (a slider drag keeps resetting
//! the debounce) they render from [`wb_preview`] — the same
//! `wb::compute_weight_balance` call the pipeline makes, run synchronously
//! over the in-edit loading values — and the authoritative [`WbReport`]
//! takes over when it lands, with identical numbers (the agreement test
//! pins that).

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Styled as _, Subscription, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, NumberInput, NumberInputEvent, StepAction},
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use strata_plan::AircraftProfile;
use strata_plan::aircraft::{AircraftId, StationKind, WbStation};
use strata_plan::flight::{FlightDoc, LoadingScenario, StationLoad};
use strata_plan::fuel::FuelLadder;
use strata_plan::units::{Kilograms, Liters};
use strata_plan::wb::{self, WbReport, WbStateKind};

use crate::ui::info_panel::{card, section};

use super::envelope::envelope_plot;
use super::fuel::{format_number, sync_number_input};
use super::{ContextPanel, FlightView, not_computable_hint};

/// Slider span for stations without a published structural limit.
const DEFAULT_STATION_MAX_KG: f64 = 200.0;
/// Fuel slider span for profiles without a usable capacity entered.
const DEFAULT_FUEL_MAX_L: f64 = 200.0;

// --- pure document mutations -----------------------------------------------------

/// Sets the mass loaded at `station` in the active loading scenario.
/// Zero (or negative, clamped) mass removes the entry — an absent station
/// load and an explicit 0 kg are the same scenario. Returns whether the
/// document changed.
pub(crate) fn set_station_load(doc: &mut FlightDoc, station: &str, mass_kg: f64) -> bool {
    let mass = mass_kg.max(0.0);
    let loads = &mut doc.loading.station_loads;
    if mass <= 0.0 {
        let before = loads.len();
        loads.retain(|l| l.station != station);
        return loads.len() != before;
    }
    match loads.iter_mut().find(|l| l.station == station) {
        Some(load) => {
            if load.mass.0 == mass {
                false
            } else {
                load.mass = Kilograms(mass);
                true
            }
        }
        None => {
            loads.push(StationLoad {
                station: station.to_owned(),
                mass: Kilograms(mass),
            });
            true
        }
    }
}

/// Sets the fuel at engine start, clamped into `0 ..= usable` (when the
/// profile has a usable capacity entered).
pub(crate) fn set_fuel_liters(doc: &mut FlightDoc, liters: f64, usable: Option<f64>) -> bool {
    let mut fuel = liters.max(0.0);
    if let Some(usable) = usable.filter(|u| *u > 0.0) {
        fuel = fuel.min(usable);
    }
    if doc.loading.fuel.0 == fuel {
        return false;
    }
    doc.loading.fuel = Liters(fuel);
    true
}

/// Mass currently loaded at `station` (0 when absent).
pub(crate) fn station_mass(doc: &FlightDoc, station: &str) -> f64 {
    doc.loading
        .station_loads
        .iter()
        .find(|l| l.station == station)
        .map_or(0.0, |l| l.mass.0)
}

/// Synchronous W&B preview over the in-edit loading values — the exact
/// [`wb::compute_weight_balance`] call the compute pipeline makes, fed
/// with the last landed ladder's taxi/trip rungs. Loading edits never move
/// those rungs (taxi is policy minutes × profile flow, trip comes from the
/// phase plan), so the debounced pipeline result that eventually lands is
/// identical — `preview_agrees_with_the_full_compute_pipeline` pins that.
///
/// `None` without a ladder (nothing landed yet) or when the scenario does
/// not compute (unknown station mid-profile-edit, no envelope); callers
/// fall back to the authoritative report. Microsecond-cheap: safe to run
/// on every render of a drag burst.
pub(crate) fn wb_preview(
    aircraft: &AircraftProfile,
    loading: &LoadingScenario,
    ladder: Option<&FuelLadder>,
) -> Option<WbReport> {
    let ladder = ladder?;
    wb::compute_weight_balance(aircraft, loading, ladder.taxi, ladder.trip).ok()
}

// --- stateful inputs (owned by ContextPanel) ---------------------------------------

/// One station's editor row state. (The slider's range already carries the
/// station's structural limit.)
pub(crate) struct StationRow {
    pub name: String,
    pub slider: Entity<SliderState>,
    pub input: Entity<InputState>,
}

/// The Loading tab's stateful inputs, rebuilt when the flight's aircraft
/// profile changes (station set + slider ranges are profile-derived).
pub(crate) struct LoadingInputs {
    /// Profile the rows were built for (`None` = none / not built yet).
    aircraft: Option<AircraftId>,
    pub rows: Vec<StationRow>,
    pub fuel_slider: Entity<SliderState>,
    pub fuel_input: Entity<InputState>,
    pub fuel_max: f64,
    subscriptions: Vec<Subscription>,
}

impl LoadingInputs {
    /// Empty editor (no aircraft yet); [`Self::sync`] builds the real rows.
    pub fn new(window: &mut Window, cx: &mut Context<ContextPanel>) -> Self {
        let mut this = Self {
            aircraft: None,
            rows: Vec::new(),
            fuel_slider: cx.new(|_| SliderState::new().min(0.).max(DEFAULT_FUEL_MAX_L as f32)),
            fuel_input: cx.new(|cx| InputState::new(window, cx)),
            fuel_max: DEFAULT_FUEL_MAX_L,
            subscriptions: Vec::new(),
        };
        this.wire_fuel(window, cx);
        this
    }

    /// Rebuilds rows when the profile changed, then pushes the document's
    /// loading values into every input (focus/epsilon-guarded).
    pub fn sync(
        &mut self,
        profile: Option<&AircraftProfile>,
        doc: &FlightDoc,
        window: &mut Window,
        cx: &mut Context<ContextPanel>,
    ) {
        if profile.map(|p| &p.id) != self.aircraft.as_ref() {
            self.rebuild(profile, window, cx);
        }
        for row in &self.rows {
            let mass = station_mass(doc, &row.name);
            sync_number_input(&row.input, mass, window, cx);
            sync_slider(&row.slider, mass, window, cx);
        }
        sync_number_input(&self.fuel_input, doc.loading.fuel.0, window, cx);
        sync_slider(&self.fuel_slider, doc.loading.fuel.0, window, cx);
    }

    /// Recreates every entity + subscription for `profile`'s stations.
    fn rebuild(
        &mut self,
        profile: Option<&AircraftProfile>,
        window: &mut Window,
        cx: &mut Context<ContextPanel>,
    ) {
        self.subscriptions.clear();
        self.rows.clear();
        self.aircraft = profile.map(|p| p.id.clone());

        let stations: Vec<&WbStation> = profile
            .map(|p| {
                p.weight_balance
                    .stations
                    .iter()
                    .filter(|s| s.kind != StationKind::Fuel)
                    .collect()
            })
            .unwrap_or_default();

        for station in stations {
            let name = station.name.clone();
            let max_kg = station.max_load.map_or(DEFAULT_STATION_MAX_KG, |m| m.0.max(1.0));
            let slider = cx.new(|_| SliderState::new().min(0.).max(max_kg as f32).step(1.));
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("0"));

            let station_name = name.clone();
            self.subscriptions.push(cx.subscribe_in(
                &slider,
                window,
                move |this: &mut ContextPanel, _, event: &SliderEvent, _, cx| {
                    if let SliderEvent::Change(value) = event {
                        let mass = f64::from(value.end());
                        let station = station_name.clone();
                        this.commit_doc_edit(cx, move |doc| set_station_load(doc, &station, mass));
                    }
                },
            ));
            let station_name = name.clone();
            self.subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut ContextPanel, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change)
                        && let Ok(mass) = input.read(cx).value().trim().parse::<f64>()
                    {
                        let station = station_name.clone();
                        this.commit_doc_edit(cx, move |doc| {
                            set_station_load(doc, &station, mass.max(0.0))
                        });
                    }
                },
            ));
            let station_name = name.clone();
            self.subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut ContextPanel, input, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    let typed = input.read(cx).value().trim().parse::<f64>().ok();
                    let current = typed.unwrap_or_else(|| {
                        this.app_state
                            .read(cx)
                            .flight
                            .as_ref()
                            .map_or(0.0, |f| station_mass(&f.doc, &station_name))
                    });
                    let next = step_value(current, *action, max_kg);
                    input.update(cx, |input, cx| {
                        input.set_value(format_number(next), window, cx);
                    });
                    let station = station_name.clone();
                    this.commit_doc_edit(cx, move |doc| set_station_load(doc, &station, next));
                },
            ));

            self.rows.push(StationRow {
                name,
                slider,
                input,
            });
        }

        // Fuel slider/input spans the profile's usable capacity.
        self.fuel_max = profile
            .map(|p| p.fuel.usable.0)
            .filter(|u| *u > 0.0)
            .unwrap_or(DEFAULT_FUEL_MAX_L);
        self.fuel_slider =
            cx.new(|_| SliderState::new().min(0.).max(self.fuel_max as f32).step(1.));
        self.fuel_input = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        self.wire_fuel(window, cx);
    }

    /// Subscriptions of the fuel slider/input pair (called for the initial
    /// entities and after every rebuild).
    fn wire_fuel(&mut self, window: &mut Window, cx: &mut Context<ContextPanel>) {
        let fuel_max = self.fuel_max;
        self.subscriptions.push(cx.subscribe_in(
            &self.fuel_slider,
            window,
            move |this: &mut ContextPanel, _, event: &SliderEvent, _, cx| {
                if let SliderEvent::Change(value) = event {
                    let liters = f64::from(value.end());
                    this.commit_doc_edit(cx, move |doc| {
                        set_fuel_liters(doc, liters, Some(fuel_max))
                    });
                }
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            &self.fuel_input,
            window,
            move |this: &mut ContextPanel, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change)
                    && let Ok(liters) = input.read(cx).value().trim().parse::<f64>()
                {
                    this.commit_doc_edit(cx, move |doc| {
                        set_fuel_liters(doc, liters, Some(fuel_max))
                    });
                }
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            &self.fuel_input,
            window,
            move |this: &mut ContextPanel, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let typed = input.read(cx).value().trim().parse::<f64>().ok();
                let current = typed.unwrap_or_else(|| {
                    this.app_state
                        .read(cx)
                        .flight
                        .as_ref()
                        .map_or(0.0, |f| f.doc.loading.fuel.0)
                });
                let next = step_value(current, *action, fuel_max);
                input.update(cx, |input, cx| {
                    input.set_value(format_number(next), window, cx);
                });
                this.commit_doc_edit(cx, move |doc| set_fuel_liters(doc, next, Some(fuel_max)));
            },
        ));
    }
}

/// ±1 steps, clamped into the editor's range.
fn step_value(current: f64, action: StepAction, max: f64) -> f64 {
    match action {
        StepAction::Increment => (current + 1.0).min(max),
        StepAction::Decrement => (current - 1.0).max(0.0),
    }
}

/// Pushes `value` into a slider unless it already shows it (the no-op
/// guard breaking the drag → commit → sync feedback loop).
fn sync_slider(slider: &Entity<SliderState>, value: f64, window: &mut Window, cx: &mut App) {
    let current = f64::from(slider.read(cx).value().end());
    if (current - value).abs() > 0.01 {
        slider.update(cx, |slider, cx| slider.set_value(value as f32, window, cx));
    }
}

// --- rendering -----------------------------------------------------------------------

pub(super) fn render_loading_tab(
    panel: &ContextPanel,
    flight: &FlightView,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let Some(aircraft) = &flight.aircraft else {
        return not_computable_hint(
            "Select an aircraft profile to edit the loading.",
            flight.compute_hint.clone(),
            cx,
        );
    };
    // Live totals during a drag: while the latest compute is still in
    // flight the authoritative report is stale, so render the synchronous
    // preview over the in-edit loading instead. When the compute lands
    // (`computed_current`) the authoritative report takes over with
    // identical numbers — see [`wb_preview`].
    let preview = (!flight.computed_current)
        .then(|| wb_preview(aircraft, &flight.doc.loading, flight.ladder.as_ref()))
        .flatten();
    let report = preview
        .as_ref()
        .or(flight.computed.as_ref().map(|c| &c.weight_balance));

    v_flex()
        .gap_3()
        .child(stations_card(panel, flight, aircraft, cx))
        .child(totals_card(aircraft, report, flight.compute_hint.clone(), cx))
        .child(
            card(cx)
                .child(section("CG envelope", cx))
                .child(envelope_plot(&aircraft.weight_balance.envelope, report, cx)),
        )
        .into_any_element()
}

/// The per-station editor + fuel entry.
fn stations_card(
    panel: &ContextPanel,
    flight: &FlightView,
    aircraft: &AircraftProfile,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let scenario = flight.doc.loading.name.clone();
    let mut el = card(cx).child(
        h_flex()
            .justify_between()
            .child(section("Loading", cx))
            .children((!scenario.is_empty()).then(|| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(scenario)
            })),
    );

    if panel.loading.rows.is_empty() {
        el = el.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("The aircraft profile has no loading stations."),
        );
    }
    for row in &panel.loading.rows {
        el = el.child(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w_0().truncate().text_sm().child(row.name.clone()))
                        .child(
                            div().w(px(116.)).flex_shrink_0().child(
                                NumberInput::new(&row.input).small().suffix(kg_suffix(cx)),
                            ),
                        ),
                )
                .child(Slider::new(&row.slider).horizontal()),
        );
    }

    // Fuel entry honoring the tank tabs: quick-set buttons for "Tabs"
    // (when the profile has them) and "Full".
    let usable = aircraft.fuel.usable.0;
    let tabs = aircraft.fuel.tabs.map(|t| t.0).filter(|t| *t > 0.0);
    el = el.child(
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().text_sm().child("Fuel"))
                    .children(tabs.map(|tabs| {
                        Button::new("fuel-tabs")
                            .ghost()
                            .xsmall()
                            .label(format!("Tabs ({})", format_number(tabs)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.commit_doc_edit(cx, move |doc| {
                                    set_fuel_liters(doc, tabs, None)
                                });
                            }))
                    }))
                    .children((usable > 0.0).then(|| {
                        Button::new("fuel-full")
                            .ghost()
                            .xsmall()
                            .label("Full")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.commit_doc_edit(cx, move |doc| {
                                    set_fuel_liters(doc, usable, None)
                                });
                            }))
                    }))
                    .child(
                        div().w(px(116.)).flex_shrink_0().child(
                            NumberInput::new(&panel.loading.fuel_input)
                                .small()
                                .suffix(liter_suffix(cx)),
                        ),
                    ),
            )
            .child(Slider::new(&panel.loading.fuel_slider).horizontal()),
    );

    el.into_any_element()
}

/// Live totals from the latest computed W&B report.
fn totals_card(
    aircraft: &AircraftProfile,
    report: Option<&WbReport>,
    compute_hint: Option<String>,
    cx: &App,
) -> AnyElement {
    let mut el = card(cx).child(
        h_flex()
            .justify_between()
            .child(section("Mass & CG", cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "MTOW {} kg",
                        format_number(aircraft.weight_balance.max_takeoff.0)
                    )),
            ),
    );
    let Some(report) = report else {
        return el
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(compute_hint.unwrap_or_else(|| "Totals appear once the route computes.".into())),
            )
            .into_any_element();
    };
    for state in &report.states {
        let label = match state.kind {
            WbStateKind::Ramp => "Ramp",
            WbStateKind::Takeoff => "Takeoff",
            WbStateKind::ZeroFuel => "Zero fuel",
            WbStateKind::Landing => "Landing",
        };
        let (dot, note) = if state.within_envelope {
            (cx.theme().success, None)
        } else {
            (cx.theme().danger, Some("out of envelope"))
        };
        el = el.child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().size_2().flex_shrink_0().rounded_full().bg(dot))
                .child(
                    div()
                        .w(px(70.))
                        .flex_shrink_0()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .font_family("monospace")
                        .child(format!("{:.0} kg @ {:.2} m", state.mass.0, state.arm.0)),
                )
                .children(note.map(|note| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(note)
                })),
        );
    }
    el.into_any_element()
}

fn kg_suffix(cx: &App) -> impl IntoElement {
    div()
        .pr_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child("kg")
}

fn liter_suffix(cx: &App) -> impl IntoElement {
    div()
        .pr_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child("L")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use strata_plan::compute::ComputeParams;

    use crate::flight_io::aircraft::example_c172;
    use crate::sources::WindsAloftFrames;
    use crate::state::flight::compute::{ComputeOutcome, run_compute, test_support};

    use super::*;

    /// A ladder carrying only the rungs the preview reads (taxi/trip).
    fn ladder(taxi: f64, trip: f64) -> FuelLadder {
        FuelLadder {
            taxi: Liters(taxi),
            trip: Liters(trip),
            contingency: Liters(0.0),
            alternate: Liters(0.0),
            final_reserve: Liters(0.0),
            extra: Liters(0.0),
            minimum_required: Liters(taxi + trip),
            loaded: Liters(0.0),
            margin: Liters(0.0),
        }
    }

    /// The agreement contract: what the preview renders during a drag is
    /// exactly what the debounced pipeline produces when it lands — same
    /// function, same taxi/trip rungs (loading edits never move those).
    #[test]
    fn preview_agrees_with_the_full_compute_pipeline() {
        let (_dir, store) = test_support::temp_store_with_elevation();
        let aircraft = example_c172();
        let winds = Arc::new(WindsAloftFrames::default());
        let params = ComputeParams::default();

        let mut doc = test_support::two_leg_doc();
        set_station_load(&mut doc, "Front seats", 80.0);
        let (outcome, _) = run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::clone(&store)),
            Arc::clone(&winds),
            &params,
            None,
        );
        let ComputeOutcome::Computed(landed) = outcome else {
            panic!("expected Computed, got {outcome:?}");
        };

        // Landed state: the preview from the landed ladder reproduces the
        // authoritative report exactly (no visual jump on hand-over).
        assert_eq!(
            wb_preview(&aircraft, &doc.loading, Some(&landed.fuel)).as_ref(),
            Some(&landed.weight_balance)
        );

        // Mid-drag state: edit the loading, preview with the *old* ladder
        // — what the UI shows before the debounced compute lands — must
        // equal what that compute then produces.
        assert!(set_station_load(&mut doc, "Front seats", 156.0));
        assert!(set_station_load(&mut doc, "Baggage", 30.0));
        assert!(set_fuel_liters(&mut doc, 90.0, Some(aircraft.fuel.usable.0)));
        let preview =
            wb_preview(&aircraft, &doc.loading, Some(&landed.fuel)).expect("preview computes");
        let (outcome, _) = run_compute(&doc, Some(&aircraft), Some(store), winds, &params, None);
        let ComputeOutcome::Computed(recomputed) = outcome else {
            panic!("expected Computed, got {outcome:?}");
        };
        assert_eq!(preview, recomputed.weight_balance);
        assert_ne!(
            landed.weight_balance, recomputed.weight_balance,
            "the edits moved the report — the agreement is not vacuous"
        );
    }

    /// Every in-flight edit yields a fresh preview — the totals/envelope
    /// track a drag burst without any background compute landing.
    #[test]
    fn preview_tracks_every_in_flight_edit() {
        let aircraft = example_c172();
        let ladder = ladder(2.0, 38.0);
        let mut doc = FlightDoc::new("t");
        doc.loading.fuel = Liters(100.0);

        let mut previous = wb_preview(&aircraft, &doc.loading, Some(&ladder)).expect("preview");
        // Simulated slider drag positions.
        for mass in [40.0, 80.0, 120.0, 160.0] {
            assert!(set_station_load(&mut doc, "Front seats", mass));
            let preview =
                wb_preview(&aircraft, &doc.loading, Some(&ladder)).expect("preview computes");
            assert_ne!(preview, previous, "{mass} kg must move the report");
            let takeoff = preview
                .states
                .iter()
                .find(|s| s.kind == WbStateKind::Takeoff)
                .expect("takeoff state");
            // Empty mass + station load + (ramp − taxi) fuel mass.
            let expected = 767.0 + mass + (100.0 - 2.0) * 0.72;
            assert!(
                (takeoff.mass.0 - expected).abs() < 1e-9,
                "takeoff mass {} != {expected}",
                takeoff.mass.0
            );
            previous = preview;
        }
    }

    /// Without a landed ladder there is no taxi/trip figure to preview
    /// with — the tab falls back to the authoritative report (or hint).
    #[test]
    fn preview_requires_a_landed_ladder() {
        let aircraft = example_c172();
        let doc = FlightDoc::new("t");
        assert_eq!(wb_preview(&aircraft, &doc.loading, None), None);
    }

    #[test]
    fn station_edits_create_update_and_remove_loads() {
        let mut doc = FlightDoc::new("t");
        assert_eq!(station_mass(&doc, "Front seats"), 0.0);

        // Create.
        assert!(set_station_load(&mut doc, "Front seats", 155.0));
        assert_eq!(station_mass(&doc, "Front seats"), 155.0);
        assert_eq!(doc.loading.station_loads.len(), 1);

        // Same value: no change reported (no dirty/compute churn).
        assert!(!set_station_load(&mut doc, "Front seats", 155.0));

        // Update.
        assert!(set_station_load(&mut doc, "Front seats", 170.0));
        assert_eq!(station_mass(&doc, "Front seats"), 170.0);
        assert_eq!(doc.loading.station_loads.len(), 1, "updated in place");

        // A second station coexists.
        assert!(set_station_load(&mut doc, "Baggage", 20.0));
        assert_eq!(doc.loading.station_loads.len(), 2);

        // Zero removes the entry (absent == 0 kg).
        assert!(set_station_load(&mut doc, "Front seats", 0.0));
        assert_eq!(station_mass(&doc, "Front seats"), 0.0);
        assert_eq!(doc.loading.station_loads.len(), 1);
        assert!(!set_station_load(&mut doc, "Front seats", 0.0), "already absent");

        // Negative input clamps to zero (= removal).
        assert!(set_station_load(&mut doc, "Baggage", -5.0));
        assert!(doc.loading.station_loads.is_empty());
    }

    #[test]
    fn fuel_edits_clamp_into_the_usable_range() {
        let mut doc = FlightDoc::new("t");
        assert!(set_fuel_liters(&mut doc, 120.0, Some(180.0)));
        assert_eq!(doc.loading.fuel, Liters(120.0));

        // Clamped to usable.
        assert!(set_fuel_liters(&mut doc, 500.0, Some(180.0)));
        assert_eq!(doc.loading.fuel, Liters(180.0));

        // Negative clamps to zero.
        assert!(set_fuel_liters(&mut doc, -10.0, Some(180.0)));
        assert_eq!(doc.loading.fuel, Liters(0.0));

        // No usable capacity entered → unclamped above zero.
        assert!(set_fuel_liters(&mut doc, 500.0, None));
        assert_eq!(doc.loading.fuel, Liters(500.0));
        assert!(!set_fuel_liters(&mut doc, 500.0, None), "no-op reported");
    }

    #[test]
    fn steps_clamp_into_the_editor_range() {
        assert_eq!(step_value(10.0, StepAction::Increment, 200.0), 11.0);
        assert_eq!(step_value(200.0, StepAction::Increment, 200.0), 200.0);
        assert_eq!(step_value(0.5, StepAction::Decrement, 200.0), 0.0);
        assert_eq!(step_value(0.0, StepAction::Decrement, 200.0), 0.0);
    }
}

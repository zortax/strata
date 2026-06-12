//! The Fuel tab (design §3.4 "Fuel"): the fuel ladder visualized as a
//! stacked horizontal bar against usable fuel, the numeric breakdown,
//! endurance / fuel-at-destination readouts, and the policy editor
//! (EASA Part-NCO **template** — clearly labelled, not regulatory
//! guidance).
//!
//! The bar's segment math ([`ladder_layout`]) is pure and unit-tested;
//! the policy edits are pure [`FlightDoc`] mutations routed through
//! `AppState::edit_flight_doc` so the recompute flows automatically.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, FontWeight, Hsla, IntoElement, ParentElement as _,
    Styled as _, Window, div, px, relative,
};
use gpui_component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::NumberInput,
    v_flex,
};
use strata_plan::AircraftProfile;
use strata_plan::flight::{Contingency, FlightDoc, FuelPolicy};
use strata_plan::fuel::{self, FuelLadder};
use strata_plan::units::{Liters, Minutes};

use crate::ui::info_panel::{card, section};

use super::{ContextPanel, FlightView, not_computable_hint};

/// Minimum scale of the ladder bar, liters — keeps fractions finite for
/// all-zero ladders (empty policy on a zero-length route).
const MIN_SCALE_LITERS: f64 = 1.0;

// --- pure segment math --------------------------------------------------------

/// One rung of the fuel ladder, in bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FuelRung {
    Taxi,
    Trip,
    Contingency,
    Alternate,
    FinalReserve,
    Extra,
}

impl FuelRung {
    pub const ALL: [FuelRung; 6] = [
        FuelRung::Taxi,
        FuelRung::Trip,
        FuelRung::Contingency,
        FuelRung::Alternate,
        FuelRung::FinalReserve,
        FuelRung::Extra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FuelRung::Taxi => "Taxi",
            FuelRung::Trip => "Trip",
            FuelRung::Contingency => "Contingency",
            FuelRung::Alternate => "Alternate",
            FuelRung::FinalReserve => "Final reserve",
            FuelRung::Extra => "Extra",
        }
    }

    fn liters(self, ladder: &FuelLadder) -> f64 {
        match self {
            FuelRung::Taxi => ladder.taxi.0,
            FuelRung::Trip => ladder.trip.0,
            FuelRung::Contingency => ladder.contingency.0,
            FuelRung::Alternate => ladder.alternate.0,
            FuelRung::FinalReserve => ladder.final_reserve.0,
            FuelRung::Extra => ladder.extra.0,
        }
    }
}

/// One bar segment: the rung, its liters and its fraction of the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LadderSegment {
    pub rung: FuelRung,
    pub liters: f64,
    pub fraction: f64,
}

/// The laid-out ladder bar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LadderLayout {
    /// Non-empty rungs in ladder order; fractions of the full bar width.
    pub segments: Vec<LadderSegment>,
    /// Liters the full bar width represents: the maximum of minimum
    /// required, loaded and usable fuel — every marker fits on the bar.
    pub scale: f64,
    /// Loaded-fuel marker position (fraction of the bar).
    pub loaded_fraction: f64,
    /// Usable-fuel marker position; `None` when the profile has no usable
    /// capacity entered.
    pub usable_fraction: Option<f64>,
}

/// Lays the ladder out against `usable` tank capacity (clamped at zero;
/// negative policy rungs cannot occur — strata-plan clamps — but the
/// layout guards anyway).
pub(crate) fn ladder_layout(ladder: &FuelLadder, usable: Option<Liters>) -> LadderLayout {
    let usable = usable.map(|u| u.0.max(0.0)).filter(|u| *u > 0.0);
    let loaded = ladder.loaded.0.max(0.0);
    let scale = ladder
        .minimum_required
        .0
        .max(loaded)
        .max(usable.unwrap_or(0.0))
        .max(MIN_SCALE_LITERS);

    let segments = FuelRung::ALL
        .into_iter()
        .filter_map(|rung| {
            let liters = rung.liters(ladder).max(0.0);
            (liters > 0.0).then_some(LadderSegment {
                rung,
                liters,
                fraction: liters / scale,
            })
        })
        .collect();

    LadderLayout {
        segments,
        scale,
        loaded_fraction: loaded / scale,
        usable_fraction: usable.map(|u| u / scale),
    }
}

// --- pure document mutations (the policy editor's commits) ---------------------

pub(crate) fn set_taxi_minutes(doc: &mut FlightDoc, minutes: f64) -> bool {
    let minutes = Minutes(minutes.max(0.0));
    if doc.fuel_policy.taxi == minutes {
        return false;
    }
    doc.fuel_policy.taxi = minutes;
    true
}

pub(crate) fn set_final_reserve_minutes(doc: &mut FlightDoc, minutes: f64) -> bool {
    let minutes = Minutes(minutes.max(0.0));
    if doc.fuel_policy.final_reserve == minutes {
        return false;
    }
    doc.fuel_policy.final_reserve = minutes;
    true
}

pub(crate) fn set_extra_liters(doc: &mut FlightDoc, liters: f64) -> bool {
    let liters = Liters(liters.max(0.0));
    if doc.fuel_policy.extra == liters {
        return false;
    }
    doc.fuel_policy.extra = liters;
    true
}

pub(crate) fn set_contingency(doc: &mut FlightDoc, contingency: Contingency) -> bool {
    if doc.fuel_policy.contingency == contingency {
        return false;
    }
    doc.fuel_policy.contingency = contingency;
    true
}

/// The contingency *value* keeps its current mode (percent stays percent,
/// fixed stays fixed) — what the value input commits.
pub(crate) fn set_contingency_value(doc: &mut FlightDoc, value: f64) -> bool {
    let value = value.max(0.0);
    let next = match doc.fuel_policy.contingency {
        Contingency::PercentOfTrip(_) => Contingency::PercentOfTrip(value),
        Contingency::Fixed(_) => Contingency::Fixed(Liters(value)),
    };
    set_contingency(doc, next)
}

/// Switches the contingency mode, carrying the EASA-template default into
/// a freshly chosen percent mode (5 %) and zero into fixed mode.
pub(crate) fn switch_contingency_mode(doc: &mut FlightDoc, percent: bool) -> bool {
    let next = match (percent, doc.fuel_policy.contingency) {
        (true, Contingency::PercentOfTrip(_)) | (false, Contingency::Fixed(_)) => return false,
        (true, Contingency::Fixed(_)) => Contingency::PercentOfTrip(5.0),
        (false, Contingency::PercentOfTrip(_)) => Contingency::Fixed(Liters(0.0)),
    };
    set_contingency(doc, next)
}

// --- formatting -----------------------------------------------------------------

pub(crate) fn format_liters(liters: f64) -> String {
    format!("{liters:.1} L")
}

/// `h:mm` endurance formatting (matches the cockpit habit).
pub(crate) fn format_minutes(minutes: f64) -> String {
    let total = minutes.max(0.0).round() as u64;
    format!("{}:{:02} h", total / 60, total % 60)
}

// --- rendering -------------------------------------------------------------------

/// Theme color of a ladder rung (semantic: reserves warn-ish, trip is the
/// primary substance, extra is discretionary).
fn rung_color(rung: FuelRung, cx: &App) -> Hsla {
    match rung {
        FuelRung::Taxi => cx.theme().muted_foreground.opacity(0.65),
        FuelRung::Trip => cx.theme().primary,
        FuelRung::Contingency => cx.theme().info,
        FuelRung::Alternate => cx.theme().chart_2,
        FuelRung::FinalReserve => cx.theme().warning,
        FuelRung::Extra => cx.theme().secondary_foreground.opacity(0.5),
    }
}

pub(super) fn render_fuel_tab(
    panel: &ContextPanel,
    flight: &FlightView,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let Some(aircraft) = &flight.aircraft else {
        return not_computable_hint(
            "Select an aircraft profile to plan fuel.",
            flight.compute_hint.clone(),
            cx,
        );
    };

    let mut content = v_flex().gap_3();

    match &flight.ladder {
        Some(ladder) => {
            content = content
                .child(ladder_card(ladder, aircraft, cx))
                .child(readouts_card(ladder, aircraft, &flight.doc, cx));
        }
        None => {
            content = content.child(not_computable_hint(
                "Fuel plan appears once the route computes.",
                flight.compute_hint.clone(),
                cx,
            ));
        }
    }

    content.child(policy_card(panel, &flight.doc.fuel_policy, cx)).into_any_element()
}

/// The visual ladder: stacked bar + loaded/usable markers + per-rung rows.
fn ladder_card(ladder: &FuelLadder, aircraft: &AircraftProfile, cx: &App) -> AnyElement {
    let usable = aircraft.fuel.usable;
    let layout = ladder_layout(ladder, Some(usable));
    let under_fueled = ladder.margin.0 < 0.0;

    // Stacked bar with overlay markers (loaded = solid line, usable = end
    // of the tinted background track).
    let bar = div()
        .relative()
        .w_full()
        .h(px(18.))
        .rounded(cx.theme().radius)
        .overflow_hidden()
        .bg(cx.theme().muted.opacity(0.35))
        .child(
            h_flex()
                .absolute()
                .inset_0()
                .children(layout.segments.iter().map(|segment| {
                    div()
                        .h_full()
                        .w(relative(segment.fraction as f32))
                        .bg(rung_color(segment.rung, cx))
                })),
        )
        .children(layout.usable_fraction.map(|fraction| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(fraction as f32))
                .w(px(2.))
                .bg(cx.theme().foreground.opacity(0.45))
        }))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(layout.loaded_fraction as f32))
                .w(px(2.))
                .bg(if under_fueled {
                    cx.theme().danger
                } else {
                    cx.theme().success
                }),
        );

    let mut el = card(cx)
        .child(
            h_flex()
                .justify_between()
                .child(section("Fuel ladder", cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("usable {}", format_liters(usable.0))),
                ),
        )
        .child(bar)
        .child(
            h_flex()
                .gap_3()
                .flex_wrap()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(layout.segments.iter().map(|segment| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().size_2().rounded_sm().bg(rung_color(segment.rung, cx)))
                        .child(segment.rung.label())
                })),
        );

    // Numeric breakdown: every rung (zeros included — the breakdown is the
    // checklist), then the derived rows.
    for rung in FuelRung::ALL {
        el = el.child(breakdown_row(
            rung.label(),
            rung.liters(ladder),
            false,
            None,
            cx,
        ));
    }
    el = el
        .child(div().h(px(1.)).bg(cx.theme().border))
        .child(breakdown_row(
            "Minimum required",
            ladder.minimum_required.0,
            true,
            None,
            cx,
        ))
        .child(breakdown_row("Loaded", ladder.loaded.0, true, None, cx))
        .child(breakdown_row(
            "Margin",
            ladder.margin.0,
            true,
            Some(if under_fueled {
                cx.theme().danger
            } else {
                cx.theme().success
            }),
            cx,
        ));
    if under_fueled {
        el = el.child(
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child("Loaded fuel is below the minimum required."),
        );
    }
    el.into_any_element()
}

fn breakdown_row(
    label: &str,
    liters: f64,
    emphasized: bool,
    color: Option<Hsla>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .text_sm()
        .when(emphasized, |el| el.font_weight(FontWeight::SEMIBOLD))
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .when(emphasized, |el| el.text_color(cx.theme().foreground))
                .child(label.to_string()),
        )
        .child(
            div()
                .font_family("monospace")
                .when_some(color, |el, color| el.text_color(color))
                .child(format_liters(liters)),
        )
}

/// Endurance and fuel-at-destination/alternate readouts.
fn readouts_card(
    ladder: &FuelLadder,
    aircraft: &AircraftProfile,
    doc: &FlightDoc,
    cx: &App,
) -> AnyElement {
    let power_setting = doc.power_setting.as_deref();
    let endurance = fuel::endurance(aircraft, power_setting, ladder.loaded).ok();
    let at_destination = ladder.loaded.0 - ladder.taxi.0 - ladder.trip.0;
    let at_alternate =
        (ladder.alternate.0 > 0.0).then_some(at_destination - ladder.alternate.0);

    let mut el = card(cx).child(section("Endurance", cx));
    if let Some(endurance) = endurance {
        el = el.child(readout_row(
            "Endurance (loaded fuel)",
            format_minutes(endurance.0),
            None,
            cx,
        ));
    }
    el = el.child(readout_row(
        "Fuel at destination",
        format_liters(at_destination),
        (at_destination < ladder.final_reserve.0).then(|| cx.theme().danger),
        cx,
    ));
    if let Some(at_alternate) = at_alternate {
        el = el.child(readout_row(
            "Fuel at alternate",
            format_liters(at_alternate),
            (at_alternate < ladder.final_reserve.0).then(|| cx.theme().danger),
            cx,
        ));
    }
    el.into_any_element()
}

fn readout_row(label: &str, value: String, color: Option<Hsla>, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_between()
        .text_sm()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .font_family("monospace")
                .when_some(color, |el, color| el.text_color(color))
                .child(value),
        )
}

/// The policy editor — writes through to the document's fuel policy.
fn policy_card(
    panel: &ContextPanel,
    policy: &FuelPolicy,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let percent_mode = matches!(policy.contingency, Contingency::PercentOfTrip(_));

    card(cx)
        .child(section("Fuel policy", cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("EASA Part-NCO template — verify current regulation."),
        )
        .child(policy_row(
            "Taxi",
            NumberInput::new(&panel.policy.taxi_input)
                .small()
                .suffix(unit_suffix("min", cx)),
        ))
        .child(policy_row(
            "Final reserve",
            NumberInput::new(&panel.policy.reserve_input)
                .small()
                .suffix(unit_suffix("min", cx)),
        ))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w(px(110.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Contingency"),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        NumberInput::new(&panel.policy.contingency_input)
                            .small()
                            .suffix(unit_suffix(if percent_mode { "%" } else { "L" }, cx)),
                    ),
                )
                .child(
                    Button::new("contingency-percent")
                        .xsmall()
                        .ghost()
                        .label("%")
                        .selected(percent_mode)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.commit_doc_edit(cx, |doc| {
                                switch_contingency_mode(doc, true)
                            });
                        })),
                )
                .child(
                    Button::new("contingency-fixed")
                        .xsmall()
                        .ghost()
                        .label("L")
                        .selected(!percent_mode)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.commit_doc_edit(cx, |doc| {
                                switch_contingency_mode(doc, false)
                            });
                        })),
                ),
        )
        .child(policy_row(
            "Extra",
            NumberInput::new(&panel.policy.extra_input)
                .small()
                .suffix(unit_suffix("L", cx)),
        ))
        .into_any_element()
}

fn policy_row(label: &str, input: NumberInput) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(110.))
                .flex_shrink_0()
                .text_sm()
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(input))
}

fn unit_suffix(unit: &'static str, cx: &App) -> impl IntoElement {
    div()
        .pr_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(unit)
}

// --- policy input plumbing (owned by ContextPanel, driven from mod.rs) ---------

use gpui::{Entity, Subscription};
use gpui_component::input::{InputEvent, InputState, NumberInputEvent, StepAction};

/// The Fuel tab's stateful policy inputs.
pub(crate) struct PolicyInputs {
    pub taxi_input: Entity<InputState>,
    pub reserve_input: Entity<InputState>,
    pub contingency_input: Entity<InputState>,
    pub extra_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// Which policy knob an input edits (commit dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyField {
    Taxi,
    Reserve,
    Contingency,
    Extra,
}

impl PolicyField {
    fn commit(self, doc: &mut FlightDoc, value: f64) -> bool {
        match self {
            PolicyField::Taxi => set_taxi_minutes(doc, value),
            PolicyField::Reserve => set_final_reserve_minutes(doc, value),
            PolicyField::Contingency => set_contingency_value(doc, value),
            PolicyField::Extra => set_extra_liters(doc, value),
        }
    }

    /// Current document value of the field (the sync direction).
    fn read(self, policy: &FuelPolicy) -> f64 {
        match self {
            PolicyField::Taxi => policy.taxi.0,
            PolicyField::Reserve => policy.final_reserve.0,
            PolicyField::Contingency => match policy.contingency {
                Contingency::PercentOfTrip(pct) => pct,
                Contingency::Fixed(liters) => liters.0,
            },
            PolicyField::Extra => policy.extra.0,
        }
    }
}

impl PolicyInputs {
    pub fn new(window: &mut Window, cx: &mut Context<ContextPanel>) -> Self {
        let mut subscriptions = Vec::new();
        let mut make = |field: PolicyField| {
            let input = cx.new(|cx| InputState::new(window, cx));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut ContextPanel, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change)
                        && let Ok(value) = input.read(cx).value().trim().parse::<f64>()
                    {
                        this.commit_doc_edit(cx, |doc| field.commit(doc, value.max(0.0)));
                    }
                },
            ));
            // The ± stepper: `set_value` is event-silent at this
            // gpui-component rev (see ui::settings), so step commits here.
            subscriptions.push(cx.subscribe_in(
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
                            .map_or(0.0, |f| field.read(&f.doc.fuel_policy))
                    });
                    let next = match action {
                        StepAction::Increment => current + 1.0,
                        StepAction::Decrement => current - 1.0,
                    }
                    .max(0.0);
                    input.update(cx, |input, cx| {
                        input.set_value(format_number(next), window, cx);
                    });
                    this.commit_doc_edit(cx, |doc| field.commit(doc, next));
                },
            ));
            input
        };
        Self {
            taxi_input: make(PolicyField::Taxi),
            reserve_input: make(PolicyField::Reserve),
            contingency_input: make(PolicyField::Contingency),
            extra_input: make(PolicyField::Extra),
            _subscriptions: subscriptions,
        }
    }

    /// Pushes the document's policy into the inputs (skipping focused ones
    /// — never clobber live typing).
    pub fn sync(&self, policy: &FuelPolicy, window: &mut Window, cx: &mut App) {
        for (field, input) in [
            (PolicyField::Taxi, &self.taxi_input),
            (PolicyField::Reserve, &self.reserve_input),
            (PolicyField::Contingency, &self.contingency_input),
            (PolicyField::Extra, &self.extra_input),
        ] {
            sync_number_input(input, field.read(policy), window, cx);
        }
    }
}

/// Writes `value` into a numeric input unless it is focused or already
/// shows the value (epsilon — round-trip through the text form).
pub(crate) fn sync_number_input(
    input: &Entity<InputState>,
    value: f64,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui::Focusable as _;
    if input.read(cx).focus_handle(cx).is_focused(window) {
        return;
    }
    let shown = input.read(cx).value().trim().parse::<f64>();
    if shown.is_ok_and(|shown| (shown - value).abs() < 1e-9) {
        return;
    }
    input.update(cx, |input, cx| {
        input.set_value(format_number(value), window, cx);
    });
}

/// Numbers in inputs: trim trailing zeros but keep sub-liter precision.
pub(crate) fn format_number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladder(
        taxi: f64,
        trip: f64,
        contingency: f64,
        alternate: f64,
        reserve: f64,
        extra: f64,
        loaded: f64,
    ) -> FuelLadder {
        let minimum = taxi + trip + contingency + alternate + reserve + extra;
        FuelLadder {
            taxi: Liters(taxi),
            trip: Liters(trip),
            contingency: Liters(contingency),
            alternate: Liters(alternate),
            final_reserve: Liters(reserve),
            extra: Liters(extra),
            minimum_required: Liters(minimum),
            loaded: Liters(loaded),
            margin: Liters(loaded - minimum),
        }
    }

    #[test]
    fn segments_cover_the_minimum_required_in_rung_order() {
        let l = ladder(2.0, 40.0, 2.0, 10.0, 11.0, 5.0, 100.0);
        let layout = ladder_layout(&l, Some(Liters(120.0)));
        // Scale = max(min required 70, loaded 100, usable 120).
        assert_eq!(layout.scale, 120.0);
        let rungs: Vec<FuelRung> = layout.segments.iter().map(|s| s.rung).collect();
        assert_eq!(rungs, FuelRung::ALL.to_vec(), "all rungs non-zero, in order");
        let total: f64 = layout.segments.iter().map(|s| s.fraction).sum();
        assert!((total - 70.0 / 120.0).abs() < 1e-12, "sum = {total}");
        assert!((layout.loaded_fraction - 100.0 / 120.0).abs() < 1e-12);
        assert_eq!(layout.usable_fraction, Some(1.0));
    }

    #[test]
    fn zero_rungs_drop_out_of_the_bar() {
        let l = ladder(0.0, 40.0, 0.0, 0.0, 11.0, 0.0, 60.0);
        let layout = ladder_layout(&l, Some(Liters(100.0)));
        let rungs: Vec<FuelRung> = layout.segments.iter().map(|s| s.rung).collect();
        assert_eq!(rungs, vec![FuelRung::Trip, FuelRung::FinalReserve]);
    }

    #[test]
    fn under_fueled_ladders_scale_to_the_minimum_required() {
        // Minimum 70 L exceeds both loaded (30) and usable (50): the bar
        // scales to the minimum so the requirement stays fully visible.
        let l = ladder(2.0, 40.0, 2.0, 10.0, 11.0, 5.0, 30.0);
        let layout = ladder_layout(&l, Some(Liters(50.0)));
        assert_eq!(layout.scale, 70.0);
        assert!((layout.loaded_fraction - 30.0 / 70.0).abs() < 1e-12);
        assert_eq!(layout.usable_fraction, Some(50.0 / 70.0));
        assert!(l.margin.0 < 0.0);
    }

    #[test]
    fn degenerate_ladders_stay_finite() {
        let l = ladder(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let layout = ladder_layout(&l, None);
        assert_eq!(layout.scale, MIN_SCALE_LITERS);
        assert!(layout.segments.is_empty());
        assert_eq!(layout.loaded_fraction, 0.0);
        assert_eq!(layout.usable_fraction, None);
        // Zero usable capacity is treated as "not entered".
        assert_eq!(ladder_layout(&l, Some(Liters(0.0))).usable_fraction, None);
    }

    #[test]
    fn policy_edits_mutate_the_document_and_report_changes() {
        let mut doc = FlightDoc::new("t");
        // Defaults: taxi 10, reserve 30, 5 % contingency, 0 extra.
        assert!(!set_taxi_minutes(&mut doc, 10.0), "no-op edit");
        assert!(set_taxi_minutes(&mut doc, 15.0));
        assert_eq!(doc.fuel_policy.taxi, Minutes(15.0));

        assert!(set_final_reserve_minutes(&mut doc, 45.0));
        assert_eq!(doc.fuel_policy.final_reserve, Minutes(45.0));
        assert!(!set_final_reserve_minutes(&mut doc, 45.0));

        assert!(set_extra_liters(&mut doc, 12.0));
        assert_eq!(doc.fuel_policy.extra, Liters(12.0));

        // Negative input clamps to zero (cannot reduce the minimum).
        assert!(set_extra_liters(&mut doc, -3.0));
        assert_eq!(doc.fuel_policy.extra, Liters(0.0));
    }

    #[test]
    fn contingency_value_keeps_its_mode_and_mode_switches_carry_defaults() {
        let mut doc = FlightDoc::new("t"); // PercentOfTrip(5.0)
        assert!(set_contingency_value(&mut doc, 10.0));
        assert_eq!(doc.fuel_policy.contingency, Contingency::PercentOfTrip(10.0));

        // Same-mode switch is a no-op.
        assert!(!switch_contingency_mode(&mut doc, true));

        assert!(switch_contingency_mode(&mut doc, false));
        assert_eq!(doc.fuel_policy.contingency, Contingency::Fixed(Liters(0.0)));
        assert!(set_contingency_value(&mut doc, 8.0));
        assert_eq!(doc.fuel_policy.contingency, Contingency::Fixed(Liters(8.0)));

        // Back to percent restores the EASA template default.
        assert!(switch_contingency_mode(&mut doc, true));
        assert_eq!(doc.fuel_policy.contingency, Contingency::PercentOfTrip(5.0));
    }

    #[test]
    fn input_number_formatting_is_stable() {
        assert_eq!(format_number(30.0), "30");
        assert_eq!(format_number(7.5), "7.5");
        assert_eq!(format_minutes(252.0), "4:12 h");
        assert_eq!(format_minutes(-5.0), "0:00 h");
        assert_eq!(format_liters(81.25), "81.2 L");
    }
}

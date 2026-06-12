//! Summary card at the bottom of the flight panel (design §3.1): totals
//! (distance, ETE, ETA), the fuel line (trip vs minimum required vs
//! usable) and the status badge row — W&B / Fuel / Terrain / Airspace
//! from the computed conflicts, NOTAM from the briefing relevance (see
//! `state::briefing`). Every badge tooltips its first offending message
//! and navigates to its surface.

use gpui::{
    AnyElement, Context, FontWeight, Hsla, IntoElement, ParentElement as _, Styled as _, div,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use strata_plan::units::Liters;

use crate::app::RootView;
use crate::state::ComputeState;

use super::model::{self, BadgeTone, BadgeVm};

pub(super) fn render_summary(root: &RootView, cx: &Context<RootView>) -> AnyElement {
    let Some(panel) = &root.flight_panel else {
        return div().into_any_element();
    };
    // Usable fuel comes from the referenced aircraft profile (the ladder
    // itself only knows the loaded amount); the NOTAM badge from the
    // live briefing state (snapshot + relevance, not conflicts).
    let (usable, notam_badge): (Option<Liters>, _) = {
        let state = root.app_state.read(cx);
        (
            state.flight_aircraft().map(|p| p.fuel.usable),
            model::notam_badge_vm(
                state.notam_badge(),
                state.flight.as_ref().and_then(|f| f.briefing.as_ref()),
            ),
        )
    };

    let computed = panel.snapshot.computed.as_deref();
    let status_line = match &panel.snapshot.compute_state {
        ComputeState::NotComputable(reason) => Some((reason.to_string(), false)),
        ComputeState::Failed(error) => Some((format!("Compute failed: {error}"), true)),
        ComputeState::Pending | ComputeState::Computed => None,
    };

    let dash = || "—".to_owned();
    let (distance, ete, eta) = computed.map_or_else(
        || (dash(), dash(), dash()),
        |c| {
            (
                format!("{} NM", model::fmt_nm(c.navlog.totals.distance)),
                model::fmt_minutes(c.navlog.totals.ete),
                model::final_eta(&c.navlog.rows).map_or_else(dash, model::fmt_eta),
            )
        },
    );
    let (trip, minimum) = computed.map_or_else(
        || (dash(), dash()),
        |c| {
            (
                model::fmt_liters(c.fuel.trip),
                model::fmt_liters(c.fuel.minimum_required),
            )
        },
    );
    let usable = usable.map_or_else(dash, model::fmt_liters);

    let badges = model::badge_row(computed.map(|c| c.conflicts.as_slice()), notam_badge);

    v_flex()
        .p_3()
        .gap_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .children(status_line.map(|(text, failed)| {
            div()
                .text_xs()
                .text_color(if failed {
                    cx.theme().danger
                } else {
                    cx.theme().muted_foreground
                })
                .child(text)
        }))
        .child(
            h_flex()
                .gap_2()
                .child(stat("Distance", distance, cx))
                .child(stat("ETE", ete, cx))
                .child(stat("ETA", eta, cx)),
        )
        .child(
            h_flex()
                .gap_2()
                .child(stat("Trip fuel", trip, cx))
                .child(stat("Minimum", minimum, cx))
                .child(stat("Usable", usable, cx)),
        )
        .child(
            h_flex().gap_1().justify_between().children(
                badges
                    .into_iter()
                    .enumerate()
                    .map(|(index, badge)| render_badge(index, badge, cx)),
            ),
        )
        .into_any_element()
}

/// One labelled stat cell (equal thirds of the row).
fn stat(label: &'static str, value: String, cx: &Context<RootView>) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(value),
        )
}

/// A status badge: tone dot + label (the Unknown tone renders an em-dash
/// instead of a dot). Tooltips the first offending conflict message;
/// clicking navigates to the badge's surface (design §3.1: profile drawer
/// at the first conflict, Loading tab, Fuel tab) through the app-state
/// focus funnel — the panel never reaches into another panel.
fn render_badge(index: usize, badge: BadgeVm, cx: &Context<RootView>) -> AnyElement {
    let indicator: AnyElement = match badge.tone {
        BadgeTone::Unknown => div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("—")
            .into_any_element(),
        tone => div()
            .size_2()
            .flex_shrink_0()
            .rounded_full()
            .bg(badge_color(tone, cx))
            .into_any_element(),
    };
    let mut button = Button::new(("fp-badge", index)).ghost().xsmall().child(
        h_flex()
            .gap_1()
            .items_center()
            .child(indicator)
            .child(div().text_xs().child(badge.label)),
    );
    if let Some(tooltip) = badge.tooltip {
        button = button.tooltip(tooltip);
    }
    if let Some(focus) = badge.focus {
        button = button.on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
            this.app_state
                .update(cx, |state, cx| state.request_planning_focus(focus, cx));
        }));
    }
    button.into_any_element()
}

/// Tone → theme color.
fn badge_color(tone: BadgeTone, cx: &Context<RootView>) -> Hsla {
    match tone {
        BadgeTone::Unknown => cx.theme().muted_foreground,
        BadgeTone::Ok => cx.theme().success,
        BadgeTone::Caution => cx.theme().warning,
        BadgeTone::Alert => cx.theme().danger,
    }
}

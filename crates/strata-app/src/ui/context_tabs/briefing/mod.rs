//! The Briefing tab (design §3.4 "Briefing"): the flight's NOTAM briefing
//! — snapshot status with the data-source provenance always visible
//! (live autorouter.aero data; without credentials the tab says so
//! honestly instead of showing anything fake), the relevance-ordered
//! NOTAM cards, and the export actions (Briefing PDF, ICAO FPL) at the
//! bottom.
//!
//! Renders exclusively off `state::briefing`'s API: the snapshot lives on
//! the document, the ranked list on [`OpenFlight::briefing`], the fetch
//! spinner on [`AppState::notam_fetch`].
//!
//! [`OpenFlight::briefing`]: crate::state::OpenFlight::briefing
//! [`AppState::notam_fetch`]: crate::state::AppState::notam_fetch

mod fpl_dialog;
mod input;
mod labels;

use chrono::Utc;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use strata_plan::notam_relevance::{NotamRelevance, RelevantNotam, is_restriction_activation};

use crate::assets::IconName;
use crate::state::AppState;
use crate::state::briefing::{NotamSource, notam_source};
use crate::ui::info_panel::{badge, card, section};

use super::{ContextPanel, FlightView};

pub(crate) use fpl_dialog::open_fpl_dialog;

// --- rendering ------------------------------------------------------------------

pub(super) fn render_briefing_tab(
    panel: &ContextPanel,
    flight: &FlightView,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let (source, fetching, fetch_error, pdf_exporting) = {
        let state = panel.app_state.read(cx);
        (
            notam_source(&state.config),
            state.notam_fetch.fetching,
            state.notam_fetch.last_error.clone(),
            state.pdf_exporting,
        )
    };
    let briefing = flight.briefing.as_ref();

    let mut content = v_flex().gap_3().child(snapshot_row(
        labels::snapshot_label(briefing.map(|b| b.taken_at), fetching, source),
        fetching,
        fetch_error,
        source,
        briefing.is_some(),
        cx,
    ));

    match briefing {
        None => {
            // The honest empty states: without credentials there is no
            // NOTAM source at all — say so instead of showing anything.
            let message = match source {
                NotamSource::NotConfigured => format!(
                    "{} Briefings fetch live NOTAMs from autorouter.aero \
                     (free account, end-user use only).",
                    crate::state::briefing::CREDENTIALS_MISSING
                ),
                NotamSource::Autorouter => "No NOTAM data yet — Refresh fetches NOTAMs \
                     for the route's aerodromes and the German FIRs and stores the \
                     snapshot with the flight."
                    .to_owned(),
            };
            content = content.child(
                card(cx).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_normal()
                        .child(message),
                ),
            );
        }
        Some(briefing) if briefing.relevant.is_empty() => {
            content = content.child(
                card(cx).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No relevant NOTAMs for this flight."),
                ),
            );
        }
        Some(briefing) => {
            for (index, entry) in briefing.relevant.iter().enumerate() {
                content = content.child(notam_card(panel, index, entry, cx));
            }
        }
    }

    content
        .child(export_section(pdf_exporting, cx))
        .into_any_element()
}

/// Snapshot status + refresh button; the provenance stays visible, and
/// without autorouter credentials the refresh is disabled with the
/// honest explanation underneath.
fn snapshot_row(
    label: String,
    fetching: bool,
    fetch_error: Option<String>,
    source: NotamSource,
    has_briefing: bool,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let not_configured = source == NotamSource::NotConfigured;
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    Button::new("notam-refresh")
                        .ghost()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .loading(fetching)
                        .disabled(fetching || not_configured)
                        .tooltip(if not_configured {
                            crate::state::briefing::CREDENTIALS_MISSING
                        } else {
                            "Refresh NOTAMs"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app_state
                                .update(cx, |state, cx| state.refresh_notams(cx));
                        })),
                ),
        )
        // With a stored snapshot the cards still render — explain why a
        // refresh is unavailable. (Without one, the empty-state card
        // below already says so.)
        .children((not_configured && has_briefing).then(|| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.8))
                .whitespace_normal()
                .child(crate::state::briefing::CREDENTIALS_MISSING)
        }))
        .children(fetch_error.map(|err| {
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .whitespace_normal()
                .child(format!("Last fetch failed: {err}"))
        }))
        .into_any_element()
}

/// One briefing-list entry as a card: location chip + id + activity state,
/// relevance chip + validity, decoded Q-line summary, item E text,
/// schedule, raw expander.
fn notam_card(
    panel: &ContextPanel,
    index: usize,
    entry: &RelevantNotam,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let notam = &entry.notam;
    let id_string = notam.id.to_string();
    let red = entry.active_during_flight
        && matches!(entry.relevance, NotamRelevance::RouteCorridor { .. })
        && is_restriction_activation(notam);

    // Activity chip: the active-during-flight emphasis (design §3.4); the
    // red badge class gets the danger treatment.
    let activity: AnyElement = if red {
        chip("restriction active", cx.theme().danger, cx)
    } else if entry.active_during_flight {
        chip("active during flight", cx.theme().warning, cx)
    } else {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("outside flight window")
            .into_any_element()
    };

    let mut el = card(cx)
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .items_center()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .min_w_0()
                        .child(badge(labels::location_label(notam), cx))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_sm()
                                .child(id_string.clone()),
                        ),
                )
                .child(activity),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(badge(labels::relevance_label(&entry.relevance), cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(if entry.active_during_flight {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(labels::validity_label(&notam.validity)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .whitespace_normal()
                .child(labels::q_summary(&notam.q)),
        )
        .child(
            div()
                .text_sm()
                .whitespace_normal()
                .min_w_0()
                .child(notam.text.clone()),
        );

    if let Some(schedule) = &notam.schedule {
        el = el.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Schedule {schedule}")),
        );
    }

    // Raw text expander — the taf_rows recipe: section header + chevron,
    // monospace block while expanded.
    let expanded = panel.expanded_notams.contains(&id_string);
    let toggle_id = id_string.clone();
    el = el.child(
        h_flex()
            .id(("notam-raw-toggle", index))
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded_notams.remove(&toggle_id) {
                    this.expanded_notams.insert(toggle_id.clone());
                }
                cx.notify();
            }))
            .child(section("Raw", cx))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .small()
                .text_color(cx.theme().muted_foreground),
            ),
    );
    if expanded {
        el = el.child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_xs()
                .font_family("monospace")
                .text_color(cx.theme().muted_foreground)
                .child(notam.raw.clone()),
        );
    }

    el.into_any_element()
}

/// A small tinted pill (the activity chip).
fn chip(text: &'static str, color: gpui::Hsla, cx: &App) -> AnyElement {
    div()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .rounded(cx.theme().radius)
        .bg(color.opacity(0.15))
        .text_color(color)
        .flex_shrink_0()
        .child(text)
        .into_any_element()
}

/// The export actions at the tab bottom (design §3.4: "Below: the export
/// actions (Briefing PDF, ICAO FPL) with the FPL preview").
fn export_section(pdf_exporting: bool, cx: &mut Context<ContextPanel>) -> AnyElement {
    v_flex()
        .gap_2()
        .child(section("Export", cx))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("export-fpl")
                        .outline()
                        .small()
                        .icon(IconName::FileText)
                        .label("ICAO FPL…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            open_fpl_dialog(this.app_state.clone(), window, cx);
                        })),
                )
                .child(
                    Button::new("export-pdf")
                        .outline()
                        .small()
                        .icon(IconName::Download)
                        .label("Briefing PDF…")
                        .loading(pdf_exporting)
                        .disabled(pdf_exporting)
                        .on_click(cx.listener(|this, _, _, cx| {
                            start_pdf_export(&this.app_state, cx);
                        })),
                ),
        )
        .children(pdf_exporting.then(|| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Rendering the briefing PDF…")
        }))
        .into_any_element()
}

// --- the PDF export entry point ----------------------------------------------------

/// Converts the open flight into the [`strata_brief::BriefingInput`] (the
/// cheap, pure part — see [`input`]) and hands it to
/// [`AppState::export_briefing_pdf`], which renders on the background
/// executor and runs the save flow. Shared by the tab button and the
/// Flight ▸ Export menu. No-op outside planning mode.
pub(crate) fn start_pdf_export(app_state: &Entity<AppState>, cx: &mut App) {
    let Some(built) = ({
        let state = app_state.read(cx);
        state.flight.as_ref().map(|flight| {
            let weather_taken_at = state.weather.last_fetched_at.map(|at| {
                Utc::now()
                    - chrono::Duration::from_std(at.elapsed()).unwrap_or_else(|_| {
                        chrono::Duration::zero()
                    })
            });
            input::briefing_input(&input::BriefingSources {
                doc: &flight.doc,
                aircraft: state.flight_aircraft(),
                computed: flight.computed.as_deref(),
                briefing: flight.briefing.as_ref(),
                notam_source: crate::state::briefing::notam_source(&state.config),
                metars: &state.weather.metars,
                tafs: &state.weather.tafs,
                winds_frames: &state.flight_winds_frames(),
                weather_taken_at,
                generated_at: Utc::now(),
            })
        })
    }) else {
        return;
    };
    app_state.update(cx, |state, cx| state.export_briefing_pdf(built, cx));
}

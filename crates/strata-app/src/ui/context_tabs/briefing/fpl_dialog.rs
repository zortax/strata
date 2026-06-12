//! The ICAO FPL dialog (design §3.4: "the export actions … with the FPL
//! preview"): the generated message in selectable monospace, the local
//! validation verdict (green check, or the typed per-item error), and
//! Copy / Save… actions. Opened from the Briefing tab and from
//! Flight ▸ Export ▸ ICAO FPL….

use gpui::{
    AnyElement, App, ClipboardItem, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    notification::NotificationType,
    text::TextView,
    v_flex,
};

use crate::assets::IconName;
use crate::state::AppState;
use crate::state::briefing::FplOutcome;

/// Dialog width — generous enough that typical FPL lines never wrap.
const DIALOG_WIDTH_PX: f32 = 560.;

/// What the dialog shows — the pure mapping from [`FplOutcome`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FplDialogBody {
    /// A locally validated message, ready to copy/save.
    Ready { message: String },
    /// Generation failed an item's local validation; `error` names the
    /// ICAO item and the reason (the typed `FplError` display).
    Invalid { error: String },
    /// Not generatable yet — missing aircraft, compute, or flight.
    NotReady { reason: String },
}

/// Pure dialog state from the FPL outcome.
pub(crate) fn dialog_body(outcome: &FplOutcome) -> FplDialogBody {
    match outcome {
        FplOutcome::Ready(message) => FplDialogBody::Ready {
            message: message.clone(),
        },
        FplOutcome::Invalid(error) => FplDialogBody::Invalid {
            error: error.to_string(),
        },
        FplOutcome::NotComputed(reason) => FplDialogBody::NotReady {
            reason: format!("The flight plan cannot be generated yet: {reason}."),
        },
        FplOutcome::NoFlight => FplDialogBody::NotReady {
            reason: "No flight is open.".to_owned(),
        },
    }
}

/// The message behind the Save…/Copy actions; `None` = the actions are
/// absent (nothing exportable).
pub(crate) fn export_message(body: &FplDialogBody) -> Option<&str> {
    match body {
        FplDialogBody::Ready { message } => Some(message),
        FplDialogBody::Invalid { .. } | FplDialogBody::NotReady { .. } => None,
    }
}

/// Opens the dialog over the current FPL outcome (a snapshot — the dialog
/// shows what the document generates *now*; reopen after edits).
pub(crate) fn open_fpl_dialog(app_state: Entity<AppState>, window: &mut Window, cx: &mut App) {
    let body = dialog_body(&app_state.read(cx).icao_fpl());
    window.open_dialog(cx, move |dialog, _, cx| {
        let dialog = dialog
            .title("ICAO flight plan")
            .w(px(DIALOG_WIDTH_PX))
            .overlay_closable(true)
            .child(render_body(&body, cx));

        let mut footer = DialogFooter::new().child(
            Button::new("fpl-close")
                .outline()
                .label("Close")
                .on_click(|_, window, cx| window.close_dialog(cx)),
        );
        if let Some(message) = export_message(&body) {
            let copy_message = message.to_owned();
            let app_state = app_state.clone();
            footer = footer
                .child(
                    Button::new("fpl-copy")
                        .outline()
                        .label("Copy")
                        .icon(IconName::Copy)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_message.clone()));
                            window.push_notification(
                                (NotificationType::Success, "FPL message copied."),
                                cx,
                            );
                        }),
                )
                .child(Button::new("fpl-save").primary().label("Save…").on_click(
                    move |_, window, cx| {
                        // The portal save dialog takes over; the FPL
                        // dialog has done its job.
                        window.close_dialog(cx);
                        app_state.update(cx, |state, cx| state.export_fpl(cx));
                    },
                ));
        }
        dialog.footer(footer)
    });
}

fn render_body(body: &FplDialogBody, cx: &App) -> AnyElement {
    match body {
        FplDialogBody::Ready { message } => v_flex()
            .gap_2()
            .child(
                // Markdown code fence: monospace block, selectable for
                // manual copy into a filing form.
                TextView::markdown(
                    "fpl-message",
                    SharedString::from(format!("```\n{message}\n```")),
                )
                .selectable(true),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::Check)
                            .small()
                            .text_color(cx.theme().success),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().success)
                            .child("Passes local format validation (items 7–19)."),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Local check only — no online filing in this milestone."),
            )
            .into_any_element(),
        FplDialogBody::Invalid { error } => v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_start()
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .small()
                            .text_color(cx.theme().danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Fix the named item (aircraft profile, route, or the pilot data \
                         in Settings) and reopen this dialog.",
                    ),
            )
            .into_any_element(),
        FplDialogBody::NotReady { reason } => div()
            .whitespace_normal()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(reason.clone())
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use strata_plan::fpl::FplError;

    use super::*;

    #[test]
    fn ready_outcome_shows_the_message_and_export_actions() {
        let outcome = FplOutcome::Ready("(FPL-DEABC-VG\n-C172/L-SDFGY/S\n…)".to_owned());
        let body = dialog_body(&outcome);
        assert_eq!(
            body,
            FplDialogBody::Ready {
                message: "(FPL-DEABC-VG\n-C172/L-SDFGY/S\n…)".to_owned()
            }
        );
        assert_eq!(
            export_message(&body),
            Some("(FPL-DEABC-VG\n-C172/L-SDFGY/S\n…)")
        );
    }

    #[test]
    fn invalid_outcome_names_the_item_and_disables_export() {
        let outcome = FplOutcome::Invalid(FplError::MissingData {
            item: 19,
            what: "the pilot in command",
        });
        let body = dialog_body(&outcome);
        let FplDialogBody::Invalid { error } = &body else {
            panic!("invalid maps to Invalid");
        };
        assert_eq!(error, "FPL item 19 requires the pilot in command");
        assert_eq!(export_message(&body), None);
    }

    #[test]
    fn not_computed_and_no_flight_read_as_reasons() {
        let body = dialog_body(&FplOutcome::NotComputed(
            "the flight has not been computed yet".to_owned(),
        ));
        assert_eq!(
            body,
            FplDialogBody::NotReady {
                reason: "The flight plan cannot be generated yet: the flight has not been \
                         computed yet."
                    .to_owned()
            }
        );
        assert_eq!(export_message(&body), None);

        let body = dialog_body(&FplOutcome::NoFlight);
        assert!(matches!(body, FplDialogBody::NotReady { .. }));
        assert_eq!(export_message(&body), None);
    }
}

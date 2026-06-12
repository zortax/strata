//! The flight library dialog (design §2 "Open…"): the settings-modal
//! pattern at ~600 px — a scrollable list of the `<data_dir>/flights/`
//! scan (name, route, modified) with open/delete per row and a New Flight
//! button. Deleting asks first; opening routes through the dirty guard.

use std::path::PathBuf;
use std::time::SystemTime;

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    label::Label,
    v_flex,
};

use crate::app::RootView;
use crate::assets::IconName;
use crate::flight_io::{self, FlightSummary};
use crate::state::AppState;

/// Dialog width — the smaller sibling of the 980 px settings modal.
const DIALOG_WIDTH_PX: f32 = 600.;
/// Fixed list height; rows scroll inside it.
const LIST_HEIGHT_PX: f32 = 380.;

/// Opens the library dialog. Like the settings modal, the content view is
/// created once per open (the dialog builder re-runs every frame, so the
/// entity is captured, not constructed, inside it).
pub fn open_library_dialog(root: &RootView, window: &mut Window, cx: &mut Context<RootView>) {
    let app_state = root.app_state.clone();
    let root_entity = cx.entity();
    let view = cx.new(|cx| LibraryView::new(app_state, root_entity, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Flight library")
            .w(px(DIALOG_WIDTH_PX))
            .overlay_closable(true)
            .child(view.clone())
    });
}

/// Dialog content: owns the background directory scan and the row actions.
pub struct LibraryView {
    app_state: Entity<AppState>,
    root: Entity<RootView>,
    /// `None` while the (re-)scan runs — the scan is fast, but it is real
    /// file IO and stays off the UI thread.
    flights: Option<Vec<FlightSummary>>,
    _scan_task: Option<Task<()>>,
}

impl LibraryView {
    fn new(
        app_state: Entity<AppState>,
        root: Entity<RootView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            app_state,
            root,
            flights: None,
            _scan_task: None,
        };
        this.rescan(cx);
        this
    }

    /// (Re-)scans the flights directory in the background; replacing the
    /// task cancels a scan still in flight.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let dir = self.app_state.read(cx).flights_dir();
        self.flights = None;
        self._scan_task = Some(cx.spawn(async move |this, cx| {
            let flights = cx
                .background_spawn(async move { flight_io::list_flights(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.flights = Some(flights);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Open a row: dismiss the library, then load through the dirty guard.
    fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.root.update(cx, |root, cx| {
            super::open_flight_guarded(root, path, window, cx);
        });
    }

    /// New Flight from the library: dismiss the dialog, then the shared
    /// guarded create.
    fn new_flight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.root
            .update(cx, |root, cx| super::new_flight(root, window, cx));
    }

    /// Delete a row — after a confirm dialog (stacked over the library).
    fn confirm_delete(&mut self, flight: FlightSummary, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let flight = flight.clone();
            alert
                .title(format!("Delete \"{}\"?", flight.name))
                .description("The flight file is removed permanently.")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .ok_variant(ButtonVariant::Danger),
                )
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    view.update(cx, |this, cx| this.delete(flight.path.clone(), cx));
                    true
                })
        });
    }

    fn delete(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The deleted file leaves the recent list too (persisted only when
        // it actually was in there).
        self.app_state.update(cx, |state, cx| {
            if state.config.forget_recent_flight(&path) {
                crate::ui::settings::persist_config(state, cx);
            }
            // Deleting the open flight's own file leaves the in-memory
            // document as the only copy: drop the dangling path (Save
            // routes to Save As… again) and mark it dirty, so closing it
            // later still prompts instead of silently losing the flight.
            if let Some(flight) = &mut state.flight
                && flight.path.as_deref() == Some(path.as_path())
            {
                flight.path = None;
                flight.dirty = true;
                flight.edit_epoch += 1;
                cx.emit(crate::state::AppStateEvent::FlightChanged);
                cx.notify();
            }
        });
        let remove_path = path.clone();
        let remove = cx.background_spawn(async move { std::fs::remove_file(&remove_path) });
        cx.spawn(async move |this, cx| {
            if let Err(err) = remove.await {
                tracing::warn!(path = %path.display(), %err, "deleting flight file failed");
            }
            // Rescan either way — the row should vanish (or reappear
            // truthfully when the delete failed).
            this.update(cx, |this, cx| this.rescan(cx)).ok();
        })
        .detach();
    }

    fn render_row(
        &self,
        index: usize,
        flight: &FlightSummary,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open_path = flight.path.clone();
        let delete_flight = flight.clone();
        let meta = format!(
            "{} · {}",
            flight.route_summary,
            super::model::modified_label(flight.modified, now)
        );
        h_flex()
            .id(("flight-row", index))
            .w_full()
            .px_2()
            .py_1p5()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .hover(|el| el.bg(cx.theme().accent))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open(open_path.clone(), window, cx);
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().truncate().child(flight.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(meta),
                    ),
            )
            .child(
                Button::new(("flight-delete", index))
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip("Delete flight")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation(); // not a row open
                        this.confirm_delete(delete_flight.clone(), window, cx);
                    })),
            )
    }
}

impl Render for LibraryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = SystemTime::now();
        let body: gpui::AnyElement = match &self.flights {
            None => centered_hint("Scanning the flight library…", cx),
            Some(flights) if flights.is_empty() => {
                centered_hint("No flights yet — create one to get started.", cx)
            }
            Some(flights) => {
                let rows: Vec<_> = flights
                    .iter()
                    .enumerate()
                    .map(|(index, flight)| self.render_row(index, flight, now, cx))
                    .collect();
                div()
                    .id("flight-library-list")
                    .h(px(LIST_HEIGHT_PX))
                    .overflow_y_scroll()
                    .p_1()
                    .child(v_flex().gap_0p5().children(rows))
                    .into_any_element()
            }
        };
        v_flex()
            .w_full()
            .gap_2()
            // The list sits in its own bordered box (the Loading tab's
            // sub-panel recipe), visually separated from the dialog header.
            .child(
                div()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover.opacity(0.6))
                    .overflow_hidden()
                    .child(body),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("library-new-flight")
                        .primary()
                        .label("New Flight")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.new_flight(window, cx);
                        })),
                ),
            )
    }
}

/// Loading / empty placeholder filling the list slot.
fn centered_hint(text: &'static str, cx: &Context<LibraryView>) -> gpui::AnyElement {
    div()
        .h(px(LIST_HEIGHT_PX))
        .v_flex()
        .items_center()
        .justify_center()
        .child(
            Label::new(text)
                .text_sm()
                .text_color(cx.theme().muted_foreground),
        )
        .into_any_element()
}

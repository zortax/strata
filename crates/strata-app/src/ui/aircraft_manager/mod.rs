//! The aircraft profile manager (design §3.5): the settings-modal pattern —
//! a large edge-to-edge dialog with the profile library on the left
//! (select / duplicate / delete / "New aircraft") and the parametric
//! editor sections on the right (identity, performance, fuel system,
//! weight & balance incl. the interactive CG-envelope plot, distances).
//!
//! Every edit saves to the profile file on change (atomic) and broadcasts
//! through [`AppState::upsert_aircraft_profile`], so an open flight planned
//! with the edited aircraft recomputes live. Validation warns, never
//! blocks (see [`validation`]).
//!
//! Opened from the Flight menu; [`open_aircraft_manager`] is the seam the
//! flight panel's aircraft selector calls too.

mod editor;
mod envelope;
mod fields;
mod plot;
mod validation;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_component::dialog::DialogButtonProps;
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, TITLE_BAR_HEIGHT, WindowExt as _,
    h_flex, v_flex,
};
use strata_data::domain::Meters;
use strata_plan::AircraftProfile;
use strata_plan::aircraft::{AircraftId, PowerSetting, StationKind, WbStation};
use strata_plan::units::{Knots, LitersPerHour};

use crate::app::RootView;
use crate::assets::IconName;
use crate::flight_io;
use crate::state::AppState;

use editor::ProfileEditor;

/// Same footprint as the settings modal (sidebar + editor page).
const DIALOG_WIDTH_PX: f32 = 980.;
const DIALOG_BODY_HEIGHT_PX: f32 = 700.;
/// Profile list width on the left.
const LIST_WIDTH_PX: f32 = 230.;

/// Opens the aircraft manager dialog (the settings-modal recipe:
/// headerless, zero-padded, content view created once per open — see
/// `ui::settings::open_settings_dialog` for the pattern's rationale).
pub fn open_aircraft_manager(root: &RootView, window: &mut Window, cx: &mut Context<RootView>) {
    let app_state = root.app_state.clone();
    let view = cx.new(|cx| AircraftManagerView::new(app_state, window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        let dialog_height = px(DIALOG_BODY_HEIGHT_PX + 2.); // body + 1px borders
        let free = (window.viewport_size().height - dialog_height).max(px(0.));
        let margin_top = (free * 0.45).max(TITLE_BAR_HEIGHT);
        dialog
            .w(px(DIALOG_WIDTH_PX))
            .p_0()
            .close_button(false)
            .margin_top(margin_top)
            .overlay_closable(true)
            .child(view.clone())
    });
}

/// Dialog content: the profile list, the working draft of the selected
/// profile and its editor's input entities.
pub struct AircraftManagerView {
    app_state: Entity<AppState>,
    selected: Option<AircraftId>,
    /// Working copy of the selected profile — the single mutation target;
    /// every commit clones it into [`AppState::upsert_aircraft_profile`].
    draft: Option<AircraftProfile>,
    editor: Option<ProfileEditor>,
    _subscriptions: Vec<Subscription>,
}

impl AircraftManagerView {
    fn new(app_state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&app_state, |_, _, cx| cx.notify())];
        let mut this = Self {
            app_state,
            selected: None,
            draft: None,
            editor: None,
            _subscriptions: subscriptions,
        };
        // Initial selection: the open flight's aircraft, or the first
        // library profile.
        let initial = {
            let state = this.app_state.read(cx);
            state
                .flight_aircraft()
                .map(|p| p.id.clone())
                .or_else(|| state.aircraft_library.first().map(|p| p.id.clone()))
        };
        if let Some(id) = initial {
            this.select_profile(id, window, cx);
        }
        this
    }

    pub(crate) fn draft(&self) -> Option<&AircraftProfile> {
        self.draft.as_ref()
    }

    // --- selection & structure ------------------------------------------------

    fn select_profile(&mut self, id: AircraftId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        let profile = self.app_state.read(cx).aircraft_profile(&id).cloned();
        self.selected = profile.is_some().then_some(id);
        self.draft = profile;
        self.rebuild_editor(window, cx);
    }

    /// Recreates the editor's input entities from the draft — profile
    /// switches and structural row changes only (value edits must never
    /// rebuild, or focus would drop mid-keystroke).
    fn rebuild_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = self
            .draft
            .clone()
            .map(|draft| ProfileEditor::new(&draft, window, cx));
        cx.notify();
    }

    /// Applies `edit` to the draft and pushes the result through the
    /// save-and-broadcast funnel (atomic file write, library update,
    /// open-flight recompute).
    pub(crate) fn commit_draft_edit(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut AircraftProfile),
    ) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        let before = draft.clone();
        edit(draft);
        if *draft == before {
            return; // no-op edit: no disk write, no recompute churn
        }
        let profile = draft.clone();
        self.app_state
            .update(cx, |state, cx| state.upsert_aircraft_profile(profile, cx));
        cx.notify();
    }

    /// Draft-only mutation (no disk write, no broadcast) — the live half
    /// of an envelope drag; the release commits.
    pub(crate) fn update_draft_in_memory(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut AircraftProfile),
    ) {
        if let Some(draft) = &mut self.draft {
            edit(draft);
            cx.notify();
        }
    }

    // --- structural row edits (rebuild the editor) ------------------------------

    pub(crate) fn add_cruise_setting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_draft_edit(cx, |p| {
            p.performance.cruise_settings.push(PowerSetting {
                name: format!("Setting {}", p.performance.cruise_settings.len() + 1),
                tas: Knots(0.0),
                fuel_flow: LitersPerHour(0.0),
            });
        });
        self.rebuild_editor(window, cx);
    }

    pub(crate) fn remove_cruise_setting(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_draft_edit(cx, |p| {
            if index < p.performance.cruise_settings.len() {
                p.performance.cruise_settings.remove(index);
            }
        });
        self.rebuild_editor(window, cx);
    }

    pub(crate) fn add_station(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_draft_edit(cx, |p| {
            p.weight_balance.stations.push(WbStation {
                name: format!("Station {}", p.weight_balance.stations.len() + 1),
                arm: Meters(0.0),
                kind: StationKind::Other,
                max_load: None,
            });
        });
        self.rebuild_editor(window, cx);
    }

    pub(crate) fn remove_station(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_draft_edit(cx, |p| {
            if index < p.weight_balance.stations.len() {
                p.weight_balance.stations.remove(index);
            }
        });
        self.rebuild_editor(window, cx);
    }

    // --- library actions ---------------------------------------------------------

    /// "New aircraft": the C172-class example values under a fresh id —
    /// clearly a template (design §3.5), selected for editing right away.
    fn new_aircraft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.app_state.read(cx).aircraft_dir();
        let id = flight_io::allocate_aircraft_id(&dir, "new aircraft");
        let profile = flight_io::aircraft::new_aircraft_template(id.clone());
        self.app_state
            .update(cx, |state, cx| state.upsert_aircraft_profile(profile, cx));
        self.select_profile(id, window, cx);
    }

    fn duplicate_profile(&mut self, id: &AircraftId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.app_state.read(cx).aircraft_profile(id).cloned() else {
            return;
        };
        let dir = self.app_state.read(cx).aircraft_dir();
        let mut copy = source;
        copy.id = flight_io::allocate_aircraft_id(&dir, &format!("{id}-copy"));
        copy.name = Some(match &copy.name {
            Some(name) => format!("{name} (copy)"),
            None => format!("{id} (copy)"),
        });
        let new_id = copy.id.clone();
        self.app_state
            .update(cx, |state, cx| state.upsert_aircraft_profile(copy, cx));
        self.select_profile(new_id, window, cx);
    }

    /// Delete with confirmation (stacked alert over the manager dialog,
    /// like the flight library's delete).
    fn confirm_delete(&mut self, profile_id: AircraftId, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let profile_id = profile_id.clone();
            alert
                .title(format!("Delete \"{name}\"?"))
                .description(
                    "The profile file is removed permanently. Flights referencing it keep \
                     the reference and stop computing until another aircraft is selected.",
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .ok_variant(ButtonVariant::Danger),
                )
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.delete_profile(&profile_id, window, cx));
                    true
                })
        });
    }

    fn delete_profile(&mut self, id: &AircraftId, window: &mut Window, cx: &mut Context<Self>) {
        self.app_state
            .update(cx, |state, cx| state.delete_aircraft_profile(id, cx));
        if self.selected.as_ref() == Some(id) {
            self.selected = None;
            self.draft = None;
            self.editor = None;
            let next = self
                .app_state
                .read(cx)
                .aircraft_library
                .first()
                .map(|p| p.id.clone());
            if let Some(next) = next {
                self.select_profile(next, window, cx);
            }
        }
        cx.notify();
    }

    // --- rendering -----------------------------------------------------------------

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let profiles: Vec<(AircraftId, String, String)> = self
            .app_state
            .read(cx)
            .aircraft_library
            .iter()
            .map(|p| {
                let title = p
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| p.id.to_string());
                let mut meta: Vec<&str> = Vec::new();
                if !p.registration.is_empty() {
                    meta.push(&p.registration);
                }
                if !p.type_designator.is_empty() {
                    meta.push(&p.type_designator);
                }
                (p.id.clone(), title, meta.join(" · "))
            })
            .collect();

        let corner_radius = (cx.theme().radius_lg - px(1.)).max(px(0.));
        let mut list = v_flex().gap_0p5();
        if profiles.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No aircraft profiles yet."),
            );
        }
        for (index, (id, title, meta)) in profiles.into_iter().enumerate() {
            let is_selected = self.selected.as_ref() == Some(&id);
            let select_id = id.clone();
            let duplicate_id = id.clone();
            let delete_id = id.clone();
            let delete_name = title.clone();
            list = list.child(
                h_flex()
                    .id(("aircraft-row", index))
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_1()
                    .items_center()
                    .rounded(cx.theme().radius)
                    .when(is_selected, |el| el.bg(cx.theme().sidebar_accent))
                    .hover(|el| el.bg(cx.theme().sidebar_accent.opacity(0.7)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_profile(select_id.clone(), window, cx);
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_sm().truncate().child(title))
                            .when(!meta.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(meta),
                                )
                            }),
                    )
                    .child(
                        Button::new(("aircraft-duplicate", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Duplicate profile")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.duplicate_profile(&duplicate_id, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("aircraft-delete", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Delete profile")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.confirm_delete(
                                    delete_id.clone(),
                                    delete_name.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        }

        v_flex()
            .w(px(LIST_WIDTH_PX))
            .h_full()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .rounded_l(corner_radius)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                h_flex()
                    .px_3()
                    .py_2p5()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Plane).small())
                    .child(div().text_sm().font_semibold().child("Aircraft")),
            )
            .child(
                div()
                    .id("aircraft-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1p5()
                    .child(list),
            )
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        Button::new("aircraft-new")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("New aircraft")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_aircraft(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Seeded from the C172-class example — replace with POH values."),
                    ),
            )
    }

    fn render_warnings(&self, draft: &AircraftProfile, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let warnings = validation::warnings(draft);
        if warnings.is_empty() {
            return None;
        }
        let mut listing = v_flex().gap_1();
        for warning in warnings {
            listing = listing.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().flex_shrink_0().mt_0p5().child(Icon::new(IconName::TriangleAlert).xsmall()))
                    .child(div().flex_1().min_w_0().text_sm().child(warning)),
            );
        }
        Some(
            v_flex()
                .w_full()
                .p_3()
                .gap_1()
                .rounded(cx.theme().radius)
                .bg(cx.theme().warning.opacity(0.12))
                .border_1()
                .border_color(cx.theme().warning.opacity(0.35))
                .text_color(cx.theme().warning)
                .child(listing),
        )
    }

    fn render_editor(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (Some(draft), Some(editor)) = (self.draft.clone(), self.editor.as_ref()) else {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select an aircraft — or create one.")
                .into_any_element();
        };
        let warnings = self.render_warnings(&draft, cx);
        let sections = editor::render_sections(editor, &draft, cx);
        div()
            .id("aircraft-editor")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_4()
                    .gap_4()
                    .children(warnings)
                    .children(sections),
            )
            .into_any_element()
    }
}

impl Render for AircraftManagerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w_full().h(px(DIALOG_BODY_HEIGHT_PX)).child(
            h_flex()
                .size_full()
                .items_start()
                .child(self.render_list(cx))
                .child(self.render_editor(cx)),
        )
    }
}

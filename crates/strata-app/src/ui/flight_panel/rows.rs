//! Route list (design §3.1): one row per waypoint — kind glyph, ident/
//! name — with the leg fields *to the next waypoint* on a connector row
//! underneath (editable altitude + the computed dist/MH/GS/ETE readout).
//! Hover affordances: reorder (up/down — the pinned gpui-component List
//! has no drag-reorder), delete, and insert-into-leg. Alternates are
//! listed separately at the bottom (added via the map's right-click flow,
//! removable here).
//!
//! Hovering a waypoint/alternate row also emphasizes its handle on the
//! map: enter/leave write [`AppState::set_route_highlight`] /
//! [`AppState::clear_route_highlight`] with the row's renderer vertex id
//! (route index, `ALTERNATE_ID_BASE + i` for alternates — the
//! `render_route` contract); the map view pushes it to the route layer.
//!
//! [`AppState::set_route_highlight`]: crate::state::AppState::set_route_highlight
//! [`AppState::clear_route_highlight`]: crate::state::AppState::clear_route_highlight

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};
use strata_plan::flight::RoutePoint;

use crate::app::RootView;
use crate::assets::IconName;
use crate::state::flight::render_route::ALTERNATE_ID_BASE;

use super::model::{self, LegReadout};

/// Group name keying the hover affordances of one list row.
const ROW_GROUP: &str = "fp-row";

pub(super) fn render_route_list(root: &RootView, cx: &Context<RootView>) -> AnyElement {
    let Some(panel) = &root.flight_panel else {
        return div().into_any_element();
    };
    let doc = &panel.snapshot.doc;
    let readouts: Vec<LegReadout> = panel
        .snapshot
        .computed
        .as_deref()
        .map(|computed| model::leg_readouts(&computed.legs, &computed.navlog.rows))
        .unwrap_or_default();

    let mut list = v_flex().py_1();

    list = list.child(section_label("Route", cx));
    if doc.route.is_empty() {
        list = list.child(hint("Right-click the map to add waypoints.", cx));
    }
    let count = doc.route.len();
    for (index, waypoint) in doc.route.iter().enumerate() {
        list = list.child(waypoint_row(index, count, &waypoint.point, cx));
        if index + 1 < count {
            let input = panel.leg_altitude_inputs.get(index);
            list = list.child(leg_row(index, input, readouts.get(index), cx));
        }
    }

    list = list.child(div().mt_2().child(section_label("Alternates", cx)));
    if doc.alternates.is_empty() {
        list = list.child(hint("Right-click the map to set an alternate.", cx));
    }
    for (index, alternate) in doc.alternates.iter().enumerate() {
        list = list.child(alternate_row(index, alternate, cx));
    }

    list.into_any_element()
}

// --- rows ---------------------------------------------------------------------

/// One waypoint row: glyph, ident/name, hover reorder/delete affordances.
/// Clicking flies the map to the point.
fn waypoint_row(
    index: usize,
    count: usize,
    point: &RoutePoint,
    cx: &Context<RootView>,
) -> impl IntoElement + use<> {
    let position = point.position();
    let zoom = model::waypoint_fly_zoom(point);
    h_flex()
        .id(("fp-wp", index))
        .group(ROW_GROUP)
        .mx_2()
        .px_2()
        .py_1()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .on_hover(
            cx.listener(move |this: &mut RootView, hovered: &bool, _, cx| {
                this.app_state.update(cx, |state, cx| match hovered {
                    true => state.set_route_highlight(Some(index as u64), cx),
                    false => state.clear_route_highlight(index as u64, cx),
                });
            }),
        )
        .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
            this.map_view.update(cx, |map, cx| {
                map.fly_to(position.lat(), position.lon(), zoom, cx)
            });
        }))
        .child(
            Icon::new(model::waypoint_icon(point))
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(model::waypoint_title(point)),
                )
                .children(model::waypoint_subtitle(point).map(|subtitle| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(subtitle)
                })),
        )
        .child(
            hover_affordances()
                .when(index > 0, |el| {
                    el.child(
                        Button::new(("fp-wp-up", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronUp)
                            .tooltip("Move up")
                            .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                                this.app_state.update(cx, |state, cx| {
                                    state.move_waypoint(index, index - 1, cx);
                                });
                            })),
                    )
                })
                .when(index + 1 < count, |el| {
                    el.child(
                        Button::new(("fp-wp-down", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .tooltip("Move down")
                            .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                                this.app_state.update(cx, |state, cx| {
                                    state.move_waypoint(index, index + 1, cx);
                                });
                            })),
                    )
                })
                .child(
                    Button::new(("fp-wp-delete", index))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip("Remove waypoint")
                        .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                            this.app_state.update(cx, |state, cx| {
                                state.remove_waypoint(index, cx);
                            });
                        })),
                ),
        )
}

/// The leg row under waypoint `index`: editable altitude (placeholder =
/// inherited cruise), the computed readout (em-dash while not computable)
/// and the hover "+" that splits the leg at its midpoint.
fn leg_row(
    index: usize,
    altitude_input: Option<&gpui::Entity<gpui_component::input::InputState>>,
    readout: Option<&LegReadout>,
    cx: &Context<RootView>,
) -> impl IntoElement + use<> {
    h_flex()
        .id(("fp-leg", index))
        .group(ROW_GROUP)
        .mx_2()
        .pl(px(30.))
        .pr_2()
        .py_0p5()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(60.))
                .flex_shrink_0()
                .children(altitude_input.map(|input| Input::new(input).xsmall())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(model::leg_readout_text(readout)),
        )
        .child(
            hover_affordances().child(
                Button::new(("fp-leg-insert", index))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("Insert waypoint into this leg")
                    .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                        this.app_state.update(cx, |state, cx| {
                            let point = state
                                .flight
                                .as_ref()
                                .and_then(|f| model::leg_insert_point(&f.doc.route, index));
                            if let Some(point) = point {
                                state.insert_waypoint(index + 1, point, cx);
                            }
                        });
                    })),
            ),
        )
}

/// One alternate row: glyph, label, hover remove. Clicking flies to it.
fn alternate_row(
    index: usize,
    point: &RoutePoint,
    cx: &Context<RootView>,
) -> impl IntoElement + use<> {
    let position = point.position();
    let zoom = model::waypoint_fly_zoom(point);
    let highlight_id = ALTERNATE_ID_BASE + index as u64;
    h_flex()
        .id(("fp-alt", index))
        .group(ROW_GROUP)
        .mx_2()
        .px_2()
        .py_1()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .on_hover(
            cx.listener(move |this: &mut RootView, hovered: &bool, _, cx| {
                this.app_state.update(cx, |state, cx| match hovered {
                    true => state.set_route_highlight(Some(highlight_id), cx),
                    false => state.clear_route_highlight(highlight_id, cx),
                });
            }),
        )
        .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
            this.map_view.update(cx, |map, cx| {
                map.fly_to(position.lat(), position.lon(), zoom, cx)
            });
        }))
        .child(
            Icon::new(model::waypoint_icon(point))
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(model::waypoint_title(point)),
                )
                .children(model::waypoint_subtitle(point).map(|subtitle| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(subtitle)
                })),
        )
        .child(
            hover_affordances().child(
                Button::new(("fp-alt-delete", index))
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip("Remove alternate")
                    .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                        this.app_state.update(cx, |state, cx| {
                            state.edit_flight_doc(cx, |doc| {
                                if index < doc.alternates.len() {
                                    doc.alternates.remove(index);
                                    true
                                } else {
                                    false
                                }
                            });
                        });
                    })),
            ),
        )
}

// --- shared bits -----------------------------------------------------------------

/// Affordance cluster that fades in when its row is hovered (the buttons
/// stay hit-testable while transparent — they are inside the row anyway).
fn hover_affordances() -> gpui::Div {
    h_flex()
        .gap_0p5()
        .flex_shrink_0()
        .opacity(0.)
        .group_hover(ROW_GROUP, |s| s.opacity(1.))
}

fn section_label(label: &str, cx: &Context<RootView>) -> impl IntoElement {
    div()
        .px_4()
        .py_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.to_uppercase())
}

fn hint(text: &'static str, cx: &Context<RootView>) -> impl IntoElement {
    div()
        .px_4()
        .py_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

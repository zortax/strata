//! Search box floating over the top-left of the map (frosted, like the
//! info panel) with a results dropdown (store ident/name search).
//!
//! Deliberately NOT in the title bar: on Wayland compositors the CSD drag
//! region swallows title-bar clicks, so an input there never gets focus.
//!
//! In planning mode the card relocates into the flight panel's header
//! area (design §3.1) and result rows gain a second action: ↵/click flies
//! to the result exactly as today, the small "+" appends it to the route.
//! In explorer mode the search renders exactly as it always has.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, InteractiveElement as _, IntoElement, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, deferred, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};
use strata_data::store::{Feature, SearchHit};
use strata_plan::flight::{FreePoint, NamedPoint, NamedPointKind, RoutePoint};

use crate::app::RootView;
use crate::app::panel_animation::PanelVisibility;
use crate::assets::IconName;
use crate::ui::{feature_icon, feature_kind_label};

pub const SEARCH_WIDTH_PX: f32 = 320.0;

/// Height of the frosted input card: small input (~28) + p_1 (2×4) + border (2×1).
const SEARCH_BOX_HEIGHT_PX: f32 = 38.0;

/// Height of the panel variant's search section: small input (~28) +
/// p_2 (2×8) — the dropdown anchors right below it.
const PANEL_SEARCH_HEIGHT_PX: f32 = 44.0;

/// Where the search card lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPlacement {
    /// Floating over the top-left of the map (explorer mode — unchanged).
    Explorer,
    /// Inside the flight panel's header area (planning mode).
    FlightPanel,
}

/// Pure placement decision: the flight panel owns the search card for as
/// long as it is on screen — including its exit animation, so the card
/// does not pop back to the map corner under the closing panel.
pub fn search_placement(flight_panel: PanelVisibility) -> SearchPlacement {
    match flight_panel {
        PanelVisibility::Closed => SearchPlacement::Explorer,
        PanelVisibility::Open | PanelVisibility::Closing => SearchPlacement::FlightPanel,
    }
}

/// The route point appending a search result creates: named features
/// become re-resolvable [`NamedPoint`]s (an airport without an ICAO ident
/// falls back to a named free point — there is no stable id to reference),
/// area/obstacle hits are not routable.
pub fn route_point_for_hit(feature: &Feature) -> Option<RoutePoint> {
    match feature {
        Feature::Airport(airport) => Some(match &airport.ident {
            Some(icao) => RoutePoint::Named(NamedPoint {
                kind: NamedPointKind::Airport,
                id: icao.as_str().to_owned(),
                name: airport.name.clone(),
                position: airport.position,
            }),
            None => RoutePoint::Free(FreePoint {
                name: Some(airport.name.clone()),
                position: airport.position,
            }),
        }),
        Feature::Navaid(navaid) => Some(RoutePoint::Named(NamedPoint {
            kind: NamedPointKind::Navaid,
            id: navaid.ident.clone(),
            name: navaid.name.clone(),
            position: navaid.position,
        })),
        Feature::ReportingPoint(point) => Some(RoutePoint::Named(NamedPoint {
            kind: NamedPointKind::ReportingPoint,
            id: point.name.clone(),
            name: point.name.clone(),
            position: point.position,
        })),
        Feature::Airspace(_) | Feature::Obstacle(_) => None,
    }
}

/// Explorer placement: the floating card over the map's top-left, exactly
/// as it always rendered (rows without the append affordance).
pub fn render_search(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let open = root.search_open && !root.search_results.is_empty();

    let rows = root
        .search_results
        .iter()
        .enumerate()
        .map(|(ix, hit)| search_hit_row(hit, ix, false, cx));

    div()
        .occlude()
        .absolute()
        .top_3()
        .left_3()
        .w(px(SEARCH_WIDTH_PX))
        .child(
            div()
                .p_1()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background.opacity(0.78))
                .backdrop_blur(px(18.))
                .shadow_lg()
                .child(
                    Input::new(&root.search_input)
                        .small()
                        .appearance(false)
                        .prefix(
                            Icon::new(IconName::Search)
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        ),
                ),
        )
        .when(open, |el| {
            el.child(deferred(
                v_flex()
                    .occlude()
                    .absolute()
                    .top(px(SEARCH_BOX_HEIGHT_PX + 4.))
                    .left_0()
                    .w_full()
                    .max_h(px(320.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background.opacity(0.78))
                    .backdrop_blur(px(18.))
                    .text_color(cx.theme().popover_foreground)
                    .shadow_lg()
                    .py_1()
                    .overflow_hidden()
                    .children(rows),
            ))
        })
}

/// Flight-panel placement: the same search, as the panel's top section;
/// result rows additionally offer "+ add to route".
pub fn render_panel_search(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let open = root.search_open && !root.search_results.is_empty();

    let rows = root
        .search_results
        .iter()
        .enumerate()
        .map(|(ix, hit)| search_hit_row(hit, ix, true, cx));

    div()
        .relative()
        .p_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            Input::new(&root.search_input).small().prefix(
                Icon::new(IconName::Search)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            ),
        )
        .when(open, |el| {
            el.child(deferred(
                v_flex()
                    .occlude()
                    .absolute()
                    .top(px(PANEL_SEARCH_HEIGHT_PX))
                    .left_2()
                    .right_2()
                    .max_h(px(320.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background.opacity(0.92))
                    .backdrop_blur(px(18.))
                    .text_color(cx.theme().popover_foreground)
                    .shadow_lg()
                    .py_1()
                    .overflow_hidden()
                    .children(rows),
            ))
        })
}

/// One result row. `with_append` (planning mode) adds the "+" button on
/// routable hits; with it `false` the element tree is exactly the
/// explorer's. Clicking the row stays fly-to-and-select in both modes.
fn search_hit_row(
    hit: &SearchHit,
    ix: usize,
    with_append: bool,
    cx: &Context<RootView>,
) -> Stateful<Div> {
    let appendable = with_append && route_point_for_hit(&hit.feature).is_some();
    h_flex()
        .id(("search-hit", ix))
        .mx_1()
        .px_2()
        .py_1()
        .gap_2()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .hover(|s| s.bg(cx.theme().accent))
        .child(
            Icon::new(feature_icon(&hit.feature))
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(hit.label.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(feature_kind_label(&hit.feature)),
        )
        .when(appendable, |el| {
            el.child(
                Button::new(("search-hit-add", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("Add to route")
                    .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
                        this.append_search_hit_to_route(ix, cx);
                    })),
            )
        })
        .on_click(cx.listener(move |this: &mut RootView, _, window, cx| {
            this.activate_search_hit(ix, window, cx);
        }))
}

#[cfg(test)]
mod tests {
    use strata_data::domain::{
        AirportKind, IcaoCode, LatLon, MetersAgl, MetersAmsl, NavaidKind, Obstacle, ObstacleKind,
    };

    use super::*;

    fn position() -> LatLon {
        LatLon::new(50.0, 8.0).unwrap()
    }

    fn airport(ident: Option<&str>) -> Feature {
        Feature::Airport(strata_data::domain::Airport {
            ident: ident.map(|i| IcaoCode::new(i).unwrap()),
            name: "Frankfurt-Egelsbach".to_owned(),
            kind: AirportKind::Airfield,
            position: position(),
            elevation: MetersAmsl(116.0),
            runways: Vec::new(),
            frequencies: Vec::new(),
        })
    }

    #[test]
    fn the_flight_panel_owns_search_for_its_whole_lifetime() {
        // Explorer mode: byte-identical placement to today.
        assert_eq!(
            search_placement(PanelVisibility::Closed),
            SearchPlacement::Explorer
        );
        // Planning mode — and the exit animation: the card must not pop
        // back to the map corner under the closing panel.
        assert_eq!(
            search_placement(PanelVisibility::Open),
            SearchPlacement::FlightPanel
        );
        assert_eq!(
            search_placement(PanelVisibility::Closing),
            SearchPlacement::FlightPanel
        );
    }

    #[test]
    fn named_features_append_as_re_resolvable_named_points() {
        let Some(RoutePoint::Named(point)) = route_point_for_hit(&airport(Some("EDFE"))) else {
            panic!("airport with ident appends as a named point");
        };
        assert_eq!(point.kind, NamedPointKind::Airport);
        assert_eq!(point.id, "EDFE");
        assert_eq!(point.name, "Frankfurt-Egelsbach");
        assert_eq!(point.position, position());

        let navaid = Feature::Navaid(strata_data::domain::Navaid {
            ident: "FFM".to_owned(),
            name: "Frankfurt".to_owned(),
            kind: NavaidKind::Vor,
            frequency: None,
            channel: None,
            position: position(),
            elevation: MetersAmsl(100.0),
        });
        let Some(RoutePoint::Named(point)) = route_point_for_hit(&navaid) else {
            panic!("navaid appends as a named point");
        };
        assert_eq!(point.kind, NamedPointKind::Navaid);
        assert_eq!(point.id, "FFM");

        let rp = Feature::ReportingPoint(strata_data::domain::ReportingPoint {
            name: "ECHO 1".to_owned(),
            mandatory: true,
            position: position(),
            airports: Vec::new(),
        });
        let Some(RoutePoint::Named(point)) = route_point_for_hit(&rp) else {
            panic!("reporting point appends as a named point");
        };
        assert_eq!(point.kind, NamedPointKind::ReportingPoint);
        assert_eq!(point.id, "ECHO 1");
    }

    #[test]
    fn ident_less_airports_fall_back_to_a_named_free_point() {
        let Some(RoutePoint::Free(point)) = route_point_for_hit(&airport(None)) else {
            panic!("no stable id to reference — must be a free point");
        };
        assert_eq!(point.name.as_deref(), Some("Frankfurt-Egelsbach"));
        assert_eq!(point.position, position());
    }

    #[test]
    fn area_and_obstacle_hits_are_not_routable() {
        let obstacle = Feature::Obstacle(Obstacle {
            name: Some("Mast".to_owned()),
            kind: ObstacleKind::Mast,
            position: position(),
            elevation_top: MetersAmsl(300.0),
            height: MetersAgl(120.0),
            lighted: false,
        });
        assert_eq!(route_point_for_hit(&obstacle), None);
    }
}

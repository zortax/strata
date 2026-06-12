//! The left flight panel (design §3.1): the flight's table of contents —
//! header (name, aircraft, departure, cruise quick-set), the route list
//! with per-leg fields, alternates, and the summary card with the status
//! badge row. In planning mode the search card relocates into the panel's
//! header area (see [`crate::ui::search`]).
//!
//! Mounted/unmounted with the established [`PanelAnimation`] machinery —
//! the info panel's slide/fade recipe mirrored to the left edge — and the
//! bottom-left overlay column glides out of the panel's way while it is
//! mounted.
//!
//! [`PanelAnimation`]: crate::app::panel_animation::PanelAnimation

pub mod model;
pub mod state;

mod header;
mod rows;
mod summary;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, ease_out_quint,
    px, quadratic,
};
use gpui_component::{ActiveTheme as _, v_flex};

use crate::app::RootView;
use crate::app::panel_animation::{
    PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PanelAnimation, PanelVisibility,
};
use crate::ui;

pub use state::FlightPanelState;

/// Panel width (design §3.1: ~340 px).
pub const FLIGHT_PANEL_WIDTH_PX: f32 = 340.;
/// Resting inset from the window edges (matches `left_3`/`top_3`/…).
const PANEL_INSET_PX: f32 = 12.;
/// Horizontal travel of the enter/exit animation.
const PANEL_SLIDE_PX: f32 = 20.;
/// Left inset of the bottom-left overlay column while the panel is
/// mounted: the column keeps its own `left_3` rhythm right of the panel.
const COLUMN_SHIFTED_LEFT_PX: f32 = PANEL_INSET_PX + FLIGHT_PANEL_WIDTH_PX + PANEL_INSET_PX;

pub fn render_flight_panel(
    root: &RootView,
    insets: &ui::profile_drawer::insets::PlanningChromeInsets,
    cx: &mut Context<RootView>,
) -> Option<impl IntoElement + use<>> {
    let visibility = root.flight_panel_anim.visibility();
    if visibility == PanelVisibility::Closed {
        return None;
    }
    let panel_state = root.flight_panel.as_ref()?;

    let panel = v_flex()
        .occlude()
        .relative()
        .size_full()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.78))
        .backdrop_blur(px(18.))
        .shadow_lg()
        // Search relocates into the panel's header area in planning mode
        // (design §3.1); results gain the append-to-route affordance.
        .child(ui::search::render_panel_search(root, cx))
        .child(header::render_header(panel_state, cx))
        .child(
            div()
                .id("flight-panel-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(rows::render_route_list(root, cx)),
        )
        .child(summary::render_summary(root, cx));

    // The info panel's one-shot enter/exit recipe, mirrored to the left
    // edge: re-keying by generation/epoch replays the animation; only the
    // panel's own offset+opacity move, nothing else reflows. The slide is
    // a relative offset because the *outer frame* below owns the absolute
    // insets — its bottom is the shared planning-chrome lift above the
    // profile drawer, animated by its own toggle (one element cannot
    // carry two `with_animation`s).
    let panel: AnyElement = match visibility {
        PanelVisibility::Closed => return None, // unreachable: handled above
        PanelVisibility::Open => panel
            .with_animation(
                ("flight-panel-enter", root.flight_panel_anim.open_generation()),
                Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
                |panel, delta| {
                    panel
                        .left(px(-PANEL_SLIDE_PX * (1. - delta)))
                        .opacity(delta)
                },
            )
            .into_any_element(),
        PanelVisibility::Closing => panel
            .with_animation(
                ("flight-panel-exit", root.flight_panel_anim.close_epoch()),
                Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
                |panel, delta| {
                    panel
                        .left(px(-PANEL_SLIDE_PX * delta))
                        .opacity(1. - delta)
                },
            )
            .into_any_element(),
    };

    let frame = div()
        .absolute()
        .top_3()
        .left(px(PANEL_INSET_PX))
        .w(px(FLIGHT_PANEL_WIDTH_PX))
        .child(panel);
    Some(ui::profile_drawer::insets::lift_panel_bottom(
        frame,
        insets,
        "flight-panel-lift",
    ))
}

/// Applies the bottom-left overlay column's left inset for the panel's
/// current state: resting `left_3` in explorer mode, shifted right of the
/// panel while it is mounted, gliding between the two during the panel's
/// enter/exit animation (same durations/easings, keyed by the same
/// generation/epoch).
pub fn shift_bottom_left_column(column: gpui::Div, anim: &PanelAnimation) -> AnyElement {
    const TRAVEL: f32 = COLUMN_SHIFTED_LEFT_PX - PANEL_INSET_PX;
    match anim.visibility() {
        PanelVisibility::Closed => column.left_3().into_any_element(),
        PanelVisibility::Open => column
            .with_animation(
                ("fp-column-shift-in", anim.open_generation()),
                Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
                |column, delta| column.left(px(PANEL_INSET_PX + TRAVEL * delta)),
            )
            .into_any_element(),
        PanelVisibility::Closing => column
            .with_animation(
                ("fp-column-shift-out", anim.close_epoch()),
                Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
                |column, delta| column.left(px(PANEL_INSET_PX + TRAVEL * (1. - delta))),
            )
            .into_any_element(),
    }
}

impl RootView {
    /// The flight panel's "Manage aircraft…" seam: opens the aircraft
    /// manager dialog (its own module/workflow).
    pub(crate) fn open_aircraft_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::aircraft_manager::open_aircraft_manager(self, window, cx);
    }
}

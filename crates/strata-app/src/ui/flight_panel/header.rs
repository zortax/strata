//! Flight panel header (design §3.1): inline-editable flight name,
//! aircraft selector, departure date/time (UTC) and the default-cruise
//! quick-set field.

use gpui::{Context, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_component::date_picker::DatePicker;
use gpui_component::input::Input;
use gpui_component::select::Select;
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use crate::app::RootView;

use super::state::FlightPanelState;

pub(super) fn render_header(
    panel: &FlightPanelState,
    cx: &Context<RootView>,
) -> impl IntoElement {
    v_flex()
        .p_3()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(Input::new(&panel.name_input).small())
        .child(field(
            "Aircraft",
            Select::new(&panel.aircraft_select)
                .small()
                .placeholder("Select aircraft…"),
            cx,
        ))
        .child(
            h_flex()
                .gap_2()
                .items_end()
                .child(div().flex_1().min_w_0().child(field(
                    "Departure (UTC)",
                    DatePicker::new(&panel.date_picker).small(),
                    cx,
                )))
                .child(div().w(px(64.)).flex_shrink_0().child(field(
                    "Time Z",
                    Input::new(&panel.time_input).small(),
                    cx,
                )))
                .child(div().w(px(72.)).flex_shrink_0().child(field(
                    "Cruise ft",
                    Input::new(&panel.cruise_input).small(),
                    cx,
                ))),
        )
}

/// A labelled form field: tiny uppercase label over the control.
fn field(
    label: &'static str,
    control: impl IntoElement,
    cx: &Context<RootView>,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(label.to_uppercase()),
        )
        .child(control)
}

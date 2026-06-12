//! Weather time slider: a frosted pill floating above the layers panel
//! (bottom-left) while at least one gridded weather layer is enabled.
//!
//! Track = `gpui_component::Slider` over the fixed −2 h…+24 h window in
//! minutes relative to "now" (chosen over a hand-rolled drag track: it
//! already does custom min/max/step ranges, continuous `Change` events
//! while scrubbing, and themed thumb/track rendering — exactly what the
//! pill needs). A tick marks "now", ± buttons step by an hour, and the
//! label shows the selected instant ("now" / "Wed 15:00Z"). Appearing and
//! disappearing replay the info panel's slide/fade recipe, driven by the
//! same [`PanelAnimation`] state machine plus the epoch-guarded unmount
//! timer in `RootView`.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Styled as _, div, ease_out_quint, px, quadratic, relative,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    slider::Slider,
};

use crate::app::RootView;
use crate::app::panel_animation::{
    PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PanelAnimation, PanelVisibility,
};
use crate::assets::IconName;
use crate::state::weather_time;

/// Vertical travel of the enter/exit animation (slide-up in, down out).
const SLIDER_SLIDE_PX: f32 = 12.;

/// Laid-out height of the pill: its tallest child is gpui-component's
/// horizontal slider whose bar container is `h_6` (24 px), plus the pill's
/// `py_1` padding (2 × 4 px) and `border_1` (2 × 1 px). The progress panel
/// glides by exactly this (plus [`BOTTOM_LEFT_COLUMN_GAP_PX`]) when the
/// pill (un)mounts beneath it — keep in sync with [`render_time_slider`]'s
/// recipe.
pub const SLIDER_PILL_HEIGHT_PX: f32 = 24. + 2. * 4. + 2. * 1.;

/// Gap of the bottom-left overlay column in `RootView::render_main_area`
/// (the column reads this constant, so the progress panel's glide distance
/// cannot drift from the actual layout).
pub const BOTTOM_LEFT_COLUMN_GAP_PX: f32 = 8.;

/// Pure visibility decision feeding the [`PanelAnimation`] machine: any
/// gridded weather layer on → show, all off → start the exit animation.
/// Returns the close epoch the caller must guard its unmount timer with.
pub fn drive_visibility(anim: &mut PanelAnimation, any_weather_layer_on: bool) -> Option<u64> {
    if any_weather_layer_on {
        anim.open_requested();
        None
    } else {
        anim.close_requested()
    }
}

pub fn render_time_slider(
    root: &RootView,
    cx: &mut Context<RootView>,
) -> Option<impl IntoElement + use<>> {
    let visibility = root.slider_anim.visibility();
    if visibility == PanelVisibility::Closed {
        return None;
    }

    let weather_time = root.app_state.read(cx).weather_time;
    let label = weather_time.label();
    let is_now = weather_time.is_now();

    // Width comes from the bottom-left column in `RootView`: the pill
    // stretches to the layers panel's intrinsic width so both cards align.
    let pill = h_flex()
        .occlude()
        .relative()
        .w_full()
        .px_2()
        .py_1()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.78))
        .backdrop_blur(px(18.))
        .shadow_lg()
        .child(
            Button::new("wx-time-back")
                .ghost()
                .xsmall()
                .icon(IconName::ChevronLeft)
                .tooltip("1 hour back")
                .on_click(cx.listener(|this: &mut RootView, _, _, cx| {
                    this.app_state
                        .update(cx, |state, cx| state.step_weather_time(-1, cx));
                })),
        )
        .child(
            // Track with the "now" tick behind it. The tick sits at the
            // fixed −2 h/+24 h split of the window (the window itself is
            // re-anchored to "now" by the fetch cycles).
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .absolute()
                        .left(relative(weather_time::now_fraction()))
                        .top(px(5.))
                        .h(px(14.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(cx.theme().muted_foreground.opacity(0.7)),
                )
                .child(Slider::new(&root.time_slider).horizontal()),
        )
        .child(
            Button::new("wx-time-forward")
                .ghost()
                .xsmall()
                .icon(IconName::ChevronRight)
                .tooltip("1 hour forward")
                .on_click(cx.listener(|this: &mut RootView, _, _, cx| {
                    this.app_state
                        .update(cx, |state, cx| state.step_weather_time(1, cx));
                })),
        )
        .child(
            // Fixed width so the pill doesn't resize between "now" and
            // "Wed 15:00Z".
            div()
                .w(px(78.))
                .flex_shrink_0()
                .text_xs()
                .text_right()
                .font_weight(FontWeight::SEMIBOLD)
                .when_else(
                    is_now,
                    |el| el.text_color(cx.theme().muted_foreground),
                    |el| el.text_color(cx.theme().foreground),
                )
                .child(label),
        );

    // Same one-shot enter/exit machinery as the info panel: re-keying by
    // generation/epoch replays the animation. The slide is a relative `top`
    // offset (layout-neutral), so the pill keeps its slot in the bottom-left
    // column and the layers panel below never reflows while animating.
    Some(match visibility {
        PanelVisibility::Closed => return None, // unreachable: handled above
        PanelVisibility::Open => pill.with_animation(
            ("wx-time-slider-enter", root.slider_anim.open_generation()),
            Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
            |pill, delta| {
                pill.top(px(SLIDER_SLIDE_PX * (1. - delta)))
                    .opacity(delta)
            },
        ),
        PanelVisibility::Closing => pill.with_animation(
            ("wx-time-slider-exit", root.slider_anim.close_epoch()),
            Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
            |pill, delta| {
                pill.top(px(SLIDER_SLIDE_PX * delta)).opacity(1. - delta)
            },
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_layer_on_shows_the_slider() {
        let mut anim = PanelAnimation::default();
        assert_eq!(drive_visibility(&mut anim, true), None);
        assert_eq!(anim.visibility(), PanelVisibility::Open);
    }

    #[test]
    fn all_layers_off_hides_via_the_closing_state() {
        let mut anim = PanelAnimation::default();
        drive_visibility(&mut anim, true);
        let epoch = drive_visibility(&mut anim, false).expect("close epoch for unmount timer");
        assert_eq!(anim.visibility(), PanelVisibility::Closing);
        assert!(anim.animation_done(epoch), "timer with current epoch closes");
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
    }

    #[test]
    fn toggle_on_during_exit_voids_the_pending_unmount_timer() {
        let mut anim = PanelAnimation::default();
        drive_visibility(&mut anim, true);
        let epoch = drive_visibility(&mut anim, false).expect("closing");
        drive_visibility(&mut anim, true); // another weather layer toggled on
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        assert!(!anim.animation_done(epoch), "stale epoch is ignored");
        assert_eq!(anim.visibility(), PanelVisibility::Open);
    }

    #[test]
    fn pill_height_constant_matches_the_render_recipe() {
        // 24 px slider row (`h_6`) + `py_1` (2 × 4 px) + `border_1` (2 × 1 px).
        assert_eq!(SLIDER_PILL_HEIGHT_PX, 34.);
        // gap_2 == 8 px — the bottom-left column's gap.
        assert_eq!(BOTTOM_LEFT_COLUMN_GAP_PX, 8.);
    }

    #[test]
    fn repeated_states_are_stable() {
        let mut anim = PanelAnimation::default();
        // Hidden stays hidden.
        assert_eq!(drive_visibility(&mut anim, false), None);
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
        // Shown stays shown (second weather layer toggled on).
        drive_visibility(&mut anim, true);
        let generation = anim.open_generation();
        drive_visibility(&mut anim, true);
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        assert_eq!(anim.open_generation(), generation, "no re-entry animation");
        // Closing twice yields no second epoch.
        let first = drive_visibility(&mut anim, false);
        let second = drive_visibility(&mut anim, false);
        assert!(first.is_some() && second.is_none());
    }
}

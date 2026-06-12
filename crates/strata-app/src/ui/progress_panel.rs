//! Ingest progress panel: a frosted card at the top of the bottom-left
//! overlay column (children order: progress panel, weather time slider,
//! layers panel) while an ingest run is in flight.
//!
//! Renders [`crate::state::ingest_progress::IngestProgressVm`] from
//! `AppState`. Mount/unmount replays the slider pill's slide/fade recipe
//! via the shared [`PanelAnimation`] machine, and a one-shot [`Glide`]
//! compensates the column reflow when the slider pill mounts/unmounts
//! beneath the panel so it moves smoothly instead of jumping.

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Styled as _, div, ease_out_quint, px, quadratic,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    v_flex,
};

use crate::app::RootView;
use crate::app::panel_animation::{
    PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PanelAnimation, PanelVisibility,
};
use crate::assets::IconName;
use crate::state::ingest_progress::JobState;
use crate::ui::time_slider::{BOTTOM_LEFT_COLUMN_GAP_PX, SLIDER_PILL_HEIGHT_PX};

/// Vertical travel of the enter/exit animation (matches the slider pill).
const PANEL_SLIDE_PX: f32 = 12.;

/// Vertical space one slider-pill slot occupies in the bottom-left column —
/// the pill itself plus the column gap. Exactly how far the panel's layout
/// position jumps when the pill (un)mounts below it.
pub const SLIDER_SLOT_PX: f32 = SLIDER_PILL_HEIGHT_PX + BOTTOM_LEFT_COLUMN_GAP_PX;

/// Duration of the one-shot glide compensating that jump.
const GLIDE_DURATION: std::time::Duration = PANEL_ENTER_DURATION;

/// Pure visibility decision feeding the [`PanelAnimation`] machine (the
/// progress-panel twin of `time_slider::drive_visibility`): VM visible →
/// show, hidden → start the exit animation. Returns the close epoch the
/// caller must guard its unmount timer with.
pub fn drive_visibility(anim: &mut PanelAnimation, vm_visible: bool) -> Option<u64> {
    if vm_visible {
        anim.open_requested();
        None
    } else {
        anim.close_requested()
    }
}

/// One-shot vertical glide of the progress panel when the slider pill
/// mounts/unmounts beneath it. The column lays the panel out at its new
/// slot instantly; the glide starts it at the old visual position (a
/// layout-neutral relative `top` offset) and eases to 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glide {
    /// Pill mounted below: the panel is laid out one slot higher, so it
    /// starts one slot down (+offset) and glides up. Keyed by the slider's
    /// open generation.
    Up { slider_generation: u64 },
    /// Pill unmounted: laid out one slot lower, starts one slot up
    /// (−offset) and glides down. Keyed by the slider's close epoch.
    Down { slider_epoch: u64 },
}

impl Glide {
    /// Initial relative `top` offset in px; the animation eases it to 0.
    pub fn start_offset_px(self) -> f32 {
        match self {
            Glide::Up { .. } => SLIDER_SLOT_PX,
            Glide::Down { .. } => -SLIDER_SLOT_PX,
        }
    }

    /// Animation key — a fresh id per slider transition replays the glide
    /// exactly once (gpui restarts `with_animation` when its id changes).
    pub fn element_id(self) -> (&'static str, u64) {
        match self {
            Glide::Up { slider_generation } => ("ingest-progress-glide-up", slider_generation),
            Glide::Down { slider_epoch } => ("ingest-progress-glide-down", slider_epoch),
        }
    }
}

/// Key selection after the slider's visibility machine processed a layer
/// toggle: only an actual mount (Closed → Open) reflows the column — a
/// Closing → Open reopen keeps the pill mounted — and only a visible
/// progress panel has a jump to compensate.
pub fn glide_for_slider_open(
    slider_before: PanelVisibility,
    slider_after: PanelVisibility,
    slider_generation: u64,
    progress_panel_shown: bool,
) -> Option<Glide> {
    (progress_panel_shown
        && slider_before == PanelVisibility::Closed
        && slider_after == PanelVisibility::Open)
        .then_some(Glide::Up { slider_generation })
}

/// Key selection when the slider pill actually unmounts — its exit
/// animation finished and the epoch-guarded timer closed it; that (not the
/// close request) is when the column reflows.
pub fn glide_for_slider_unmount(slider_epoch: u64, progress_panel_shown: bool) -> Option<Glide> {
    progress_panel_shown.then_some(Glide::Down { slider_epoch })
}

pub fn render_progress_panel(root: &RootView, cx: &mut Context<RootView>) -> Option<AnyElement> {
    let visibility = root.progress_anim.visibility();
    if visibility == PanelVisibility::Closed {
        return None;
    }

    // Snapshot the VM up front: `dismiss` keeps the jobs around, so the
    // exit animation still has content, and no AppState borrow is held
    // while building listeners.
    let (label, detail, job_state, fraction, running, has_cancel) = {
        let vm = &root.app_state.read(cx).ingest_progress;
        let job = vm.active_job()?;
        (
            job.label.clone(),
            job.detail.clone(),
            job.state,
            vm.overall_fraction(),
            vm.any_running(),
            vm.on_cancel.is_some(),
        )
    };

    let bar = {
        let mut bar = Progress::new("ingest-progress-bar").xsmall();
        bar = match fraction {
            Some(fraction) => bar.value(fraction * 100.),
            // Total still unknown: indeterminate sliding affordance.
            None => bar.loading(running),
        };
        if job_state == JobState::Failed {
            bar = bar.color(cx.theme().danger);
        }
        bar
    };

    let card = h_flex()
        .occlude()
        .relative()
        .w_full()
        .px_2()
        .py_1p5()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.78))
        .backdrop_blur(px(18.))
        .shadow_lg()
        .child(
            Icon::new(IconName::Download)
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(detail),
                        ),
                )
                .child(bar),
        )
        .child(
            Button::new("ingest-cancel")
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .text_color(cx.theme().muted_foreground)
                .tooltip(if has_cancel { "Cancel" } else { "Dismiss" })
                .on_click(cx.listener(|this: &mut RootView, _, _, cx| {
                    // Cancel-callback slot: filled by the ingest
                    // orchestration; without one the ✕ just dismisses.
                    let cancel = this.app_state.read(cx).ingest_progress.on_cancel.clone();
                    match cancel {
                        Some(cancel) => cancel(),
                        None => this.app_state.update(cx, |state, cx| {
                            state.update_ingest_progress(cx, |vm| vm.dismiss());
                        }),
                    }
                })),
        );

    // Glide wrapper INSIDE the enter/exit animation: two `with_animation`s
    // cannot style the same element, and element state is keyed by the id
    // path — were the (frequently re-keyed) glide the outer wrapper, every
    // re-key would drop the enter animation's state and visibly replay the
    // fade-in. Nested this way a re-key only resets card-internal state
    // (the progress bar's value tween), which is invisible. The wrapper is
    // relative + layout-neutral, so the column never reflows from a glide.
    //
    // Both wrappers here are explicit *flex columns* (`flex().flex_col()`),
    // not default `div()`s: a default div is `display: block` in gpui-ce,
    // which breaks the bottom-left column's width-stretch chain (see
    // `profile_drawer::insets::lift_relative`) — the card must inherit the
    // layers panel's width through every wrapper layer.
    let glided: AnyElement = match root.progress_glide {
        Some(glide) => div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .child(card)
            .with_animation(
                glide.element_id(),
                Animation::new(GLIDE_DURATION).with_easing(ease_out_quint()),
                move |slot, delta| slot.top(px(glide.start_offset_px() * (1. - delta))),
            )
            .into_any_element(),
        None => card.into_any_element(),
    };

    // Enter/exit: the slider pill's one-shot slide/fade recipe, re-keyed by
    // generation/epoch. The slide is a relative `top` offset, so the panel
    // keeps its slot and the cards below never reflow while animating.
    let shell = div().relative().flex().flex_col().w_full().child(glided);
    Some(match visibility {
        PanelVisibility::Closed => return None, // unreachable: handled above
        PanelVisibility::Open => shell
            .with_animation(
                ("ingest-progress-enter", root.progress_anim.open_generation()),
                Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
                |shell, delta| shell.top(px(PANEL_SLIDE_PX * (1. - delta))).opacity(delta),
            )
            .into_any_element(),
        PanelVisibility::Closing => shell
            .with_animation(
                ("ingest-progress-exit", root.progress_anim.close_epoch()),
                Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
                |shell, delta| shell.top(px(PANEL_SLIDE_PX * delta)).opacity(1. - delta),
            )
            .into_any_element(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_visibility_drives_the_panel_machine() {
        let mut anim = PanelAnimation::default();
        assert_eq!(drive_visibility(&mut anim, true), None);
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        let epoch = drive_visibility(&mut anim, false).expect("close epoch for unmount timer");
        assert_eq!(anim.visibility(), PanelVisibility::Closing);
        assert!(anim.animation_done(epoch));
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
        // Hidden stays hidden.
        assert_eq!(drive_visibility(&mut anim, false), None);
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
    }

    #[test]
    fn glide_up_requires_an_actual_mount_and_a_shown_panel() {
        use PanelVisibility::*;
        assert_eq!(
            glide_for_slider_open(Closed, Open, 3, true),
            Some(Glide::Up {
                slider_generation: 3
            })
        );
        // Panel hidden → nothing to compensate.
        assert_eq!(glide_for_slider_open(Closed, Open, 3, false), None);
        // Reopen while still Closing: the pill never unmounted, no reflow.
        assert_eq!(glide_for_slider_open(Closing, Open, 4, true), None);
        // Second layer toggled on while already open: no transition at all.
        assert_eq!(glide_for_slider_open(Open, Open, 3, true), None);
    }

    #[test]
    fn glide_down_requires_a_shown_panel() {
        assert_eq!(
            glide_for_slider_unmount(7, true),
            Some(Glide::Down { slider_epoch: 7 })
        );
        assert_eq!(glide_for_slider_unmount(7, false), None);
    }

    #[test]
    fn glide_offsets_compensate_exactly_one_slider_slot() {
        assert_eq!(
            SLIDER_SLOT_PX,
            SLIDER_PILL_HEIGHT_PX + BOTTOM_LEFT_COLUMN_GAP_PX
        );
        let up = Glide::Up {
            slider_generation: 1,
        };
        let down = Glide::Down { slider_epoch: 1 };
        // Mount below → laid out higher → start lower (positive offset).
        assert_eq!(up.start_offset_px(), SLIDER_SLOT_PX);
        // Unmount below → laid out lower → start higher (negative offset).
        assert_eq!(down.start_offset_px(), -SLIDER_SLOT_PX);
    }

    #[test]
    fn glide_keys_are_unique_per_transition() {
        let up1 = Glide::Up {
            slider_generation: 1,
        }
        .element_id();
        let up2 = Glide::Up {
            slider_generation: 2,
        }
        .element_id();
        let down1 = Glide::Down { slider_epoch: 1 }.element_id();
        assert_ne!(up1, up2, "each mount replays the glide");
        assert_ne!(up1, down1, "up/down with equal counters must not collide");
    }
}

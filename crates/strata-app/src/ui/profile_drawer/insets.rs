//! The shared "planning chrome insets" source (design §3.6 / task item 5):
//! how far surfaces above the profile drawer — the flight panel, the
//! context panel, the bottom-left overlay column — must lift their bottom
//! edge so nothing overlaps the drawer.
//!
//! One derivation ([`super::ProfileDrawer::chrome_insets`]) feeds every
//! consumer; the helpers here turn the snapshot into either an absolute
//! `bottom` inset (panels, whose height must actually shrink) or a
//! layout-neutral relative offset (the overlay column, which only moves).
//! Expand/collapse and mount/unmount carry a [`LiftToggle`], so each
//! consumer wraps a one-shot `with_animation` keyed by its generation —
//! the established parallel-wrapper recipe (`fp-column-shift` pairs with
//! the flight panel's enter the same way) keeps them in step with the
//! drawer card. Drag-resize publishes no toggle: layout follows the
//! pointer immediately.

use gpui::{
    Animation, AnimationExt as _, AnyElement, Div, IntoElement, ParentElement as _, Styled as _,
    ease_out_quint, px, quadratic,
};

use crate::app::panel_animation::{PANEL_ENTER_DURATION, PANEL_EXIT_DURATION};

use super::state::LiftToggle;

/// Resting inset from the bottom edge (the `bottom_3` rhythm every
/// floating card uses).
pub const RESTING_BOTTOM_INSET_PX: f32 = 12.;

/// Inset of the floating drawer card from the window's left/right/bottom
/// edges — the same `_3` rhythm as the other floating panels.
pub const DRAWER_INSET_PX: f32 = 12.;

/// The lift one drawer height produces: the card's height plus its bottom
/// inset — the full bottom band the floating drawer occupies. Zero stays
/// zero (an unmounted drawer occupies nothing, so nothing lifts).
pub fn lift_for_height(height_px: f32) -> f32 {
    if height_px > 0. {
        height_px + DRAWER_INSET_PX
    } else {
        0.
    }
}

/// Snapshot of the drawer's effect on the chrome around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanningChromeInsets {
    /// Current target lift in px: the drawer's height while mounted, 0
    /// otherwise. Surfaces rest [`RESTING_BOTTOM_INSET_PX`] above it.
    pub lift_px: f32,
    /// One-shot animation descriptor when the lift just changed.
    pub toggle: Option<LiftToggle>,
}

impl PlanningChromeInsets {
    /// Explorer mode / drawer unmounted: nothing lifts.
    pub const NONE: Self = Self {
        lift_px: 0.,
        toggle: None,
    };

    /// The absolute `bottom` inset for a floating panel above the drawer.
    pub fn bottom_px(&self) -> f32 {
        RESTING_BOTTOM_INSET_PX + self.lift_px
    }
}

/// The animation a toggle plays: enter timing for mounts/expands/
/// collapses, exit timing for the drawer unmount.
pub(super) fn toggle_animation(toggle: &LiftToggle) -> Animation {
    if toggle.closing() {
        Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic)
    } else {
        Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint())
    }
}

/// Applies the drawer lift as the absolute `bottom` inset of `frame` (a
/// panel's positioned outer frame — its height shrinks as the drawer
/// rises). `key` must be unique per consumer; the toggle generation
/// re-keys the one-shot animation.
pub fn lift_panel_bottom(
    frame: Div,
    insets: &PlanningChromeInsets,
    key: &'static str,
) -> AnyElement {
    match insets.toggle {
        Some(toggle) => frame
            .with_animation(
                (key, toggle.generation),
                toggle_animation(&toggle),
                move |frame, delta| {
                    let lift = toggle.from_px + (toggle.to_px - toggle.from_px) * delta;
                    frame.bottom(px(RESTING_BOTTOM_INSET_PX + lift))
                },
            )
            .into_any_element(),
        None => frame.bottom(px(insets.bottom_px())).into_any_element(),
    }
}

/// The [`lift_relative`] wrapper element: width-transparent by
/// construction. It must be an explicit *flex column* — a default
/// `gpui::div()` is `display: block` in gpui-ce, which breaks the
/// bottom-left column's width-stretch chain (the slider pill's `w_full`
/// then resolves against an indefinite width and collapses to its content
/// size, letting the slider track overflow the pill). As a flex column
/// with no width of its own, the shell stretches to the outer column's
/// width and its default cross-axis stretch passes that width on to the
/// wrapped content, so the layers panel's intrinsic width keeps driving
/// every card in the column.
fn lift_shell() -> Div {
    gpui::div().relative().flex().flex_col()
}

/// Wraps in-flow `content` so it rises by the drawer lift without
/// reflowing its container — the bottom-left overlay column's variant
/// (its outer absolute element keeps the resting `bottom_3` anchor plus
/// the flight-panel left shift; this inner wrapper only offsets visually,
/// exactly like the progress panel's glide).
pub fn lift_relative(content: Div, insets: &PlanningChromeInsets, key: &'static str) -> AnyElement {
    let shell = lift_shell().child(content);
    match insets.toggle {
        Some(toggle) => shell
            .with_animation(
                (key, toggle.generation),
                toggle_animation(&toggle),
                move |shell, delta| {
                    let lift = toggle.from_px + (toggle.to_px - toggle.from_px) * delta;
                    shell.top(px(-lift))
                },
            )
            .into_any_element(),
        None => shell.top(px(-insets.lift_px)).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resting_insets_match_the_bottom_3_rhythm() {
        assert_eq!(PlanningChromeInsets::NONE.bottom_px(), 12.);
        assert_eq!(PlanningChromeInsets::NONE.lift_px, 0.);
        let lifted = PlanningChromeInsets {
            lift_px: 292.,
            toggle: None,
        };
        assert_eq!(
            lifted.bottom_px(),
            304.,
            "occupied bottom band + resting inset"
        );
    }

    /// The bottom-left column's lift wrapper must stay width-transparent:
    /// an explicit flex column (a default `div()` is `display: block` in
    /// gpui-ce and breaks the width-stretch chain — the slider pill then
    /// collapses to its content width and the track overflows it), with
    /// no width/alignment overrides so the outer column's width flows
    /// through to the cards unchanged.
    #[test]
    fn lift_shell_is_width_transparent() {
        use gpui::Styled as _;
        let mut shell = lift_shell();
        let style = shell.style();
        assert_eq!(style.display, Some(gpui::Display::Flex));
        assert_eq!(style.flex_direction, Some(gpui::FlexDirection::Column));
        assert_eq!(style.size.width, None, "no explicit width on the shell");
        assert_eq!(
            style.align_items, None,
            "default cross-axis stretch passes the width to the content"
        );
    }

    /// The floating drawer occupies its height *plus* its own bottom
    /// inset; surfaces above derive their lift from that band. No drawer,
    /// no band — explorer mode keeps the plain `bottom_3` rest.
    #[test]
    fn lift_accounts_for_the_floating_drawer_inset() {
        assert_eq!(lift_for_height(0.), 0., "unmounted drawer lifts nothing");
        assert_eq!(lift_for_height(280.), 280. + DRAWER_INSET_PX);
        assert_eq!(lift_for_height(40.), 40. + DRAWER_INSET_PX);
        // A lifted surface then rests one rhythm above the drawer's top
        // edge: inset + height + resting inset.
        let insets = PlanningChromeInsets {
            lift_px: lift_for_height(280.),
            toggle: None,
        };
        assert_eq!(insets.bottom_px(), 12. + 280. + 12.);
    }
}

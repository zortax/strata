//! Pure state machine for a floating panel's mount/animation lifecycle —
//! shared by the info panel and the weather time slider.
//!
//! gpui's `with_animation` restarts whenever its `ElementId` key changes and
//! offers no completion callback, so the machine deals in counters: the
//! generations key the enter animations (bump = re-trigger) and the close
//! epoch both keys the exit animation and guards the unmount timer the view
//! spawns per close request — a timer that fires after the panel re-opened
//! carries a stale epoch and is ignored.
//!
//! ```text
//! Closed ───open_requested───▶ Open ──close_requested──▶ Closing
//!   ▲                           ▲                           │
//!   │                           └────────open_requested─────┤
//!   └──────────animation_done(current epoch)────────────────┘
//! ```

use std::time::Duration;

/// Duration of the panel's slide/fade entrance.
pub const PANEL_ENTER_DURATION: Duration = Duration::from_millis(180);
/// Duration of the panel's slide/fade exit.
pub const PANEL_EXIT_DURATION: Duration = Duration::from_millis(160);
/// Duration of the content fade when the selection changes while open.
pub const CONTENT_ENTER_DURATION: Duration = Duration::from_millis(160);
/// Unmount delay after a close request: the exit duration plus a small
/// margin so the final fully-transparent frame paints before the element is
/// dropped.
pub const PANEL_UNMOUNT_DELAY: Duration = Duration::from_millis(200);

/// Mount/animation phase of a floating panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelVisibility {
    /// Not rendered at all.
    #[default]
    Closed,
    /// Rendered; entered (or still entering) via the slide-in animation.
    Open,
    /// Still rendered (from a content snapshot) while sliding out.
    Closing,
}

/// The state machine. Owned by `RootView` (one instance per animated
/// panel); all inputs are the three events below, all outputs are
/// [`PanelVisibility`] plus the counters used as animation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelAnimation {
    visibility: PanelVisibility,
    open_generation: u64,
    content_generation: u64,
    close_epoch: u64,
}

impl PanelAnimation {
    pub fn visibility(&self) -> PanelVisibility {
        self.visibility
    }

    /// Keys the panel's enter animation; bumps on every (re-)open so the
    /// slide-in replays.
    pub fn open_generation(&self) -> u64 {
        self.open_generation
    }

    /// Keys the content container; bumps on a repeated open request while
    /// already open (the info panel's re-selection) so a content fade can
    /// replay without moving the panel frame.
    pub fn content_generation(&self) -> u64 {
        self.content_generation
    }

    /// Keys the panel's exit animation and guards unmount timers.
    pub fn close_epoch(&self) -> u64 {
        self.close_epoch
    }

    /// Input: the panel should be visible (a selection was made / a weather
    /// layer turned on).
    pub fn open_requested(&mut self) {
        match self.visibility {
            PanelVisibility::Closed | PanelVisibility::Closing => {
                self.visibility = PanelVisibility::Open;
                self.open_generation += 1;
            }
            PanelVisibility::Open => {
                self.content_generation += 1;
            }
        }
    }

    /// Input: the panel should hide (selection cleared / last weather layer
    /// turned off). Returns the epoch to pass back to
    /// [`Self::animation_done`] once the exit animation has played, or
    /// `None` when there is nothing to close.
    #[must_use]
    pub fn close_requested(&mut self) -> Option<u64> {
        match self.visibility {
            PanelVisibility::Open => {
                self.visibility = PanelVisibility::Closing;
                self.close_epoch += 1;
                Some(self.close_epoch)
            }
            PanelVisibility::Closed | PanelVisibility::Closing => None,
        }
    }

    /// Input: the exit animation for `epoch` finished. Returns `true` when
    /// this closed the panel (the caller unmounts / drops its snapshot);
    /// stale epochs — the panel re-opened or re-closed in the meantime —
    /// return `false` and leave the state untouched.
    #[must_use]
    pub fn animation_done(&mut self, epoch: u64) -> bool {
        if self.visibility == PanelVisibility::Closing && self.close_epoch == epoch {
            self.visibility = PanelVisibility::Closed;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_from_closed_and_bumps_open_generation() {
        let mut anim = PanelAnimation::default();
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
        anim.open_requested();
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        assert_eq!(anim.open_generation(), 1);
        assert_eq!(anim.content_generation(), 0);
    }

    #[test]
    fn reopen_while_open_bumps_content_generation_only() {
        let mut anim = PanelAnimation::default();
        anim.open_requested();
        let open_gen = anim.open_generation();
        anim.open_requested();
        anim.open_requested();
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        assert_eq!(
            anim.open_generation(),
            open_gen,
            "panel frame must stay put"
        );
        assert_eq!(anim.content_generation(), 2);
    }

    #[test]
    fn close_then_animation_done_reaches_closed() {
        let mut anim = PanelAnimation::default();
        anim.open_requested();
        let epoch = anim.close_requested().expect("panel was open");
        assert_eq!(anim.visibility(), PanelVisibility::Closing);
        assert!(anim.animation_done(epoch));
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
    }

    #[test]
    fn close_is_noop_unless_open() {
        let mut anim = PanelAnimation::default();
        assert_eq!(anim.close_requested(), None, "already closed");
        anim.open_requested();
        anim.close_requested().expect("open panel closes");
        assert_eq!(anim.close_requested(), None, "already closing");
        assert_eq!(anim.visibility(), PanelVisibility::Closing);
    }

    #[test]
    fn reopen_during_closing_replays_enter_and_voids_pending_timer() {
        let mut anim = PanelAnimation::default();
        anim.open_requested();
        let first_open = anim.open_generation();
        let epoch = anim.close_requested().expect("open panel closes");
        anim.open_requested(); // re-select while sliding out
        assert_eq!(anim.visibility(), PanelVisibility::Open);
        assert_eq!(
            anim.open_generation(),
            first_open + 1,
            "slide-in re-triggers"
        );
        // The unmount timer from the aborted close must not close the panel.
        assert!(!anim.animation_done(epoch));
        assert_eq!(anim.visibility(), PanelVisibility::Open);
    }

    #[test]
    fn stale_timer_cannot_end_a_newer_close() {
        let mut anim = PanelAnimation::default();
        anim.open_requested();
        let first = anim.close_requested().expect("open panel closes");
        anim.open_requested(); // reopen mid-exit
        let second = anim.close_requested().expect("re-opened panel closes");
        assert_ne!(first, second, "each close gets a fresh exit-animation key");
        assert!(!anim.animation_done(first), "stale epoch is ignored");
        assert_eq!(anim.visibility(), PanelVisibility::Closing);
        assert!(anim.animation_done(second));
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
    }

    #[test]
    fn animation_done_while_closed_is_noop() {
        let mut anim = PanelAnimation::default();
        assert!(!anim.animation_done(0));
        assert_eq!(anim.visibility(), PanelVisibility::Closed);
    }

    #[test]
    fn unmount_delay_covers_the_exit_animation() {
        assert!(PANEL_UNMOUNT_DELAY >= PANEL_EXIT_DURATION);
    }
}

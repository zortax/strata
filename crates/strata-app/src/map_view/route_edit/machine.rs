//! The press→drag→release state machine for route edits (pure).
//!
//! A press on a handle or leg *captures* the gesture (the map never pans
//! under it), but stays a plain click until the cursor travels beyond the
//! click slop — design §3.2: route editing must never hijack the primary
//! selection gesture. Once beyond the slop the gesture is *active* (the
//! caller shows the ghost preview) and stays active even if the cursor
//! returns to the start.

use strata_render::glam::DVec2;

/// What the press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gesture {
    /// Drag an existing waypoint handle (vertex id).
    Handle { id: u64 },
    /// Rubber-band a new waypoint out of main-track leg `index`.
    Leg { index: usize },
}

/// A captured route gesture between mouse-down and mouse-up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RouteDrag {
    gesture: Gesture,
    last_px: DVec2,
    travelled_px: f64,
    active: bool,
}

impl RouteDrag {
    pub(crate) fn new(gesture: Gesture, start_px: DVec2) -> Self {
        Self {
            gesture,
            last_px: start_px,
            travelled_px: 0.0,
            active: false,
        }
    }

    pub(crate) fn gesture(&self) -> Gesture {
        self.gesture
    }

    /// Whether the gesture has left the click slop (the ghost is showing).
    pub(crate) fn active(&self) -> bool {
        self.active
    }

    /// Accumulates cursor travel (same metric as the pan-click detection)
    /// and activates the drag once it reaches `slop_px`. Returns whether
    /// the drag is active after this move.
    pub(crate) fn moved(&mut self, px: DVec2, slop_px: f64) -> bool {
        self.travelled_px += (px - self.last_px).length();
        self.last_px = px;
        if !self.active && self.travelled_px >= slop_px {
            self.active = true;
        }
        self.active
    }

    /// What the release commits.
    pub(crate) fn outcome(&self) -> DragOutcome {
        if !self.active {
            return DragOutcome::Click;
        }
        match self.gesture {
            Gesture::Handle { id } => DragOutcome::MoveHandle { id },
            Gesture::Leg { index } => DragOutcome::InsertIntoLeg { index },
        }
    }
}

/// Release outcome of a [`RouteDrag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragOutcome {
    /// Never left the click slop: plain selection click, exactly as today.
    Click,
    /// Commit the dragged handle's new position.
    MoveHandle { id: u64 },
    /// Commit the rubber-banded waypoint into leg `index`.
    InsertIntoLeg { index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOP: f64 = 4.0;

    #[test]
    fn press_release_within_slop_is_a_click() {
        let mut drag = RouteDrag::new(Gesture::Handle { id: 3 }, DVec2::new(100.0, 100.0));
        assert!(!drag.moved(DVec2::new(101.0, 101.0), SLOP));
        assert!(!drag.active());
        assert_eq!(drag.outcome(), DragOutcome::Click);
    }

    #[test]
    fn travel_beyond_slop_activates_and_commits_a_move() {
        let mut drag = RouteDrag::new(Gesture::Handle { id: 3 }, DVec2::new(100.0, 100.0));
        assert!(!drag.moved(DVec2::new(102.0, 100.0), SLOP));
        assert!(drag.moved(DVec2::new(105.0, 100.0), SLOP));
        assert_eq!(drag.outcome(), DragOutcome::MoveHandle { id: 3 });
    }

    /// Slop is accumulated travel, not displacement: a zigzag activates
    /// even when it ends near the start, and an active drag released over
    /// its origin still commits (matching the pan-click metric).
    #[test]
    fn accumulated_travel_counts_and_active_drags_stay_active() {
        let mut drag = RouteDrag::new(Gesture::Leg { index: 1 }, DVec2::new(100.0, 100.0));
        drag.moved(DVec2::new(103.0, 100.0), SLOP);
        drag.moved(DVec2::new(100.0, 100.0), SLOP);
        assert!(drag.active(), "6 px of travel beats the 4 px slop");
        assert_eq!(drag.outcome(), DragOutcome::InsertIntoLeg { index: 1 });
    }

    #[test]
    fn leg_gestures_insert_handle_gestures_move() {
        let mut handle = RouteDrag::new(Gesture::Handle { id: 7 }, DVec2::ZERO);
        let mut leg = RouteDrag::new(Gesture::Leg { index: 0 }, DVec2::ZERO);
        handle.moved(DVec2::new(10.0, 0.0), SLOP);
        leg.moved(DVec2::new(10.0, 0.0), SLOP);
        assert_eq!(handle.outcome(), DragOutcome::MoveHandle { id: 7 });
        assert_eq!(leg.outcome(), DragOutcome::InsertIntoLeg { index: 0 });
    }
}

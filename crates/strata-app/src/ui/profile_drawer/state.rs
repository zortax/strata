//! Pure state machine for the profile drawer's chrome (design §3.3): the
//! collapsed-strip ↔ expanded modes, the drag-resizable expanded height,
//! and the [`LiftToggle`] records that key the one-shot height/lift
//! animations (the drawer card *and* every surface that rests on top of
//! it — see [`super::insets`]).
//!
//! All inputs are plain numbers (pointer y, window height in logical px),
//! all outputs are heights/toggles — no gpui, fully unit-testable. The
//! [`ProfileDrawer`] entity owns one instance and forwards events.
//!
//! [`ProfileDrawer`]: super::ProfileDrawer

/// Height of the collapsed summary strip.
pub const COLLAPSED_HEIGHT_PX: f32 = 40.;
/// Default expanded height (design §3.3: ~280 px), also the config
/// fallback.
pub const DEFAULT_EXPANDED_HEIGHT_PX: f32 = 280.;
/// Drag-resize lower bound for the expanded drawer.
pub const MIN_EXPANDED_HEIGHT_PX: f32 = 160.;
/// Drag-resize upper bound as a fraction of the window height.
pub const MAX_WINDOW_FRACTION: f32 = 0.6;

/// Collapsed strip or full drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerMode {
    Collapsed,
    /// The default state while planning (design §3.3).
    #[default]
    Expanded,
}

/// What caused a lift transition. Mount/unmount slide the full-height
/// card in from the bottom edge (the card's height never animates for
/// them); expand/collapse animates the card height in place. All three
/// move the lifted surfaces above the drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftKind {
    /// Expand/collapse while mounted.
    Mode,
    /// The drawer (re-)mounted (planning mode on) — enter timing.
    Mount,
    /// The drawer started its exit animation — exit timing.
    Unmount,
}

/// One lift transition: the drawer's visual height moved from `from_px`
/// to `to_px`. Keys the one-shot `with_animation` wrappers on the drawer
/// card and on every lifted surface — a fresh generation per transition
/// replays the animation exactly once; drag-resize records no toggle
/// (immediate layout).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiftToggle {
    pub from_px: f32,
    pub to_px: f32,
    /// Animation key; unique per transition.
    pub generation: u64,
    pub kind: LiftKind,
}

impl LiftToggle {
    /// Consumers match the panel *exit* timing for the unmount.
    pub fn closing(&self) -> bool {
        self.kind == LiftKind::Unmount
    }
}

/// An active drag on the top-edge grab handle.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DragResize {
    /// Expanded height when the drag started.
    start_height_px: f32,
    /// Pointer y (window coordinates) when the drag started.
    start_y_px: f32,
}

/// The drawer chrome state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawerState {
    mode: DrawerMode,
    /// Persisted expanded height (unclamped store value; reads clamp
    /// against the current window).
    expanded_height_px: f32,
    toggle: Option<LiftToggle>,
    generation: u64,
    drag: Option<DragResize>,
}

impl DrawerState {
    /// A drawer at `expanded_height_px` (from config), expanded, at rest.
    pub fn new(expanded_height_px: f32) -> Self {
        Self {
            mode: DrawerMode::default(),
            expanded_height_px,
            toggle: None,
            generation: 0,
            drag: None,
        }
    }

    // Part of the state-machine surface (exercised by the unit tests);
    // the entity reads `is_expanded` today.
    #[allow(dead_code)]
    pub fn mode(&self) -> DrawerMode {
        self.mode
    }

    pub fn is_expanded(&self) -> bool {
        self.mode == DrawerMode::Expanded
    }

    pub fn is_resizing(&self) -> bool {
        self.drag.is_some()
    }

    /// The most recent height/lift transition (idles at its end state
    /// once the animation played; replaced by the next transition,
    /// cleared by a drag).
    pub fn lift_toggle(&self) -> Option<LiftToggle> {
        self.toggle
    }

    /// Upper resize bound for a window of `window_height_px`.
    pub fn max_expanded_height_px(window_height_px: f32) -> f32 {
        (window_height_px * MAX_WINDOW_FRACTION).max(MIN_EXPANDED_HEIGHT_PX)
    }

    /// The expanded height clamped into the window's valid range.
    pub fn expanded_height_px(&self, window_height_px: f32) -> f32 {
        self.expanded_height_px.clamp(
            MIN_EXPANDED_HEIGHT_PX,
            Self::max_expanded_height_px(window_height_px),
        )
    }

    /// Current target height of the drawer card for the current mode.
    pub fn height_px(&self, window_height_px: f32) -> f32 {
        match self.mode {
            DrawerMode::Collapsed => COLLAPSED_HEIGHT_PX,
            DrawerMode::Expanded => self.expanded_height_px(window_height_px),
        }
    }

    // --- mode transitions -------------------------------------------------

    /// Expand/collapse (the header button / strip click). Records the
    /// height toggle so the change animates; a no-op for the current mode.
    pub fn set_mode(&mut self, mode: DrawerMode, window_height_px: f32) {
        if self.mode == mode {
            return;
        }
        let from = self.height_px(window_height_px);
        self.mode = mode;
        self.drag = None;
        self.record_toggle(from, self.height_px(window_height_px), LiftKind::Mode);
    }

    pub fn toggle_mode(&mut self, window_height_px: f32) {
        let next = match self.mode {
            DrawerMode::Collapsed => DrawerMode::Expanded,
            DrawerMode::Expanded => DrawerMode::Collapsed,
        };
        self.set_mode(next, window_height_px);
    }

    // --- mount lifecycle ----------------------------------------------------

    /// The drawer (re-)mounted: the lift rises from nothing to the
    /// current height, in step with the card's enter slide.
    pub fn mounted(&mut self, window_height_px: f32) {
        self.drag = None;
        self.record_toggle(0., self.height_px(window_height_px), LiftKind::Mount);
    }

    /// The drawer started its exit animation: the lift returns to zero
    /// with the exit timing.
    pub fn unmounting(&mut self, window_height_px: f32) {
        self.drag = None;
        self.record_toggle(self.height_px(window_height_px), 0., LiftKind::Unmount);
    }

    // --- drag resize ----------------------------------------------------------

    /// Pointer down on the grab handle. Only the expanded drawer resizes;
    /// returns whether a drag started. Starting a drag drops any pending
    /// toggle — resize feedback is immediate, never animated.
    pub fn begin_resize(&mut self, pointer_y_px: f32, window_height_px: f32) -> bool {
        if self.mode != DrawerMode::Expanded || self.drag.is_some() {
            return false;
        }
        self.toggle = None;
        self.drag = Some(DragResize {
            start_height_px: self.expanded_height_px(window_height_px),
            start_y_px: pointer_y_px,
        });
        true
    }

    /// Pointer moved during a drag: dragging up grows the drawer. Returns
    /// whether the height changed (the caller re-renders only then).
    pub fn resize_to(&mut self, pointer_y_px: f32, window_height_px: f32) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let target = (drag.start_height_px + (drag.start_y_px - pointer_y_px)).clamp(
            MIN_EXPANDED_HEIGHT_PX,
            Self::max_expanded_height_px(window_height_px),
        );
        if self.expanded_height_px == target {
            return false;
        }
        self.expanded_height_px = target;
        true
    }

    /// Pointer released (or the drag was otherwise abandoned). Returns
    /// the final expanded height to persist when a drag was active.
    pub fn end_resize(&mut self) -> Option<f32> {
        self.drag.take().map(|_| self.expanded_height_px)
    }

    fn record_toggle(&mut self, from_px: f32, to_px: f32, kind: LiftKind) {
        self.generation += 1;
        self.toggle = Some(LiftToggle {
            from_px,
            to_px,
            generation: self.generation,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: f32 = 1000.;

    #[test]
    fn starts_expanded_at_the_configured_height() {
        let state = DrawerState::new(280.);
        assert_eq!(state.mode(), DrawerMode::Expanded);
        assert!(state.is_expanded());
        assert_eq!(state.height_px(WINDOW), 280.);
        assert_eq!(state.lift_toggle(), None, "no transition yet");
        assert!(!state.is_resizing());
    }

    #[test]
    fn heights_clamp_into_the_window_bounds() {
        // Stored height above 60 % of the window clamps down…
        let state = DrawerState::new(900.);
        assert_eq!(state.height_px(WINDOW), 600.);
        // …below the minimum clamps up…
        let state = DrawerState::new(10.);
        assert_eq!(state.height_px(WINDOW), MIN_EXPANDED_HEIGHT_PX);
        // …and a tiny window still honours the minimum (min beats max).
        let state = DrawerState::new(280.);
        assert_eq!(state.height_px(100.), MIN_EXPANDED_HEIGHT_PX);
        assert_eq!(
            DrawerState::max_expanded_height_px(100.),
            MIN_EXPANDED_HEIGHT_PX
        );
    }

    #[test]
    fn collapse_and_expand_record_animated_toggles() {
        let mut state = DrawerState::new(280.);
        state.set_mode(DrawerMode::Collapsed, WINDOW);
        assert_eq!(state.height_px(WINDOW), COLLAPSED_HEIGHT_PX);
        let toggle = state.lift_toggle().expect("collapse animates");
        assert_eq!((toggle.from_px, toggle.to_px), (280., COLLAPSED_HEIGHT_PX));
        assert_eq!(toggle.kind, LiftKind::Mode);
        assert!(!toggle.closing());

        state.toggle_mode(WINDOW);
        assert!(state.is_expanded());
        let expand = state.lift_toggle().expect("expand animates");
        assert_eq!((expand.from_px, expand.to_px), (COLLAPSED_HEIGHT_PX, 280.));
        assert_ne!(
            expand.generation, toggle.generation,
            "each transition gets a fresh animation key"
        );

        // Re-setting the current mode is a no-op (no replayed animation).
        state.set_mode(DrawerMode::Expanded, WINDOW);
        assert_eq!(state.lift_toggle(), Some(expand));
    }

    #[test]
    fn mount_and_unmount_toggle_the_full_lift() {
        let mut state = DrawerState::new(280.);
        state.mounted(WINDOW);
        let mount = state.lift_toggle().expect("mount lifts");
        assert_eq!((mount.from_px, mount.to_px), (0., 280.));
        assert_eq!(mount.kind, LiftKind::Mount);
        assert!(!mount.closing());

        state.unmounting(WINDOW);
        let unmount = state.lift_toggle().expect("unmount lowers");
        assert_eq!((unmount.from_px, unmount.to_px), (280., 0.));
        assert_eq!(unmount.kind, LiftKind::Unmount);
        assert!(unmount.closing(), "unmount uses the exit timing");
        assert_ne!(mount.generation, unmount.generation);

        // A collapsed drawer mounts/unmounts at strip height.
        let mut state = DrawerState::new(280.);
        state.set_mode(DrawerMode::Collapsed, WINDOW);
        state.mounted(WINDOW);
        let mount = state.lift_toggle().expect("mount lifts");
        assert_eq!((mount.from_px, mount.to_px), (0., COLLAPSED_HEIGHT_PX));
    }

    #[test]
    fn drag_resize_tracks_the_pointer_and_reports_the_final_height() {
        let mut state = DrawerState::new(280.);
        assert!(state.begin_resize(700., WINDOW));
        assert!(state.is_resizing());
        assert_eq!(state.lift_toggle(), None, "drags never animate");

        // Up 40 px → 40 px taller; the same position again is a no-op.
        assert!(state.resize_to(660., WINDOW));
        assert_eq!(state.height_px(WINDOW), 320.);
        assert!(!state.resize_to(660., WINDOW));

        // Clamped at both ends.
        assert!(state.resize_to(0., WINDOW));
        assert_eq!(state.height_px(WINDOW), 600., "60 % of the window");
        assert!(state.resize_to(2000., WINDOW));
        assert_eq!(state.height_px(WINDOW), MIN_EXPANDED_HEIGHT_PX);

        assert_eq!(state.end_resize(), Some(MIN_EXPANDED_HEIGHT_PX));
        assert!(!state.is_resizing());
        assert_eq!(state.end_resize(), None, "no drag active anymore");
    }

    #[test]
    fn drags_only_start_from_the_expanded_drawer() {
        let mut state = DrawerState::new(280.);
        state.set_mode(DrawerMode::Collapsed, WINDOW);
        assert!(!state.begin_resize(700., WINDOW));
        assert!(!state.resize_to(600., WINDOW));
        assert_eq!(state.end_resize(), None);

        state.set_mode(DrawerMode::Expanded, WINDOW);
        assert!(state.begin_resize(700., WINDOW));
        assert!(!state.begin_resize(500., WINDOW), "one drag at a time");
    }

    #[test]
    fn drag_clears_a_pending_toggle_and_mode_changes_cancel_drags() {
        let mut state = DrawerState::new(280.);
        state.toggle_mode(WINDOW); // collapse…
        state.toggle_mode(WINDOW); // …and expand: a toggle is pending
        assert!(state.lift_toggle().is_some());
        assert!(state.begin_resize(700., WINDOW));
        assert_eq!(state.lift_toggle(), None, "resize feedback is immediate");

        // Collapsing mid-drag abandons the drag (it can no longer commit).
        assert!(state.resize_to(650., WINDOW));
        state.set_mode(DrawerMode::Collapsed, WINDOW);
        assert!(!state.is_resizing());
        // The height the drag reached before the collapse is what the
        // collapse toggle animates from.
        assert_eq!(state.lift_toggle().map(|t| t.from_px), Some(330.));
    }

    #[test]
    fn toggle_targets_match_the_current_height() {
        // Invariant the insets derivation relies on: whenever a toggle is
        // live, its `to_px` equals the state's current target height (or 0
        // for the unmount).
        let mut state = DrawerState::new(280.);
        state.mounted(WINDOW);
        assert_eq!(
            state.lift_toggle().map(|t| t.to_px),
            Some(state.height_px(WINDOW))
        );
        state.toggle_mode(WINDOW);
        assert_eq!(
            state.lift_toggle().map(|t| t.to_px),
            Some(state.height_px(WINDOW))
        );
        state.unmounting(WINDOW);
        assert_eq!(state.lift_toggle().map(|t| t.to_px), Some(0.));
    }
}

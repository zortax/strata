//! Pure state + math for the gridded-weather time slider.
//!
//! The slider spans a fixed window of −2 h … +24 h around an *anchor*
//! ("now", re-anchored on each fetch cycle). All slider positions are
//! expressed as minute offsets from the anchor so the UI's `SliderState`
//! range never changes; the absolute selected instant lives here and is
//! what the renderer and the fetch scheduler consume.

use chrono::{DateTime, Duration, Utc};

/// Observed past reachable on the slider (precip radar history).
pub const PAST_HOURS: i64 = 2;
/// Forecast future reachable on the slider. ICON-D2 publishes out to
/// +48 h, but the slider deliberately stops at +24 h — model skill beyond
/// a day isn't trusted for VFR planning, and the small window lets the
/// scheduler prefetch every frame in it.
pub const FUTURE_HOURS: i64 = 24;
/// Slider snap granularity in minutes (the radar's 5-minute lattice; ICON
/// is hourly but the renderer blends between frames, so finer scrubbing
/// stays smooth).
pub const STEP_MINUTES: i64 = 5;

/// Slider minimum, in minutes relative to the anchor.
pub const MIN_OFFSET_MINUTES: f32 = -(PAST_HOURS * 60) as f32;
/// Slider maximum, in minutes relative to the anchor.
pub const MAX_OFFSET_MINUTES: f32 = (FUTURE_HOURS * 60) as f32;

/// Fraction (0..=1) along the slider track for a minute offset — the pure
/// time↔position mapping the track tick marks use.
pub fn fraction_for_offset(minutes: f32) -> f32 {
    ((minutes - MIN_OFFSET_MINUTES) / (MAX_OFFSET_MINUTES - MIN_OFFSET_MINUTES)).clamp(0.0, 1.0)
}

/// Track position of the anchor ("now") tick.
pub fn now_fraction() -> f32 {
    fraction_for_offset(0.0)
}

/// Floor onto the slider's 5-minute lattice. Anchors snap so every slider
/// position (a multiple of [`STEP_MINUTES`] from the anchor) lands on a
/// round wall-clock minute — labels read "Thu 17:05Z", not "Thu 17:09Z" —
/// and coincides with the radar's 5-minute frame times.
fn floor_to_step(t: DateTime<Utc>) -> DateTime<Utc> {
    let step = STEP_MINUTES * 60;
    DateTime::from_timestamp(t.timestamp().div_euclid(step) * step, 0).unwrap_or(t)
}

/// Anchor + selected instant of the weather time slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherTime {
    /// "Now" as of the last re-anchor; the slider window is relative to it.
    anchor: DateTime<Utc>,
    /// The instant the user selected, always within the window.
    selected: DateTime<Utc>,
}

impl WeatherTime {
    pub fn new(now: DateTime<Utc>) -> Self {
        let now = floor_to_step(now);
        Self {
            anchor: now,
            selected: now,
        }
    }

    pub fn anchor(&self) -> DateTime<Utc> {
        self.anchor
    }

    pub fn selected(&self) -> DateTime<Utc> {
        self.selected
    }

    /// The reachable window `(anchor − 2 h, anchor + 24 h)`.
    pub fn range(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        (
            self.anchor - Duration::hours(PAST_HOURS),
            self.anchor + Duration::hours(FUTURE_HOURS),
        )
    }

    /// Selected instant as a minute offset from the anchor (the slider
    /// value).
    pub fn offset_minutes(&self) -> f32 {
        (self.selected - self.anchor).num_seconds() as f32 / 60.0
    }

    /// Set the selection from a slider value (minutes from the anchor),
    /// clamped into the window. Returns whether the selection changed.
    pub fn set_offset_minutes(&mut self, minutes: f32) -> bool {
        let minutes = minutes.clamp(MIN_OFFSET_MINUTES, MAX_OFFSET_MINUTES);
        let selected = self.anchor + Duration::seconds((f64::from(minutes) * 60.0).round() as i64);
        let changed = selected != self.selected;
        self.selected = selected;
        changed
    }

    /// Step the selection by `delta`, clamped into the window. Returns
    /// whether the selection changed.
    pub fn step(&mut self, delta: Duration) -> bool {
        let (min, max) = self.range();
        let selected = (self.selected + delta).clamp(min, max);
        let changed = selected != self.selected;
        self.selected = selected;
        changed
    }

    /// Re-anchor the window at a fresh "now" (fetch-cycle cadence; snapped
    /// onto the 5-minute lattice). Drift below one slider step (5 min)
    /// keeps the anchor as-is so follow-up cycles can't re-anchor in a
    /// loop. A selection sitting *exactly* on the old anchor (an untouched
    /// slider, or one dragged back onto "now") follows the new now; any
    /// other selection is the user's explicit absolute instant and is
    /// never moved — except clamping when the shifting window leaves it
    /// behind entirely. Returns whether anchor or selection changed.
    pub fn re_anchor(&mut self, now: DateTime<Utc>) -> bool {
        let now = floor_to_step(now);
        if (now - self.anchor).abs() < Duration::minutes(STEP_MINUTES) {
            return false;
        }
        let was_at_now = self.selected == self.anchor;
        self.anchor = now;
        if was_at_now {
            self.selected = now;
        } else {
            let (min, max) = self.range();
            self.selected = self.selected.clamp(min, max);
        }
        true
    }

    /// Whether the selection sits at "now" (within half a slider step).
    pub fn is_now(&self) -> bool {
        (self.selected - self.anchor).abs().num_seconds() < STEP_MINUTES * 60 / 2
    }

    /// Slider label: `"now"` at the anchor, otherwise weekday + UTC time,
    /// e.g. `"Wed 15:00Z"`.
    pub fn label(&self) -> String {
        if self.is_now() {
            "now".to_string()
        } else {
            self.selected.format("%a %H:%MZ").to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().expect("test timestamp")
    }

    #[test]
    fn offsets_map_onto_track_fractions() {
        assert_eq!(fraction_for_offset(MIN_OFFSET_MINUTES), 0.0);
        assert_eq!(fraction_for_offset(MAX_OFFSET_MINUTES), 1.0);
        // -2 h of 26 h total → now sits at 2/26 ≈ 7.7 % of the track;
        // +11 h at 50 %.
        assert!((now_fraction() - 2.0 / 26.0).abs() < 1e-6);
        assert!((fraction_for_offset(11.0 * 60.0) - 0.5).abs() < 1e-6);
        // Out-of-range values clamp instead of leaving the track.
        assert_eq!(fraction_for_offset(-9999.0), 0.0);
        assert_eq!(fraction_for_offset(9999.0), 1.0);
    }

    /// The other direction of the time↔position mapping: a slider value
    /// (minutes) round-trips through the absolute selected instant.
    #[test]
    fn slider_offsets_round_trip_through_absolute_times() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        for minutes in [-120.0_f32, -5.0, 0.0, 60.0, 1234.0, 1440.0] {
            wt.set_offset_minutes(minutes);
            assert!(
                (wt.offset_minutes() - minutes).abs() < 1e-3,
                "{minutes} -> {}",
                wt.offset_minutes()
            );
        }
    }

    #[test]
    fn defaults_to_now_and_clamps_offsets() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert!(wt.is_now());
        assert_eq!(wt.offset_minutes(), 0.0);

        assert!(wt.set_offset_minutes(90.0));
        assert_eq!(wt.selected(), t("2026-06-10T13:30:00Z"));
        assert!(!wt.is_now());

        // Below/above the window clamps to the edges.
        wt.set_offset_minutes(-9_999.0);
        assert_eq!(wt.selected(), t("2026-06-10T10:00:00Z"));
        wt.set_offset_minutes(9_999.0);
        assert_eq!(wt.selected(), t("2026-06-11T12:00:00Z"));

        // Setting the same value reports no change.
        assert!(!wt.set_offset_minutes(MAX_OFFSET_MINUTES));
    }

    #[test]
    fn step_clamps_at_the_window_edges() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert!(wt.step(Duration::hours(-1)));
        assert_eq!(wt.offset_minutes(), -60.0);
        assert!(wt.step(Duration::hours(-1)));
        assert!(!wt.step(Duration::hours(-1)), "already at -2 h");
        assert_eq!(wt.offset_minutes(), MIN_OFFSET_MINUTES);
    }

    #[test]
    fn re_anchor_follows_now_when_untouched() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert!(wt.re_anchor(t("2026-06-10T12:10:00Z")));
        assert_eq!(wt.anchor(), t("2026-06-10T12:10:00Z"));
        assert!(wt.is_now(), "an untouched slider keeps tracking now");
        assert_eq!(wt.selected(), t("2026-06-10T12:10:00Z"));
    }

    #[test]
    fn re_anchor_keeps_an_explicit_selection() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        wt.set_offset_minutes(360.0); // 18:00Z
        assert!(wt.re_anchor(t("2026-06-10T12:10:00Z")));
        assert_eq!(wt.selected(), t("2026-06-10T18:00:00Z"));
        // The offset shifted with the anchor.
        assert_eq!(wt.offset_minutes(), 350.0);
    }

    /// Even a selection one slider step away from "now" is the user's
    /// explicit absolute instant — a re-anchor must never move it.
    #[test]
    fn re_anchor_never_moves_an_explicit_selection_near_now() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        wt.set_offset_minutes(5.0); // 12:05Z, one step ahead of now
        assert!(wt.re_anchor(t("2026-06-10T12:10:00Z")));
        assert_eq!(wt.selected(), t("2026-06-10T12:05:00Z"));
        assert_eq!(wt.offset_minutes(), -5.0);
    }

    /// The drift threshold is one slider step (5 min): below it the anchor
    /// is kept, at it the window re-anchors.
    #[test]
    fn re_anchor_drift_threshold_is_one_slider_step() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        // 12:04:59 floors onto the lattice at 12:00 — zero drift, no-op.
        assert!(!wt.re_anchor(t("2026-06-10T12:04:59Z")));
        assert_eq!(wt.anchor(), t("2026-06-10T12:00:00Z"));
        // 12:05:00 is a full step of drift — re-anchors.
        assert!(wt.re_anchor(t("2026-06-10T12:05:00Z")));
        assert_eq!(wt.anchor(), t("2026-06-10T12:05:00Z"));
    }

    #[test]
    fn re_anchor_clamps_a_selection_that_fell_off_the_window() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        wt.set_offset_minutes(MIN_OFFSET_MINUTES); // 10:00Z
        assert!(wt.re_anchor(t("2026-06-10T13:00:00Z")));
        assert_eq!(wt.selected(), t("2026-06-10T11:00:00Z"), "clamped to -2 h");
    }

    #[test]
    fn re_anchor_is_a_noop_for_fresh_anchors() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert!(
            !wt.re_anchor(t("2026-06-10T12:03:00Z")),
            "fresh anchor kept — guards against re-anchor/event loops"
        );
        assert_eq!(wt.anchor(), t("2026-06-10T12:00:00Z"));
    }

    #[test]
    fn anchors_snap_onto_the_five_minute_lattice() {
        // 12:07:42 floors to 12:05 — slider positions then land on round
        // wall-clock minutes (and radar frame times).
        let wt = WeatherTime::new(t("2026-06-10T12:07:42Z"));
        assert_eq!(wt.anchor(), t("2026-06-10T12:05:00Z"));
        assert_eq!(wt.selected(), wt.anchor());

        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert!(wt.re_anchor(t("2026-06-10T12:11:59Z")));
        assert_eq!(wt.anchor(), t("2026-06-10T12:10:00Z"));
        wt.set_offset_minutes(60.0);
        assert_eq!(wt.label(), "Wed 13:10Z");
    }

    #[test]
    fn labels_now_and_utc_times() {
        let mut wt = WeatherTime::new(t("2026-06-10T12:00:00Z"));
        assert_eq!(wt.label(), "now");
        wt.set_offset_minutes(180.0);
        assert_eq!(wt.label(), "Wed 15:00Z");
        wt.set_offset_minutes(-60.0);
        assert_eq!(wt.label(), "Wed 11:00Z");
    }
}

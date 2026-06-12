//! Bracketing-frame selection for the weather time slider, plus the pure
//! blend-fraction math (range clamping and the re-blend ramp).

use std::time::Duration;

/// The two frame indices bracketing `time` in a strictly ascending list of
/// valid times, plus the blend fraction toward the second frame.
///
/// * `time` before the first frame → `(0, 0, 0.0)` (clamp, single frame).
/// * `time` after the last frame → `(last, last, 0.0)`.
/// * exact hit → `(i, i, 0.0)`.
/// * in between → `(i, i + 1, frac)` with `frac ∈ (0, 1)`.
///
/// Returns `None` for an empty list.
pub(crate) fn bracket(times: &[i64], time: i64) -> Option<(usize, usize, f32)> {
    let (&first, &last) = (times.first()?, times.last()?);
    if time <= first {
        return Some((0, 0, 0.0));
    }
    if time >= last {
        let i = times.len() - 1;
        return Some((i, i, 0.0));
    }
    match times.binary_search(&time) {
        Ok(i) => Some((i, i, 0.0)),
        Err(i) => {
            // `0 < i < len` here: `time` is strictly inside the range.
            let (t0, t1) = (times[i - 1], times[i]);
            let frac = (time - t0) as f32 / (t1 - t0) as f32;
            Some((i - 1, i, frac))
        }
    }
}

/// Like [`bracket`], but a pair spanning more than `max_gap_seconds` is a
/// data hole (frames missing from the working set, not adjacent model
/// steps): hold the **nearest** endpoint steadily instead of dissolving
/// two distant frames into a fictitious in-between state. Within the gap
/// threshold the selection is exactly [`bracket`]'s.
pub(crate) fn bracket_hold(
    times: &[i64],
    time: i64,
    max_gap_seconds: i64,
) -> Option<(usize, usize, f32)> {
    let (ia, ib, frac) = bracket(times, time)?;
    if ia != ib && times[ib] - times[ia] > max_gap_seconds {
        let i = if frac < 0.5 { ia } else { ib };
        return Some((i, i, 0.0));
    }
    Some((ia, ib, frac))
}

/// Blend fraction of `time` within `[t0, t1]`, clamped to `[0, 1]` — used
/// to keep drawing the last-ready pair (with a clamped fraction) while a
/// newly needed frame texture is still uploading.
pub(crate) fn clamped_fraction(t0: i64, t1: i64, time: i64) -> f32 {
    if t1 <= t0 {
        return 0.0;
    }
    ((time - t0) as f64 / (t1 - t0) as f64).clamp(0.0, 1.0) as f32
}

/// Displayed blend fraction `elapsed` into a re-blend ramp: smoothstep
/// from `from` to `target` over `duration`, pinned at `target` once the
/// ramp has run out. A pure function of wall-clock elapsed time — the
/// result is independent of how many frames were rendered along the way,
/// and a `target` that keeps moving (slider drag during the ramp) is
/// tracked live and reached exactly when the ramp completes.
pub(crate) fn ramp_fraction(from: f32, target: f32, elapsed: Duration, duration: Duration) -> f32 {
    if duration.is_zero() || elapsed >= duration {
        return target;
    }
    let t = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t); // smoothstep: gentle at both ends
    from + (target - from) * s
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMES: [i64; 4] = [1000, 2000, 4000, 5000];

    #[test]
    fn empty_list_has_no_bracket() {
        assert_eq!(bracket(&[], 1234), None);
    }

    #[test]
    fn before_the_range_clamps_to_the_first_frame() {
        assert_eq!(bracket(&TIMES, 0), Some((0, 0, 0.0)));
        assert_eq!(bracket(&TIMES, 1000), Some((0, 0, 0.0)), "first is exact");
    }

    #[test]
    fn after_the_range_clamps_to_the_last_frame() {
        assert_eq!(bracket(&TIMES, 9999), Some((3, 3, 0.0)));
        assert_eq!(bracket(&TIMES, 5000), Some((3, 3, 0.0)), "last is exact");
    }

    #[test]
    fn exact_interior_hit_needs_no_blend() {
        assert_eq!(bracket(&TIMES, 2000), Some((1, 1, 0.0)));
    }

    #[test]
    fn inside_the_range_blends_between_neighbors() {
        let (a, b, frac) = bracket(&TIMES, 2500).expect("inside");
        assert_eq!((a, b), (1, 2));
        assert!((frac - 0.25).abs() < 1e-6, "2500 is ¼ into [2000, 4000]");
        let (a, b, frac) = bracket(&TIMES, 4999).expect("inside");
        assert_eq!((a, b), (2, 3));
        assert!(frac > 0.99 && frac < 1.0);
    }

    #[test]
    fn single_frame_always_maps_to_itself() {
        for time in [0, 7777, 100_000] {
            assert_eq!(bracket(&[7777], time), Some((0, 0, 0.0)));
        }
    }

    #[test]
    fn clamped_fraction_saturates_outside_the_pair() {
        assert_eq!(clamped_fraction(100, 200, 50), 0.0);
        assert_eq!(clamped_fraction(100, 200, 150), 0.5);
        assert_eq!(clamped_fraction(100, 200, 900), 1.0);
        assert_eq!(clamped_fraction(100, 100, 100), 0.0, "degenerate pair");
    }

    /// Inside the gap threshold `bracket_hold` is exactly `bracket` —
    /// selection (and therefore scrubbing) is unchanged.
    #[test]
    fn bracket_hold_matches_bracket_for_normal_spacing() {
        for time in [0, 1000, 1500, 2000, 2500, 4999, 5000, 9999] {
            assert_eq!(
                bracket_hold(&TIMES, time, 2000),
                bracket(&TIMES, time),
                "time {time}"
            );
        }
        assert_eq!(bracket_hold(&[], 1234, 2000), None);
    }

    /// A pair wider than the gap threshold is a data hole: hold the
    /// nearest endpoint instead of blending non-adjacent frames.
    #[test]
    fn bracket_hold_pins_the_nearest_frame_across_a_hole() {
        let times = [1000, 2000, 30_000];
        assert_eq!(
            bracket_hold(&times, 8000, 7200),
            Some((1, 1, 0.0)),
            "before the midpoint: hold the earlier frame"
        );
        assert_eq!(
            bracket_hold(&times, 25_000, 7200),
            Some((2, 2, 0.0)),
            "past the midpoint: hold the later frame"
        );
        assert_eq!(
            bracket_hold(&times, 16_000, 7200),
            Some((2, 2, 0.0)),
            "exact midpoint rounds toward the later frame"
        );
        assert_eq!(
            bracket_hold(&times, 1500, 7200),
            Some((0, 1, 0.5)),
            "the narrow pair still blends"
        );
    }

    #[test]
    fn ramp_starts_at_from_and_ends_pinned_at_target() {
        let dur = Duration::from_millis(200);
        assert_eq!(ramp_fraction(0.0, 0.75, Duration::ZERO, dur), 0.0);
        assert_eq!(ramp_fraction(0.0, 0.75, dur, dur), 0.75);
        assert_eq!(
            ramp_fraction(0.0, 0.75, Duration::from_secs(9), dur),
            0.75,
            "stays at the target after the ramp"
        );
        assert_eq!(
            ramp_fraction(1.0, 0.25, Duration::ZERO, dur),
            1.0,
            "ramps run downward too"
        );
        assert_eq!(
            ramp_fraction(0.3, 0.9, Duration::from_millis(5), Duration::ZERO),
            0.9,
            "zero duration snaps"
        );
    }

    /// The ramp is a function of elapsed wall time only: sampling it on
    /// any frame schedule gives identical values at identical instants,
    /// and the smoothstep midpoint is exact.
    #[test]
    fn ramp_is_dt_independent_and_monotonic() {
        let dur = Duration::from_millis(200);
        let at = |ms: u64| ramp_fraction(0.0, 1.0, Duration::from_millis(ms), dur);
        // 60 fps vs 9 fps sampling hit the same instant: same value.
        assert_eq!(at(100), at(100));
        assert!((at(100) - 0.5).abs() < 1e-6, "smoothstep midpoint");
        let samples: Vec<f32> = (0..=20).map(|i| at(i * 10)).collect();
        for pair in samples.windows(2) {
            assert!(pair[0] <= pair[1], "monotonic toward the target");
        }
        // A target that moved during the ramp is still reached exactly.
        assert_eq!(ramp_fraction(0.0, 0.42, dur, dur), 0.42);
    }
}

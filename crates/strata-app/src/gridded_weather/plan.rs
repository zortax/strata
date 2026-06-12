//! Pure fetch planning for the gridded-weather overlays: which (source,
//! valid-time) frames a field needs inside the slider window, and in what
//! order to fetch them — the pair bracketing the slider time first (fast
//! first paint), then outward until the *entire* window is covered, so
//! scrubbing anywhere always finds cached brackets.
//!
//! All times are unix seconds UTC — the same representation
//! [`strata_render::WeatherGridFrame`] carries.

use strata_render::GriddedField;

/// Which provider a frame comes from. Radar (observed + nowcast) and ICON
/// (forecast) both feed the precipitation layer; the cache keys on the
/// source so equal valid-times from both can coexist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridSource {
    Radar,
    Icon,
}

/// Retention margin below the window start: frames that fell *just* out of
/// the window after a re-anchor stay in the renderer push list until they
/// drift past this slack. Re-anchors happen every [`super::REFRESH_INTERVAL`]
/// (10 min) and shift by the wall-clock drift, so 15 min guarantees the
/// push right after a re-anchor is a strict superset of the previous one —
/// the renderer reuses every resident texture, nothing blinks. Frames are
/// only *fetched* inside the strict window.
pub const KEEP_SLACK_SECS: i64 = 15 * 60;

/// Everything one fetch cycle wants for a single render field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPlan {
    pub field: GriddedField,
    /// Every advertised step inside the slider window (plus the
    /// [`KEEP_SLACK_SECS`] retention margin below it), ascending by time —
    /// the frame list pushed to the renderer is assembled from these.
    pub steps: Vec<(GridSource, i64)>,
    /// Every step inside the strict window in fetch order: the bracketing
    /// pair first, then alternating outward to both window edges.
    pub fetch: Vec<(GridSource, i64)>,
}

/// Merge radar and ICON steps into the single precipitation step list.
/// Dedupe rule: **radar wins inside its own range** (observed past +
/// nowcast) — every ICON step with `radar_min <= t <= radar_max` is
/// dropped, including one landing exactly on the radar boundary — ICON
/// only contributes beyond it. The output is therefore strictly ascending
/// with no duplicate valid times, and the radar→ICON hand-over (last radar
/// nowcast step, first later ICON step) is an ordinary adjacent pair the
/// renderer blends across like any other; the slight visual jump there is
/// genuine data disagreement between sensor and model, accepted for v1.
/// Inputs ascending; output ascending.
pub fn merge_steps(radar: &[i64], icon: &[i64]) -> Vec<(GridSource, i64)> {
    let mut steps: Vec<(GridSource, i64)> = radar.iter().map(|&t| (GridSource::Radar, t)).collect();
    let (radar_min, radar_max) = match (radar.first(), radar.last()) {
        (Some(&min), Some(&max)) => (min, max),
        _ => (i64::MAX, i64::MIN), // no radar: every ICON step passes
    };
    steps.extend(
        icon.iter()
            .filter(|&&t| t < radar_min || t > radar_max)
            .map(|&t| (GridSource::Icon, t)),
    );
    steps.sort_by_key(|&(_, t)| t);
    steps
}

/// Restrict `steps` (ascending) to the closed window `[start, end]`.
pub fn clip_to_window(steps: &[(GridSource, i64)], window: (i64, i64)) -> Vec<(GridSource, i64)> {
    steps
        .iter()
        .copied()
        .filter(|&(_, t)| t >= window.0 && t <= window.1)
        .collect()
}

/// Fetch order around `selected`, covering **all** of `steps`: the step
/// at-or-before and the step at-or-after the slider time first (those are
/// what the renderer blends right now), then alternating outward until
/// both ends of the window are reached. `steps` must be ascending by time.
pub fn fetch_order(steps: &[(GridSource, i64)], selected: i64) -> Vec<(GridSource, i64)> {
    if steps.is_empty() {
        return Vec::new();
    }
    // First index with time >= selected; `lo` is the last index with time
    // <= selected (both equal on an exact hit).
    let hi = steps.partition_point(|&(_, t)| t < selected);
    let lo = if hi < steps.len() && steps[hi].1 == selected {
        Some(hi)
    } else {
        hi.checked_sub(1)
    };

    let mut order: Vec<usize> = Vec::new();
    let push = |idx: Option<usize>, order: &mut Vec<usize>| {
        if let Some(idx) = idx
            && idx < steps.len()
            && !order.contains(&idx)
        {
            order.push(idx);
        }
    };

    // Bracketing pair (one index when selected hits a step exactly or lies
    // outside the list).
    push(lo, &mut order);
    push((hi < steps.len()).then_some(hi), &mut order);
    // Outward, alternating earlier/later, until the whole list is covered.
    for d in 1..steps.len() {
        push(lo.and_then(|lo| lo.checked_sub(d)), &mut order);
        push((hi + d < steps.len()).then_some(hi + d), &mut order);
    }

    order.into_iter().map(|idx| steps[idx]).collect()
}

/// Build the full per-field plan: merge sources, clip to the slider window
/// (with the [`KEEP_SLACK_SECS`] retention margin on the push list), derive
/// the full-window fetch order around the slider time.
pub fn plan_field(
    field: GriddedField,
    radar_steps: &[i64],
    icon_steps: &[i64],
    window: (i64, i64),
    selected: i64,
) -> FieldPlan {
    let merged = merge_steps(radar_steps, icon_steps);
    let steps = clip_to_window(&merged, (window.0 - KEEP_SLACK_SECS, window.1));
    let fetch = fetch_order(&clip_to_window(&merged, window), selected);
    FieldPlan {
        field,
        steps,
        fetch,
    }
}

/// Interleave the per-field fetch lists into one global queue, rank-major:
/// every enabled field gets its bracketing pair before any field's deeper
/// prefetch. Entries are `(plan index, step)`.
pub fn interleave_fetches(plans: &[FieldPlan]) -> Vec<(usize, (GridSource, i64))> {
    let max_rank = plans.iter().map(|p| p.fetch.len()).max().unwrap_or(0);
    let mut queue = Vec::with_capacity(plans.iter().map(|p| p.fetch.len()).sum());
    for rank in 0..max_rank {
        for (idx, plan) in plans.iter().enumerate() {
            if let Some(&step) = plan.fetch.get(rank) {
                queue.push((idx, step));
            }
        }
    }
    queue
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;
    const M5: i64 = 300;

    #[test]
    fn merge_prefers_radar_inside_its_range_and_icon_beyond() {
        // Radar: -2 h … +2 h on 5-min lattice; ICON hourly to +24 h.
        let radar: Vec<i64> = (-24..=24).map(|i| i * M5).collect();
        let icon: Vec<i64> = (1..=24).map(|i| i * H).collect();
        let merged = merge_steps(&radar, &icon);

        // Inside the radar range only radar steps survive — the +1 h and
        // +2 h ICON steps coincide with radar nowcast times and are dropped.
        assert!(
            merged
                .iter()
                .filter(|&&(_, t)| (-2 * H..=2 * H).contains(&t))
                .all(|&(source, _)| source == GridSource::Radar)
        );
        // Beyond the radar's last nowcast step ICON takes over: +3 h … +24 h.
        let icon_part: Vec<i64> = merged
            .iter()
            .filter(|&&(source, _)| source == GridSource::Icon)
            .map(|&(_, t)| t)
            .collect();
        assert_eq!(icon_part, (3..=24).map(|i| i * H).collect::<Vec<_>>());
        // The seam: last radar step then first ICON step, ascending overall.
        assert!(merged.windows(2).all(|w| w[0].1 < w[1].1));
        let seam = merged.iter().position(|&(s, _)| s == GridSource::Icon);
        let seam = seam.expect("icon part exists");
        assert_eq!(merged[seam - 1], (GridSource::Radar, 2 * H));
        assert_eq!(merged[seam], (GridSource::Icon, 3 * H));
    }

    /// The radar→ICON boundary: no duplicate valid time (radar wins an
    /// exact collision on its last step), no gap beyond the sources' own
    /// lattices — the seam pair is an ordinary adjacent pair.
    #[test]
    fn merge_seam_has_no_gap_and_no_duplicate_valid_time() {
        // ICON's +2 h step collides exactly with the radar's last nowcast
        // step; ICON's +3 h step is the first beyond.
        let radar: Vec<i64> = (0..=24).map(|i| i * M5).collect(); // 0 … +2 h
        let icon = [H, 2 * H, 3 * H, 4 * H];
        let merged = merge_steps(&radar, &icon);

        // Strictly ascending → no duplicates anywhere.
        assert!(merged.windows(2).all(|w| w[0].1 < w[1].1));
        // Radar wins the exact collision at +2 h.
        assert!(merged.contains(&(GridSource::Radar, 2 * H)));
        assert!(!merged.contains(&(GridSource::Icon, 2 * H)));
        // The boundary pair is adjacent: (radar +2 h, icon +3 h).
        let seam = merged
            .iter()
            .position(|&step| step == (GridSource::Icon, 3 * H))
            .expect("first icon-only step");
        assert_eq!(merged[seam - 1], (GridSource::Radar, 2 * H));
    }

    #[test]
    fn merge_without_radar_is_pure_icon() {
        let icon: Vec<i64> = (0..=4).map(|i| i * H).collect();
        let merged = merge_steps(&[], &icon);
        assert_eq!(merged.len(), 5);
        assert!(merged.iter().all(|&(s, _)| s == GridSource::Icon));
    }

    #[test]
    fn icon_steps_before_the_radar_window_pass_through() {
        let radar = [0, M5, 2 * M5];
        let icon = [-2 * H, -H, 0, H];
        let merged = merge_steps(&radar, &icon);
        assert_eq!(merged[0], (GridSource::Icon, -2 * H));
        assert_eq!(merged[1], (GridSource::Icon, -H));
        // 0 collides with radar's range → radar wins; H is beyond → icon.
        assert!(merged.contains(&(GridSource::Radar, 0)));
        assert!(!merged.contains(&(GridSource::Icon, 0)));
        assert!(merged.contains(&(GridSource::Icon, H)));
    }

    #[test]
    fn window_clipping_is_inclusive() {
        let steps: Vec<(GridSource, i64)> = (0..10).map(|i| (GridSource::Icon, i * H)).collect();
        let clipped = clip_to_window(&steps, (2 * H, 5 * H));
        let times: Vec<i64> = clipped.iter().map(|&(_, t)| t).collect();
        assert_eq!(times, vec![2 * H, 3 * H, 4 * H, 5 * H]);
    }

    /// Brackets first, then alternating outward — and the order covers the
    /// ENTIRE window, not a fixed radius around the slider.
    #[test]
    fn fetch_order_brackets_first_then_covers_the_whole_window() {
        let steps: Vec<(GridSource, i64)> = (0..=10).map(|i| (GridSource::Icon, i * H)).collect();
        // Selected between +4 h and +5 h.
        let order = fetch_order(&steps, 4 * H + 1800);
        let times: Vec<i64> = order.iter().map(|&(_, t)| t / H).collect();
        assert_eq!(times, vec![4, 5, 3, 6, 2, 7, 1, 8, 0, 9, 10]);
        assert_eq!(order.len(), steps.len(), "every step gets fetched");
    }

    #[test]
    fn fetch_order_on_an_exact_step_starts_with_that_step_alone() {
        let steps: Vec<(GridSource, i64)> = (0..=10).map(|i| (GridSource::Icon, i * H)).collect();
        let order = fetch_order(&steps, 4 * H);
        let times: Vec<i64> = order.iter().map(|&(_, t)| t / H).collect();
        // lo == hi == 4: the hit first, then ±1, ±2, … to both edges.
        assert_eq!(times, vec![4, 3, 5, 2, 6, 1, 7, 0, 8, 9, 10]);
    }

    #[test]
    fn fetch_order_clips_at_the_list_edges() {
        let steps: Vec<(GridSource, i64)> = (0..=3).map(|i| (GridSource::Icon, i * H)).collect();
        // Before the first step: forward only.
        let before: Vec<i64> = fetch_order(&steps, -H)
            .iter()
            .map(|&(_, t)| t / H)
            .collect();
        assert_eq!(before, vec![0, 1, 2, 3]);
        // After the last step: backward only.
        let after: Vec<i64> = fetch_order(&steps, 99 * H)
            .iter()
            .map(|&(_, t)| t / H)
            .collect();
        assert_eq!(after, vec![3, 2, 1, 0]);
        assert!(fetch_order(&[], 0).is_empty());
    }

    #[test]
    fn plan_field_combines_merge_clip_and_order() {
        let radar: Vec<i64> = (-24..=24).map(|i| i * M5).collect();
        let icon: Vec<i64> = (1..=48).map(|i| i * H).collect();
        let plan = plan_field(GriddedField::PrecipRate, &radar, &icon, (-2 * H, 24 * H), 0);
        assert_eq!(plan.field, GriddedField::PrecipRate);
        // Selected sits exactly on the radar analysis step.
        assert_eq!(plan.fetch[0], (GridSource::Radar, 0));
        // Steps cover the full window: 49 radar + 22 icon (+3 h … +24 h) —
        // ICON's +25 h … +48 h steps are clipped away by the slider window.
        assert_eq!(plan.steps.len(), 49 + 22);
        // The fetch order covers every window step, none beyond +24 h.
        assert_eq!(plan.fetch.len(), plan.steps.len());
        assert!(plan.fetch.iter().all(|&(_, t)| t <= 24 * H));
        // Every fetched step is in the push list (frames_for finds it).
        assert!(plan.fetch.iter().all(|step| plan.steps.contains(step)));
    }

    /// After a re-anchor (window shifted forward by less than the keep
    /// slack), the new push list is a superset of the old one — the
    /// renderer drops nothing it already holds.
    #[test]
    fn plan_steps_after_a_re_anchor_are_a_superset() {
        let radar: Vec<i64> = (-24..=24).map(|i| i * M5).collect();
        let icon: Vec<i64> = (1..=48).map(|i| i * H).collect();
        let before = plan_field(GriddedField::PrecipRate, &radar, &icon, (-2 * H, 24 * H), 0);
        // Anchor drifted 10 min (one refresh interval).
        let shift = 2 * M5;
        let after = plan_field(
            GriddedField::PrecipRate,
            &radar,
            &icon,
            (-2 * H + shift, 24 * H + shift),
            shift,
        );
        assert!(
            before.steps.iter().all(|step| after.steps.contains(step)),
            "re-anchored push list must retain every previous step"
        );
        // In particular the oldest radar step survives via the keep slack
        // even though it now lies before the strict window start.
        assert!(after.steps.contains(&(GridSource::Radar, -2 * H)));
        // But it is no longer fetched (strict window only).
        assert!(!after.fetch.contains(&(GridSource::Radar, -2 * H)));
    }

    /// The global queue across fields is rank-major: all brackets first.
    #[test]
    fn interleave_serves_every_fields_bracket_before_deeper_prefetch() {
        let steps_a: Vec<(GridSource, i64)> = (0..=4).map(|i| (GridSource::Icon, i * H)).collect();
        let steps_b: Vec<(GridSource, i64)> = (0..=2).map(|i| (GridSource::Radar, i * H)).collect();
        let plan = |field, steps: &Vec<(GridSource, i64)>| FieldPlan {
            field,
            steps: steps.clone(),
            fetch: fetch_order(steps, H + 1800),
        };
        let plans = [
            plan(GriddedField::CloudCover, &steps_a),
            plan(GriddedField::PrecipRate, &steps_b),
        ];
        let queue = interleave_fetches(&plans);
        // Rank 0: both fields' at-or-before step; rank 1: both at-or-after.
        assert_eq!(queue[0], (0, (GridSource::Icon, H)));
        assert_eq!(queue[1], (1, (GridSource::Radar, H)));
        assert_eq!(queue[2], (0, (GridSource::Icon, 2 * H)));
        assert_eq!(queue[3], (1, (GridSource::Radar, 2 * H)));
        // Everything queued exactly once.
        assert_eq!(queue.len(), steps_a.len() + steps_b.len());
        assert!(interleave_fetches(&[]).is_empty());
    }
}

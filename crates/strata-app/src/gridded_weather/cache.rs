//! In-memory frame cache for the gridded-weather overlays.
//!
//! Keyed by (source, field, valid-time) so radar and ICON frames for the
//! same instant coexist. Capped at [`FRAME_CAP`] frames; eviction drops the
//! frames temporally farthest from the current slider time, so the working
//! set around the slider position survives.
//!
//! ## Sizing
//!
//! The scheduler prefetches the **entire** −2 h … +24 h slider window per
//! enabled field, so the cap must fit a full window for all three fields
//! at once (otherwise prefetch would thrash its own tail). Worst case:
//!
//! * cloud cover / thunderstorm: ≤28 hourly ICON steps each (26 h window
//!   + the 15 min keep slack),
//! * precipitation: ≤52 radar steps (5-min lattice over the −2 h … +2 h
//!   radar range, + slack) + ≤23 hourly ICON steps beyond the radar,
//!
//! ≈131 frames total. At ICON-D2 resolution a frame is ~3.6 MB of `f32`s
//! (1215×746), a reprojected DE1200 radar frame ~4.9 MB (1152×1055), so a
//! full cache is ~540 MB worst case with all three layers on — typical
//! single-layer use is ~100 MB (clouds) to ~330 MB (precipitation).

use std::collections::HashMap;

use strata_render::{GriddedField, WeatherGridFrame};

use super::plan::GridSource;

/// Maximum number of cached frames across all fields and sources: the
/// ~131-frame full-window worst case (see the module docs) plus headroom
/// for timeline skew between sources.
pub const FRAME_CAP: usize = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub source: GridSource,
    pub field: GriddedField,
    /// Unix seconds UTC.
    pub valid_time: i64,
}

#[derive(Default)]
pub struct FrameCache {
    frames: HashMap<FrameKey, WeatherGridFrame>,
}

impl FrameCache {
    pub fn contains(&self, key: &FrameKey) -> bool {
        self.frames.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Insert a frame, then evict the entries temporally farthest from
    /// `selected` (the slider time) while over [`FRAME_CAP`].
    pub fn insert(&mut self, key: FrameKey, frame: WeatherGridFrame, selected: i64) {
        self.frames.insert(key, frame);
        while self.frames.len() > FRAME_CAP {
            let Some(farthest) = self
                .frames
                .keys()
                .max_by_key(|k| (k.valid_time - selected).abs())
                .copied()
            else {
                break;
            };
            self.frames.remove(&farthest);
        }
    }

    /// The cached frames for `field` along `steps`, ascending in step order
    /// (steps are ascending by time). Missing steps are skipped — the
    /// renderer blends across whatever gaps remain.
    pub fn frames_for(
        &self,
        field: GriddedField,
        steps: &[(GridSource, i64)],
    ) -> Vec<WeatherGridFrame> {
        steps
            .iter()
            .filter_map(|&(source, valid_time)| {
                self.frames.get(&FrameKey {
                    source,
                    field,
                    valid_time,
                })
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: GridSource, t: i64) -> FrameKey {
        FrameKey {
            source,
            field: GriddedField::PrecipRate,
            valid_time: t,
        }
    }

    fn frame(t: i64) -> WeatherGridFrame {
        WeatherGridFrame {
            field: GriddedField::PrecipRate,
            valid_time: t,
            extent: (47.0, 6.0, 55.0, 15.0),
            ni: 2,
            nj: 2,
            values: vec![0.0; 4],
        }
    }

    /// The cap fits a full prefetched window for all three fields at once
    /// (see the module docs' derivation) — full-window prefetch must never
    /// evict its own tail.
    #[test]
    fn cap_fits_a_full_window_for_all_three_fields() {
        use crate::gridded_weather::plan::KEEP_SLACK_SECS;
        use crate::state::weather_time::{FUTURE_HOURS, PAST_HOURS};

        let window_hours = (PAST_HOURS + FUTURE_HOURS) as usize; // 26
        // Hourly ICON lattice inside the closed window: ≤ span+1 steps,
        // plus one more retained inside the keep slack after a re-anchor.
        let icon_full_window = window_hours + 1 + (KEEP_SLACK_SECS > 0) as usize;
        // Radar advertises a 5-min lattice over −2 h … +2 h around its
        // analysis: 49 steps, plus analysis lag retained by the slack.
        let radar_full_window = 49 + (KEEP_SLACK_SECS / 300) as usize;
        // Precipitation's ICON tail beyond the radar nowcast: ≤ +3 … +24 h
        // hourly, +1 for source/anchor skew.
        let icon_tail = (FUTURE_HOURS as usize - 2) + 1;

        let worst_case = 2 * icon_full_window + radar_full_window + icon_tail;
        assert!(
            worst_case <= FRAME_CAP,
            "worst case {worst_case} frames exceeds FRAME_CAP {FRAME_CAP}"
        );
    }

    #[test]
    fn evicts_the_temporally_farthest_frame_over_cap() {
        let mut cache = FrameCache::default();
        for i in 0..FRAME_CAP as i64 {
            cache.insert(key(GridSource::Icon, i * 3600), frame(i * 3600), 0);
        }
        assert_eq!(cache.len(), FRAME_CAP);
        // Inserting near the selected time evicts the farthest (the last
        // hour), not the new entry.
        cache.insert(key(GridSource::Radar, 300), frame(300), 0);
        assert_eq!(cache.len(), FRAME_CAP);
        assert!(cache.contains(&key(GridSource::Radar, 300)));
        assert!(!cache.contains(&key(GridSource::Icon, (FRAME_CAP as i64 - 1) * 3600)));
    }

    #[test]
    fn radar_and_icon_frames_for_the_same_time_coexist() {
        let mut cache = FrameCache::default();
        cache.insert(key(GridSource::Radar, 3600), frame(3600), 0);
        cache.insert(key(GridSource::Icon, 3600), frame(3600), 0);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn frames_for_returns_cached_steps_in_order_and_skips_missing() {
        let mut cache = FrameCache::default();
        cache.insert(key(GridSource::Icon, 7200), frame(7200), 0);
        cache.insert(key(GridSource::Radar, 0), frame(0), 0);
        let steps = [
            (GridSource::Radar, 0),
            (GridSource::Radar, 300), // not cached
            (GridSource::Icon, 7200),
        ];
        let frames = cache.frames_for(GriddedField::PrecipRate, &steps);
        let times: Vec<i64> = frames.iter().map(|f| f.valid_time).collect();
        assert_eq!(times, vec![0, 7200]);
        // Different field: nothing.
        assert!(cache.frames_for(GriddedField::CloudCover, &steps).is_empty());
    }
}

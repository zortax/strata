//! Gridded weather overlays (cloud cover, precipitation, thunderstorm
//! potential): providers, frame cache, fetch planning and domain→render
//! conversion.
//!
//! The scheduling itself (when cycles run, pushing frames into the
//! renderer) lives in [`crate::map_view`]'s `gridded` submodule — this
//! module is the state it drives plus the pure logic underneath:
//!
//! * [`plan`] — merge radar/ICON steps, full-window fetch order
//!   (bracketing pair first, then outward to both window edges).
//! * [`cache`] — capped in-memory frame cache keyed (source, field, time).
//! * [`convert`] — `WeatherGrid` → `WeatherGridFrame` (+ downsample guard).
//!
//! The controller also remembers the frame-key list last pushed to the
//! renderer per field ([`GriddedWeatherController::record_push`]) so
//! refresh cycles that change nothing never touch the renderer at all.

pub mod cache;
pub mod convert;
pub mod plan;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::Task;
use strata_data::domain::GriddedTimeline;
use strata_data::providers::GriddedWeatherProvider;
use strata_data::providers::dwd_icon::DwdIconD2;
use strata_data::providers::dwd_radar::DwdRadarRv;
use strata_render::GriddedField;

use cache::{FrameCache, FrameKey};
use plan::GridSource;

/// Cadence of the periodic re-fetch while any gridded weather layer is on
/// (new ICON runs appear every 3 h, radar composites every 5 min — 10 min
/// keeps the nowcast reasonably fresh without hammering opendata.dwd.de).
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Debounce between a slider move and the coverage fetch it may need.
pub const SLIDER_FETCH_DEBOUNCE: Duration = Duration::from_millis(400);

/// Frame fetches a cycle keeps in flight at once. Full-window prefetch
/// downloads the whole −2 h … +24 h window per enabled field; four
/// concurrent GRIB/RADOLAN downloads fill it quickly without hammering
/// opendata.dwd.de or saturating the tokio bridge.
pub const FETCH_CONCURRENCY: usize = 4;

/// How long a fetched timeline is reused before the next cycle re-probes
/// the source. Slightly under [`REFRESH_INTERVAL`] so every periodic cycle
/// refreshes, while slider-triggered cycles in between stay network-free.
const TIMELINE_TTL: Duration = Duration::from_secs(9 * 60);

struct CachedTimeline {
    fetched_at: Instant,
    timeline: Arc<GriddedTimeline>,
}

/// Providers, caches and in-flight tasks of the gridded-weather scheduler.
/// Owned by the map view (which holds the renderer the frames go to).
pub struct GriddedWeatherController {
    /// ICON-D2 forecast source (all three fields).
    pub(crate) icon: Arc<dyn GriddedWeatherProvider>,
    /// DWD RV radar composite (observed + nowcast precipitation).
    pub(crate) radar: Arc<dyn GriddedWeatherProvider>,
    pub(crate) cache: FrameCache,
    timelines: HashMap<(GridSource, GriddedField), CachedTimeline>,
    /// The frame-key list last pushed to the renderer, per field. Cached
    /// frame data is immutable once inserted, so an identical key list
    /// means a byte-identical working set — the push is skipped entirely
    /// and the renderer's textures are never disturbed (this is what makes
    /// idle refresh cycles true no-ops).
    pushed: HashMap<GriddedField, Vec<FrameKey>>,
    /// Periodic cycle loop; present while any gridded layer is enabled.
    pub(crate) loop_task: Option<Task<()>>,
    /// The in-flight fetch cycle; replaced (cancelled) by newer cycles.
    pub(crate) cycle_task: Option<Task<()>>,
    /// Debounced slider-move follow-up.
    pub(crate) followup_task: Option<Task<()>>,
}

impl GriddedWeatherController {
    pub fn new() -> Self {
        Self {
            icon: Arc::new(DwdIconD2::new()),
            radar: Arc::new(DwdRadarRv::new()),
            cache: FrameCache::default(),
            timelines: HashMap::new(),
            pushed: HashMap::new(),
            loop_task: None,
            cycle_task: None,
            followup_task: None,
        }
    }

    /// Drop every scheduled/in-flight task (all layers toggled off). Cached
    /// frames, timelines and the pushed bookkeeping survive (the renderer
    /// keeps its frames too), so re-enabling is instant.
    pub fn stop(&mut self) {
        self.loop_task = None;
        self.cycle_task = None;
        self.followup_task = None;
    }

    /// The renderer was recreated (GPU device loss): it holds no frames
    /// anymore, so the pushed bookkeeping must forget what the old one had
    /// or the next cycle would skip re-pushing identical working sets.
    pub fn renderer_reset(&mut self) {
        self.pushed.clear();
    }

    /// Record an intended push of `keys` for `field`; returns whether it
    /// differs from what the renderer already holds (i.e. whether the
    /// caller should actually push). Identical key lists are skipped:
    /// cached frame data never changes under a key, so equal keys mean an
    /// equal working set.
    pub fn record_push(&mut self, field: GriddedField, keys: &[FrameKey]) -> bool {
        if self.pushed.get(&field).map(Vec::as_slice) == Some(keys) {
            return false;
        }
        self.pushed.insert(field, keys.to_vec());
        true
    }

    /// The cached timeline for (source, field) if it is still fresh.
    pub fn fresh_timeline(
        &self,
        source: GridSource,
        field: GriddedField,
    ) -> Option<Arc<GriddedTimeline>> {
        self.timelines
            .get(&(source, field))
            .filter(|cached| cached.fetched_at.elapsed() < TIMELINE_TTL)
            .map(|cached| Arc::clone(&cached.timeline))
    }

    pub fn store_timeline(
        &mut self,
        source: GridSource,
        field: GriddedField,
        timeline: Arc<GriddedTimeline>,
    ) {
        self.timelines.insert(
            (source, field),
            CachedTimeline {
                fetched_at: Instant::now(),
                timeline,
            },
        );
    }
}

impl Default for GriddedWeatherController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use strata_data::domain::{StepKind, TimelineStep};

    use super::*;

    #[test]
    fn timelines_are_cached_per_source_and_field() {
        let mut ctl = GriddedWeatherController::new();
        let run = Utc.with_ymd_and_hms(2026, 6, 10, 9, 0, 0).unwrap();
        let tl = Arc::new(GriddedTimeline {
            run_time: run,
            steps: vec![TimelineStep {
                valid_time: run,
                kind: StepKind::Forecast,
            }],
        });
        ctl.store_timeline(GridSource::Icon, GriddedField::CloudCover, Arc::clone(&tl));

        let fresh = ctl.fresh_timeline(GridSource::Icon, GriddedField::CloudCover);
        assert!(fresh.is_some_and(|got| Arc::ptr_eq(&got, &tl)));
        // Other key: nothing.
        assert!(
            ctl.fresh_timeline(GridSource::Radar, GriddedField::CloudCover)
                .is_none()
        );
    }

    fn key(t: i64) -> FrameKey {
        FrameKey {
            source: GridSource::Icon,
            field: GriddedField::CloudCover,
            valid_time: t,
        }
    }

    /// Identical key lists are no-ops; supersets, removals and a renderer
    /// reset re-push.
    #[test]
    fn record_push_skips_identical_key_lists() {
        let mut ctl = GriddedWeatherController::new();
        let field = GriddedField::CloudCover;
        assert!(ctl.record_push(field, &[key(0), key(3600)]));
        // The idle refresh: same keys → renderer untouched.
        assert!(!ctl.record_push(field, &[key(0), key(3600)]));
        // A new frame arrived (superset) → push.
        assert!(ctl.record_push(field, &[key(0), key(3600), key(7200)]));
        // A frame fell out of the retention window → push (removal).
        assert!(ctl.record_push(field, &[key(3600), key(7200)]));
        assert!(!ctl.record_push(field, &[key(3600), key(7200)]));
        // Other fields are tracked independently.
        assert!(ctl.record_push(GriddedField::PrecipRate, &[key(3600)]));
        // After a renderer recreation everything must be re-pushed.
        ctl.renderer_reset();
        assert!(ctl.record_push(field, &[key(3600), key(7200)]));
    }
}

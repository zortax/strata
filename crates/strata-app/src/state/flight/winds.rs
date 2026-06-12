//! Minimal winds-aloft prefetch for the open flight (plan §5.3).
//!
//! While a flight with a departure time is open, the 4-level ICON-D2
//! U/V wind **and temperature** grids plus the `hzerocl` freezing-level
//! grid are fetched for the timeline steps **bracketing the flight
//! window** (departure → estimated arrival) — the documented minimal
//! wiring. The cached grids snapshot into a [`WindsAloftFrames`] for each
//! compute run; with no frames present the sampler returns `None` and
//! strata-plan applies its calm-ISA fallback per leg, so winds are never
//! silently invented. Temperatures degrade per level: a missing
//! temperature grid pins that level at its ISA temperature with
//! [`Provenance::Isa`](strata_plan::sources::Provenance::Isa) — honest
//! over blocking the wind sample.
//!
//! Fetches ride the tokio bridge like the live-weather loop, **one tokio
//! task per file with each grid landing as it arrives** — a superseded
//! prefetch keeps everything fetched so far, so the follow-up fetch list
//! only shrinks (it used to restart from zero on every compute-relevant
//! keystroke). A live task's plan (window + crop envelope) is kept beside
//! the handle: an edit that produces the same plan never restarts the
//! fetch.
//!
//! Grids are **cropped to the route + alternates envelope** (plus a
//! generous margin) before they cross to the main thread — single-digit
//! MB per window for typical hops instead of ≤ 4 steps × 13 full-domain
//! ICON-D2 grids ≈ 190 MB. A route extending beyond the cached envelope
//! invalidates the cache (cropped grids cannot serve it) and refetches
//! with the larger envelope. Grids outside the current window are pruned
//! when a prefetch lands, so memory stays bounded by one window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gpui::{Context, Task};
use gpui_tokio::Tokio;
use strata_data::domain::{BoundingBox, GriddedTimeline, PressureLevel, WeatherField, WeatherGrid};
use strata_data::providers::GriddedWeatherProvider;
use strata_data::providers::dwd_icon::DwdIconD2;
use strata_plan::route::total_distance;
use strata_plan::{AircraftProfile, FlightDoc};

use crate::sources::{LevelWinds, WindsAloftFrames, WindsTimeStep, points_prefetch_bbox};
use crate::state::AppState;

/// The timeline is probed via one representative field; ICON-D2 publishes
/// all pressure-level fields per run, so the timelines coincide. A step
/// that 404s for a sibling field is logged and skipped by the fetch loop.
const TIMELINE_FIELD: WeatherField = WeatherField::WindU(PressureLevel::P850);

/// How long a fetched timeline is reused (matches the gridded-overlay
/// scheduler's TTL rationale: ICON runs appear every 3 h).
const TIMELINE_TTL: Duration = Duration::from_secs(9 * 60);

/// Cruise TAS assumed for the window estimate when no aircraft profile is
/// resolved yet (a slow conservative GA figure → a longer window).
const DEFAULT_PLANNING_TAS_KT: f64 = 90.0;

/// Bounds on the estimated flight duration (degenerate TAS/distances must
/// not produce empty or absurd windows).
const MIN_WINDOW_MINUTES: f64 = 15.0;
const MAX_WINDOW_MINUTES: f64 = 12.0 * 60.0;

/// Padding around the route/alternates envelope for the grid crop: legs
/// sample on the track only, but a generous margin keeps small route
/// nudges inside the cached envelope (a ~2° crop is ~1% of the full
/// ICON-D2 domain — the margin costs next to nothing).
const CROP_MARGIN_METERS: f64 = 50_000.0;

/// A cached-grid key: the flight-planning [`WeatherField`] (wind
/// component / temperature per level, or the freezing level) at one valid
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridKey {
    pub field: WeatherField,
    /// Unix seconds UTC.
    pub valid_unix: i64,
}

/// The fields prefetched per timeline step: U/V/T × all four pressure
/// levels plus the `hzerocl` freezing level — everything the sampler's
/// documented fallback chains can consume.
pub fn prefetched_fields() -> impl Iterator<Item = WeatherField> {
    PressureLevel::ALL
        .into_iter()
        .flat_map(|level| {
            [
                WeatherField::WindU(level),
                WeatherField::WindV(level),
                WeatherField::Temperature(level),
            ]
        })
        .chain([WeatherField::FreezingLevel])
}

/// The cache key for a fetched grid; `None` for fields the flight prefetch
/// never asked for.
fn key_for(grid: &WeatherGrid) -> Option<GridKey> {
    match grid.field {
        WeatherField::WindU(_)
        | WeatherField::WindV(_)
        | WeatherField::Temperature(_)
        | WeatherField::FreezingLevel => Some(GridKey {
            field: grid.field,
            valid_unix: grid.valid_time.timestamp(),
        }),
        _ => None,
    }
}

/// What a live prefetch task is working on. An edit that produces the
/// same plan must not restart the fetch — each restart aborts the tokio
/// task, and altitude/name keystrokes used to restart a whole window's
/// downloads for nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PrefetchPlan {
    window: (DateTime<Utc>, DateTime<Utc>),
    crop: BoundingBox,
}

/// Provider, cache and in-flight task of the flight winds prefetch.
/// Owned by [`AppState`]; plain state apart from the gpui task handle.
pub struct FlightWinds {
    provider: Arc<dyn GriddedWeatherProvider>,
    grids: HashMap<GridKey, Arc<WeatherGrid>>,
    timeline: Option<(Instant, Arc<GriddedTimeline>)>,
    /// The envelope every cached grid is cropped to; `None` until the
    /// first prefetch (tests inserting uncropped grids leave it `None`).
    crop_bbox: Option<BoundingBox>,
    task: Option<Task<()>>,
    /// The live task's plan. Cleared by the completion handler — the
    /// fetch loop is skip-and-continue on per-file failures, so a
    /// finished-but-incomplete task must not keep suppressing retries.
    /// (Kept separate from `task`: the completion runs *inside* the task,
    /// and dropping the handle from within its own poll would abort it.)
    task_plan: Option<PrefetchPlan>,
}

impl FlightWinds {
    pub fn new() -> Self {
        Self {
            provider: Arc::new(DwdIconD2::new()),
            grids: HashMap::new(),
            timeline: None,
            crop_bbox: None,
            task: None,
            task_plan: None,
        }
    }

    /// Drops the in-flight prefetch (flight closed). Cached grids survive —
    /// they are cropped to the closed flight's route (single-digit MB),
    /// and reopening a flight in the same window/area is instant.
    pub fn stop(&mut self) {
        self.task = None;
        self.task_plan = None;
    }

    /// Ensures the cache's crop envelope covers `required`, clearing the
    /// cached grids when it does not — cropped grids cannot serve points
    /// outside their envelope, and keeping them would silently degrade
    /// the extended legs to the calm-ISA fallback. Returns the effective
    /// envelope all of this cache's grids are cropped to: the cached one
    /// while it still covers `required` (so cache and envelope always
    /// agree), the new one after a clear.
    fn ensure_crop_covers(&mut self, required: BoundingBox) -> BoundingBox {
        match self.crop_bbox {
            Some(cached) if cached.contains_bbox(&required) => cached,
            _ => {
                self.grids.clear();
                self.crop_bbox = Some(required);
                required
            }
        }
    }

    /// Whether the just-inserted grid advanced what the sampler can serve
    /// — a wind grid completing its U/V pair, a temperature refining an
    /// already-complete pair, or a freezing-level grid (its chain serves
    /// alone). Gates the per-grid recompute during incremental landing: a
    /// U grid without its V partner cannot change any output yet.
    fn grid_advances_sampling(&self, field: WeatherField, valid_unix: i64) -> bool {
        let has = |field| self.grid_at(field, valid_unix).is_some();
        match field {
            WeatherField::WindU(level) => has(WeatherField::WindV(level)),
            WeatherField::WindV(level) => has(WeatherField::WindU(level)),
            WeatherField::Temperature(level) => {
                has(WeatherField::WindU(level)) && has(WeatherField::WindV(level))
            }
            WeatherField::FreezingLevel => true,
            _ => false,
        }
    }

    fn fresh_timeline(&self) -> Option<Arc<GriddedTimeline>> {
        self.timeline
            .as_ref()
            .filter(|(at, _)| at.elapsed() < TIMELINE_TTL)
            .map(|(_, tl)| Arc::clone(tl))
    }

    fn insert(&mut self, grid: WeatherGrid) {
        match key_for(&grid) {
            Some(key) => {
                self.grids.insert(key, Arc::new(grid));
            }
            None => tracing::warn!(field = %grid.field, "unexpected grid in winds prefetch"),
        }
    }

    /// Drops every cached grid whose valid time is not in `keep` — the
    /// memory bound after a window change.
    fn prune(&mut self, keep: &[DateTime<Utc>]) {
        let keep: HashSet<i64> = keep.iter().map(|t| t.timestamp()).collect();
        self.grids.retain(|key, _| keep.contains(&key.valid_unix));
    }

    fn grid_at(&self, field: WeatherField, valid_unix: i64) -> Option<&Arc<WeatherGrid>> {
        self.grids.get(&GridKey { field, valid_unix })
    }

    /// Immutable snapshot for one compute run: per valid time, every level
    /// with **both** wind components cached (a half-fetched level
    /// contributes nothing — honest over interpolating with a missing
    /// component); the level's temperature grid and the step's
    /// freezing-level grid attach where cached, degrading to the sampler's
    /// labelled ISA fallbacks where not.
    pub fn frames_snapshot(&self) -> Arc<WindsAloftFrames> {
        let times: HashSet<i64> = self.grids.keys().map(|k| k.valid_unix).collect();
        let steps = times
            .into_iter()
            .filter_map(|valid_unix| {
                let levels: Vec<LevelWinds> = PressureLevel::ALL
                    .into_iter()
                    .filter_map(|level| {
                        let u = self.grid_at(WeatherField::WindU(level), valid_unix)?;
                        let v = self.grid_at(WeatherField::WindV(level), valid_unix)?;
                        Some(LevelWinds {
                            level,
                            u: Arc::clone(u),
                            v: Arc::clone(v),
                            temperature: self
                                .grid_at(WeatherField::Temperature(level), valid_unix)
                                .map(Arc::clone),
                        })
                    })
                    .collect();
                let freezing_level = self
                    .grid_at(WeatherField::FreezingLevel, valid_unix)
                    .map(Arc::clone);
                if levels.is_empty() && freezing_level.is_none() {
                    return None; // nothing usable at this time
                }
                Some(WindsTimeStep {
                    valid_time: DateTime::<Utc>::from_timestamp(valid_unix, 0)?,
                    levels,
                    freezing_level,
                })
            })
            .collect();
        Arc::new(WindsAloftFrames::new(steps))
    }
}

impl Default for FlightWinds {
    fn default() -> Self {
        Self::new()
    }
}

/// The flight's time window: departure → departure + estimated duration
/// (total route distance at the selected cruise TAS, else
/// [`DEFAULT_PLANNING_TAS_KT`]; clamped to 15 min … 12 h). `None` without a
/// departure time or with fewer than two waypoints — there is nothing to
/// sample then (strata-plan's sampler contract needs a time).
pub fn flight_time_window(
    doc: &FlightDoc,
    cruise_tas_kt: Option<f64>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let departure = doc.departure_time?;
    if doc.route.len() < 2 {
        return None;
    }
    let distance_nm = total_distance(&doc.route).0 / 1852.0;
    let tas = cruise_tas_kt
        .filter(|t| *t > 1.0)
        .unwrap_or(DEFAULT_PLANNING_TAS_KT);
    let minutes = (distance_nm / tas * 60.0).clamp(MIN_WINDOW_MINUTES, MAX_WINDOW_MINUTES);
    let end = departure + chrono::Duration::milliseconds((minutes * 60_000.0) as i64);
    Some((departure, end))
}

fn selected_cruise_tas_kt(aircraft: &AircraftProfile, doc: &FlightDoc) -> Option<f64> {
    let settings = &aircraft.performance.cruise_settings;
    match doc.power_setting.as_deref() {
        Some(name) => settings
            .iter()
            .find(|setting| setting.name == name)
            .or_else(|| settings.first()),
        None => settings.first(),
    }
    .map(|setting| setting.tas.0)
}

/// The valid times to prefetch for `window`: the timeline steps bracketing
/// the window start plus those bracketing the window end (deduplicated,
/// ascending) — ≤ 4 instants. Mid-window legs sample the nearest of these
/// (documented approximation in `sources::winds`). Steps farther than the
/// sampler's serving distance from the window are dropped — they could
/// never be sampled, so fetching them would be pure waste (a window
/// entirely outside the timeline therefore fetches nothing).
pub fn prefetch_instants(
    timeline: &GriddedTimeline,
    window: (DateTime<Utc>, DateTime<Utc>),
) -> Vec<DateTime<Utc>> {
    let (start, end) = window;
    let usable = |t: &DateTime<Utc>| {
        let outside = (start - *t).num_seconds().max((*t - end).num_seconds());
        outside <= crate::sources::winds::MAX_STEP_DISTANCE_SECS
    };
    let mut instants: Vec<DateTime<Utc>> = [
        timeline.bracketing_steps(start),
        timeline.bracketing_steps(end),
    ]
    .into_iter()
    .flat_map(|(a, b)| [a, b])
    .flatten()
    .map(|s| s.valid_time)
    .filter(usable)
    .collect();
    instants.sort_unstable();
    instants.dedup();
    instants
}

/// The (field, valid time) fetches still missing from `cached`:
/// U/V/T × all four levels plus the freezing level × `instants`.
pub fn needed_fetches(
    instants: &[DateTime<Utc>],
    cached: &HashSet<GridKey>,
) -> Vec<(WeatherField, DateTime<Utc>)> {
    let mut needed = Vec::new();
    for &t in instants {
        for field in prefetched_fields() {
            let key = GridKey {
                field,
                valid_unix: t.timestamp(),
            };
            if !cached.contains(&key) {
                needed.push((field, t));
            }
        }
    }
    needed
}

impl AppState {
    /// An immutable snapshot of the prefetched flight winds/temperature
    /// grids — what the weather surfaces (freezing-level chain, briefing
    /// conversion) read alongside the computed outputs.
    pub fn flight_winds_frames(&self) -> Arc<WindsAloftFrames> {
        self.flight_winds.frames_snapshot()
    }

    /// Kicks off (or skips) the winds-aloft prefetch for the open flight's
    /// window. Cheap when nothing is needed: with a fresh timeline and a
    /// fully cached window no task is spawned at all, and a live task
    /// already working the same plan (window + crop envelope) is left
    /// alone — altitude/name keystrokes never change the plan. Grids land
    /// **incrementally**, each cropped to the route envelope in the tokio
    /// worker and scheduling the debounced recompute as soon as it makes
    /// something newly sampleable.
    pub(crate) fn maybe_prefetch_flight_winds(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = &self.flight else {
            return;
        };
        let tas = self
            .flight_aircraft()
            .and_then(|aircraft| selected_cruise_tas_kt(aircraft, &flight.doc));
        let Some(window) = flight_time_window(&flight.doc, tas) else {
            return;
        };
        let Some(required) = points_prefetch_bbox(
            flight
                .doc
                .route
                .iter()
                .map(|w| w.position())
                .chain(flight.doc.alternates.iter().map(|p| p.position())),
            CROP_MARGIN_METERS,
        ) else {
            return; // unreachable with ≥ 2 waypoints, but stay honest
        };
        // Clears the cache when the route outgrew the cached envelope.
        let crop = self.flight_winds.ensure_crop_covers(required);

        let plan = PrefetchPlan { window, crop };
        if self.flight_winds.task_plan == Some(plan) {
            return; // a live prefetch is already working exactly this plan
        }

        let cached: HashSet<GridKey> = self.flight_winds.grids.keys().copied().collect();
        let timeline = self.flight_winds.fresh_timeline();
        if let Some(timeline) = &timeline {
            let instants = prefetch_instants(timeline, window);
            if needed_fetches(&instants, &cached).is_empty() {
                // Window fully cached — no task, no churn. Any still-live
                // task belongs to a *different* plan (same plans returned
                // above): drop it, or its landing would prune this
                // window's grids to the old plan's instants.
                if self.flight_winds.task_plan.take().is_some() {
                    self.flight_winds.task = None;
                }
                return;
            }
        }

        let provider = Arc::clone(&self.flight_winds.provider);
        self.flight_winds.task_plan = Some(plan);
        // Replacing the task aborts a superseded prefetch; everything that
        // already landed stays cached, so the new task's fetch list only
        // shrinks.
        self.flight_winds.task = Some(cx.spawn(async move |this, cx| {
            // Timeline: cached, or probed through the tokio bridge.
            let timeline = match timeline {
                Some(timeline) => timeline,
                None => {
                    let provider = Arc::clone(&provider);
                    let probe = Tokio::spawn_result(cx, async move {
                        anyhow::Ok(Arc::new(provider.timeline(TIMELINE_FIELD).await?))
                    });
                    match probe.await {
                        Ok(timeline) => {
                            let stamped = (Instant::now(), Arc::clone(&timeline));
                            if this
                                .update(cx, |this, _| this.flight_winds.timeline = Some(stamped))
                                .is_err()
                            {
                                return;
                            }
                            timeline
                        }
                        Err(err) => {
                            tracing::warn!(%err, "winds-aloft timeline probe failed");
                            this.update(cx, |this, _| this.flight_winds.task_plan = None)
                                .ok();
                            return;
                        }
                    }
                }
            };

            let instants = prefetch_instants(&timeline, window);
            let mut landed = 0_usize;
            for (field, valid) in needed_fetches(&instants, &cached) {
                let provider = Arc::clone(&provider);
                // One tokio task per file, landing each grid as it
                // arrives: an aborted prefetch keeps everything fetched so
                // far, and the crop happens in the worker — the 3.6 MB
                // full-domain buffer never crosses to the main thread.
                let fetch = Tokio::spawn_result(cx, async move {
                    anyhow::Ok(provider.fetch(field, valid).await?.cropped(crop))
                });
                match fetch.await {
                    Ok(Some(grid)) => {
                        landed += 1;
                        let update = this.update(cx, |this, cx| {
                            this.flight_winds.insert(grid);
                            if this
                                .flight_winds
                                .grid_advances_sampling(field, valid.timestamp())
                                && this.flight.is_some()
                            {
                                // The 50 ms compute debounce coalesces
                                // fast-landing bursts.
                                this.schedule_flight_compute(cx);
                            }
                        });
                        if update.is_err() {
                            return;
                        }
                    }
                    // The route lies entirely outside this grid's domain:
                    // nothing worth caching — the sampler could never
                    // serve the route from it anyway.
                    Ok(None) => {
                        tracing::warn!(%field, valid = %valid, "winds grid does not cover the route");
                    }
                    // Skip-and-continue: a missing sibling file must not
                    // sink the whole window (the sampler copes with
                    // partial levels and missing temperatures honestly).
                    Err(err) => tracing::warn!(%field, valid = %valid, %err, "winds fetch failed"),
                }
            }
            this.update(cx, |this, _| {
                this.flight_winds.prune(&instants);
                // Clear the plan, not the task handle (see `task_plan`):
                // a finished-but-incomplete fetch must not suppress the
                // next edit's retry of the files that failed.
                this.flight_winds.task_plan = None;
                if landed > 0 {
                    tracing::info!(grids = landed, "winds-aloft prefetch finished");
                }
            })
            .ok();
        }));
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::{LatLon, RegularLatLonGrid, StepKind, TimelineStep};
    use strata_plan::aircraft::{AircraftId, PowerSetting};
    use strata_plan::flight::{FreePoint, RoutePoint, RouteWaypoint};
    use strata_plan::units::{Knots, LitersPerHour};

    use super::*;

    fn t(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 14, hour, minute, 0).unwrap()
    }

    fn hourly_timeline(from_hour: u32, to_hour: u32) -> GriddedTimeline {
        GriddedTimeline {
            run_time: t(from_hour, 0),
            steps: (from_hour..=to_hour)
                .map(|h| TimelineStep {
                    valid_time: t(h, 0),
                    kind: StepKind::Forecast,
                })
                .collect(),
        }
    }

    fn doc_with_route(departure: Option<DateTime<Utc>>, nm: f64) -> FlightDoc {
        let mut doc = FlightDoc::new("t");
        // 1° of longitude at the equator ≈ 60 NM.
        let lon_span = nm / 60.0;
        let wp = |lat: f64, lon: f64| {
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: None,
                position: LatLon::new(lat, lon).unwrap(),
            }))
        };
        doc.route = vec![wp(0.0, 8.0), wp(0.0, 8.0 + lon_span)];
        doc.departure_time = departure;
        doc
    }

    #[test]
    fn window_needs_departure_and_route() {
        assert!(flight_time_window(&doc_with_route(None, 100.0), None).is_none());
        let mut no_route = doc_with_route(Some(t(9, 0)), 100.0);
        no_route.route.truncate(1);
        assert!(flight_time_window(&no_route, None).is_none());
    }

    #[test]
    fn window_scales_with_distance_and_tas() {
        // 90 NM at the 90 kt default → 60 min.
        let (start, end) = flight_time_window(&doc_with_route(Some(t(9, 0)), 90.0), None).unwrap();
        assert_eq!(start, t(9, 0));
        let minutes = (end - start).num_minutes();
        assert!((59..=61).contains(&minutes), "{minutes} min");

        // The profile's cruise TAS shortens it.
        let (_, end) =
            flight_time_window(&doc_with_route(Some(t(9, 0)), 90.0), Some(120.0)).unwrap();
        let minutes = (end - t(9, 0)).num_minutes();
        assert!((44..=46).contains(&minutes), "{minutes} min");

        // Degenerate short hops clamp to the minimum window.
        let (_, end) = flight_time_window(&doc_with_route(Some(t(9, 0)), 1.0), None).unwrap();
        assert_eq!((end - t(9, 0)).num_minutes(), 15);
    }

    #[test]
    fn selected_cruise_tas_follows_the_document_power_setting() {
        let mut aircraft = AircraftProfile::new(AircraftId::new("test").unwrap());
        aircraft.performance.cruise_settings = vec![
            PowerSetting {
                name: "fast".to_owned(),
                tas: Knots(120.0),
                fuel_flow: LitersPerHour(34.0),
            },
            PowerSetting {
                name: "economy".to_owned(),
                tas: Knots(95.0),
                fuel_flow: LitersPerHour(24.0),
            },
        ];

        let mut doc = doc_with_route(Some(t(9, 0)), 90.0);
        assert_eq!(selected_cruise_tas_kt(&aircraft, &doc), Some(120.0));
        doc.power_setting = Some("economy".to_owned());
        assert_eq!(selected_cruise_tas_kt(&aircraft, &doc), Some(95.0));
        doc.power_setting = Some("stale".to_owned());
        assert_eq!(selected_cruise_tas_kt(&aircraft, &doc), Some(120.0));
    }

    #[test]
    fn instants_bracket_both_window_edges() {
        let timeline = hourly_timeline(6, 18);
        // 09:30 → 10:40: brackets {09,10} ∪ {10,11} = {09,10,11}.
        let instants = prefetch_instants(&timeline, (t(9, 30), t(10, 40)));
        assert_eq!(instants, vec![t(9, 0), t(10, 0), t(11, 0)]);

        // Exactly on steps: both brackets collapse.
        let instants = prefetch_instants(&timeline, (t(9, 0), t(11, 0)));
        assert_eq!(instants, vec![t(9, 0), t(11, 0)]);

        // Window beyond the timeline end: only the inside edges exist.
        let instants = prefetch_instants(&timeline, (t(17, 30), t(20, 0)));
        assert_eq!(instants, vec![t(17, 0), t(18, 0)]);

        // Window entirely outside: nothing to fetch.
        let instants = prefetch_instants(&timeline, (t(20, 0), t(22, 0)));
        assert!(instants.is_empty());
    }

    #[test]
    fn needed_fetches_cover_uvt_levels_and_hzerocl_minus_cache() {
        let instants = vec![t(9, 0), t(10, 0)];
        let empty = HashSet::new();
        let all = needed_fetches(&instants, &empty);
        // 2 instants × (4 levels × {U, V, T} + freezing level).
        assert_eq!(all.len(), 2 * (4 * 3 + 1));
        assert!(all.contains(&(WeatherField::WindU(PressureLevel::P500), t(9, 0))));
        assert!(all.contains(&(WeatherField::WindV(PressureLevel::P950), t(10, 0))));
        assert!(all.contains(&(WeatherField::Temperature(PressureLevel::P850), t(9, 0))));
        assert!(all.contains(&(WeatherField::FreezingLevel, t(10, 0))));

        // Cached keys drop out.
        let cached: HashSet<GridKey> = [
            GridKey {
                field: WeatherField::WindU(PressureLevel::P500),
                valid_unix: t(9, 0).timestamp(),
            },
            GridKey {
                field: WeatherField::FreezingLevel,
                valid_unix: t(9, 0).timestamp(),
            },
        ]
        .into();
        let remaining = needed_fetches(&instants, &cached);
        assert_eq!(remaining.len(), 2 * (4 * 3 + 1) - 2);
        assert!(!remaining.contains(&(WeatherField::WindU(PressureLevel::P500), t(9, 0))));
        assert!(!remaining.contains(&(WeatherField::FreezingLevel, t(9, 0))));
        assert!(remaining.contains(&(WeatherField::FreezingLevel, t(10, 0))));
    }

    fn test_grid(field: WeatherField, valid: DateTime<Utc>) -> WeatherGrid {
        WeatherGrid {
            field,
            run_time: valid,
            valid_time: valid,
            grid: RegularLatLonGrid::new(
                LatLon::new(46.0, 5.0).unwrap(),
                10.0,
                10.0,
                2,
                2,
                vec![1.0; 4],
            )
            .unwrap(),
        }
    }

    #[test]
    fn snapshot_groups_complete_uv_pairs_and_prune_bounds_the_cache() {
        let mut winds = FlightWinds::new();
        assert!(winds.frames_snapshot().is_empty());

        // P850 has both components at 09Z; P700 only U → excluded.
        winds.insert(test_grid(WeatherField::WindU(PressureLevel::P850), t(9, 0)));
        winds.insert(test_grid(WeatherField::WindV(PressureLevel::P850), t(9, 0)));
        winds.insert(test_grid(WeatherField::WindU(PressureLevel::P700), t(9, 0)));
        // A complete pair at 10Z.
        winds.insert(test_grid(
            WeatherField::WindU(PressureLevel::P850),
            t(10, 0),
        ));
        winds.insert(test_grid(
            WeatherField::WindV(PressureLevel::P850),
            t(10, 0),
        ));

        let frames = winds.frames_snapshot();
        assert_eq!(frames.step_count(), 2);

        // Pruning to the 10Z window drops the 09Z grids.
        winds.prune(&[t(10, 0)]);
        assert_eq!(winds.grids.len(), 2);
        assert_eq!(winds.frames_snapshot().step_count(), 1);
    }

    #[test]
    fn route_outgrowing_the_crop_envelope_invalidates_the_cache() {
        let mut winds = FlightWinds::new();
        winds.insert(test_grid(WeatherField::WindU(PressureLevel::P850), t(9, 0)));
        winds.insert(test_grid(WeatherField::WindV(PressureLevel::P850), t(9, 0)));
        let cached_envelope = BoundingBox::new(8.0, 49.0, 12.0, 52.0).unwrap();
        winds.crop_bbox = Some(cached_envelope);

        // A route still inside the envelope keeps the cache, and keeps
        // the *cached* (larger) envelope so cache and crop stay agreed.
        let inside = BoundingBox::new(9.0, 50.0, 11.0, 51.0).unwrap();
        assert_eq!(winds.ensure_crop_covers(inside), cached_envelope);
        assert_eq!(winds.grids.len(), 2);

        // Extending beyond it clears the grids: needed_fetches then
        // reports the whole window as missing, and the refetch uses the
        // enlarged envelope.
        let beyond = BoundingBox::new(9.0, 50.0, 13.5, 51.0).unwrap();
        assert_eq!(winds.ensure_crop_covers(beyond), beyond);
        assert!(winds.grids.is_empty());
        let cached: HashSet<GridKey> = winds.grids.keys().copied().collect();
        assert_eq!(
            needed_fetches(&[t(9, 0)], &cached).len(),
            4 * 3 + 1,
            "every field of the window is needed again"
        );
    }

    #[test]
    fn incremental_recompute_gates_on_grids_that_advance_sampling() {
        let mut winds = FlightWinds::new();
        let at = t(9, 0).timestamp();
        winds.insert(test_grid(WeatherField::WindU(PressureLevel::P850), t(9, 0)));
        assert!(
            !winds.grid_advances_sampling(WeatherField::WindU(PressureLevel::P850), at),
            "U alone cannot be sampled yet"
        );
        winds.insert(test_grid(WeatherField::WindV(PressureLevel::P850), t(9, 0)));
        assert!(
            winds.grid_advances_sampling(WeatherField::WindV(PressureLevel::P850), at),
            "V completes the pair"
        );
        winds.insert(test_grid(
            WeatherField::Temperature(PressureLevel::P700),
            t(9, 0),
        ));
        assert!(
            !winds.grid_advances_sampling(WeatherField::Temperature(PressureLevel::P700), at),
            "a temperature without its U/V pair changes no output"
        );
        winds.insert(test_grid(
            WeatherField::Temperature(PressureLevel::P850),
            t(9, 0),
        ));
        assert!(winds.grid_advances_sampling(WeatherField::Temperature(PressureLevel::P850), at));
        assert!(
            winds.grid_advances_sampling(WeatherField::FreezingLevel, at),
            "the freezing chain serves alone"
        );
    }

    #[test]
    fn snapshot_attaches_temperature_and_freezing_grids() {
        let mut winds = FlightWinds::new();
        winds.insert(test_grid(WeatherField::WindU(PressureLevel::P850), t(9, 0)));
        winds.insert(test_grid(WeatherField::WindV(PressureLevel::P850), t(9, 0)));
        winds.insert(test_grid(
            WeatherField::Temperature(PressureLevel::P850),
            t(9, 0),
        ));
        // A temperature without its U/V pair never forms a level…
        winds.insert(test_grid(
            WeatherField::Temperature(PressureLevel::P700),
            t(9, 0),
        ));
        winds.insert(test_grid(WeatherField::FreezingLevel, t(9, 0)));

        let frames = winds.frames_snapshot();
        assert_eq!(frames.step_count(), 1);
        let position = LatLon::new(50.0, 10.0).unwrap();
        // …but the freezing-level chain serves from the hzerocl grid.
        let (level, source) = frames.freezing_level(position, t(9, 0)).expect("hzerocl");
        assert_eq!(source, crate::sources::FreezingLevelSource::Forecast);
        assert!((level.0 - 1.0).abs() < 1e-6);

        // A time with only an hzerocl grid still snapshots as a step (the
        // freezing chain can serve where wind sampling honestly cannot).
        winds.insert(test_grid(WeatherField::FreezingLevel, t(10, 0)));
        assert_eq!(winds.frames_snapshot().step_count(), 2);
    }
}

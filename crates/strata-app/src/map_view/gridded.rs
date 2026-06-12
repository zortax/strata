//! Gridded-weather fetch scheduling for the map view.
//!
//! Lifecycle: toggling any of the three gridded layers on (re)starts a
//! periodic loop ([`REFRESH_INTERVAL`]) of fetch *cycles*; toggling the
//! last one off drops every task. A cycle:
//!
//! 1. re-anchors the slider window at the wall clock (no-op while the
//!    drift is below one slider step; never moves an explicit selection),
//! 2. resolves timelines (TTL-cached) — ICON-D2 for every enabled field,
//!    plus the RV radar composite for precipitation,
//! 3. plans per-field steps: radar and ICON merge into one precipitation
//!    list (radar preferred inside its observed+nowcast range, ICON
//!    beyond — see [`plan::merge_steps`] for the seam/dedupe rule),
//! 4. pushes already-cached frames immediately, then fetches the **entire
//!    window**: the two frames bracketing the slider time first (fast
//!    first paint), then outward to both window edges, round-robin across
//!    fields, with up to [`FETCH_CONCURRENCY`] downloads in flight —
//!    pushing each field's grown frame list after every arrival.
//!
//! With the whole −2 h … +24 h window prefetched, scrubbing anywhere finds
//! cached brackets and stays a continuous dissolve. Slider moves still
//! trigger a debounced extra cycle as a safety net (e.g. frames that
//! failed to fetch); the frame cache makes overlapping cycles cheap.
//! Pushes are deduplicated against the last pushed key list
//! ([`record_push`](crate::gridded_weather::GriddedWeatherController::record_push))
//! so refresh cycles that change nothing never touch the renderer, and
//! the plan's retention slack
//! keeps post-re-anchor pushes strict supersets — resident renderer
//! textures are never dropped by a window shift. Network and GRIB/RADOLAN
//! decoding run on the tokio bridge (the providers are reqwest-based and
//! carry their own timeouts); failures are logged and retried on the next
//! cycle — a cycle never wedges the loop.

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::DateTime;
use gpui::{AsyncApp, Context, Task, WeakEntity};
use gpui_tokio::Tokio;
use strata_data::domain::GriddedTimeline;
use strata_data::providers::GriddedWeatherProvider;
use strata_render::{GriddedField, WeatherGridFrame};

use crate::gridded_weather::cache::FrameKey;
use crate::gridded_weather::plan::{self, FieldPlan, GridSource};
use crate::gridded_weather::{FETCH_CONCURRENCY, REFRESH_INTERVAL, SLIDER_FETCH_DEBOUNCE, convert};

use super::MapView;

impl MapView {
    /// The gridded fields whose layers are currently toggled on.
    pub(super) fn enabled_gridded_fields(&self) -> Vec<GriddedField> {
        let Some(cell) = &self.cell else {
            return Vec::new();
        };
        let cell = cell.lock();
        GriddedField::ALL
            .into_iter()
            .filter(|field| cell.renderer.layer_enabled(field.layer()))
            .collect()
    }

    pub fn any_gridded_layer_enabled(&self) -> bool {
        !self.enabled_gridded_fields().is_empty()
    }

    /// Called whenever a gridded layer toggle changes (and after renderer
    /// recreation): (re)start the periodic loop while any layer is on —
    /// restarting runs an immediate cycle for the just-enabled field —
    /// or drop all scheduling when the last one turned off.
    pub(super) fn sync_gridded_weather(&mut self, cx: &mut Context<Self>) {
        if self.any_gridded_layer_enabled() {
            self.start_gridded_loop(cx);
        } else {
            self.gridded_weather.stop();
        }
    }

    fn start_gridded_loop(&mut self, cx: &mut Context<Self>) {
        self.gridded_weather.loop_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let keep_running = this.update(cx, |this, cx| {
                    let on = this.any_gridded_layer_enabled();
                    if on {
                        this.run_gridded_cycle(cx);
                    }
                    on
                });
                if !matches!(keep_running, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        }));
    }

    /// Renderer + coverage reaction to a slider move: scrub the blend time
    /// immediately, then (debounced) make sure frames around the new
    /// position exist. With full-window prefetch the follow-up cycle is
    /// normally a no-op safety net for frames that failed to fetch.
    pub(super) fn on_weather_time_changed(&mut self, cx: &mut Context<Self>) {
        self.push_weather_time(cx);
        if !self.any_gridded_layer_enabled() {
            return;
        }
        // Replacing the task debounces a continuing scrub.
        self.gridded_weather.followup_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLIDER_FETCH_DEBOUNCE).await;
            this.update(cx, |this, cx| this.run_gridded_cycle(cx)).ok();
        }));
    }

    /// A fetch cycle re-anchored the window: sync the renderer's blend
    /// time (the selection only moved if it was tracking "now" or got
    /// clamped), but schedule **no** follow-up cycle — the cycle that
    /// re-anchored is already fetching against the shifted window, and a
    /// follow-up would cancel it mid-flight.
    pub(super) fn on_weather_time_reanchored(&mut self, cx: &mut Context<Self>) {
        self.push_weather_time(cx);
    }

    fn push_weather_time(&mut self, cx: &mut Context<Self>) {
        let selected = self.app_state.read(cx).weather_time.selected().timestamp();
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_weather_time(selected);
            cx.notify();
        }
    }

    /// Push `plan`'s cached frames to the renderer unless the renderer
    /// already holds exactly this working set (cached frame data is
    /// immutable under its key, so equal key lists mean identical data —
    /// see [`GriddedWeatherController::record_push`]). Returns whether a
    /// push happened.
    ///
    /// [`GriddedWeatherController::record_push`]: crate::gridded_weather::GriddedWeatherController::record_push
    fn push_plan_frames(&mut self, plan: &FieldPlan) -> bool {
        let keys: Vec<FrameKey> = plan
            .steps
            .iter()
            .map(|&(source, valid_time)| FrameKey {
                source,
                field: plan.field,
                valid_time,
            })
            .filter(|key| self.gridded_weather.cache.contains(key))
            .collect();
        if keys.is_empty() || !self.gridded_weather.record_push(plan.field, &keys) {
            return false;
        }
        let frames = self
            .gridded_weather
            .cache
            .frames_for(plan.field, &plan.steps);
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_weather_frames(plan.field, frames);
        }
        true
    }

    /// One fetch cycle (see the module docs). Replaces (cancels) any cycle
    /// still in flight; completed work survives in the frame cache.
    pub(super) fn run_gridded_cycle(&mut self, cx: &mut Context<Self>) {
        let fields = self.enabled_gridded_fields();
        if fields.is_empty() || self.cell.is_none() {
            return;
        }
        // Fresh window for this cycle (no-op while the anchor is fresh;
        // emits `WeatherTimeReanchored`, which schedules no follow-up).
        self.app_state
            .update(cx, |state, cx| state.re_anchor_weather_time(cx));
        let weather_time = self.app_state.read(cx).weather_time;
        let window = {
            let (start, end) = weather_time.range();
            (start.timestamp(), end.timestamp())
        };
        let selected = weather_time.selected().timestamp();
        let icon = Arc::clone(&self.gridded_weather.icon);
        let radar = Arc::clone(&self.gridded_weather.radar);
        tracing::debug!(
            ?fields,
            selected,
            anchor = weather_time.anchor().timestamp(),
            "gridded weather cycle"
        );

        self.gridded_weather.cycle_task = Some(cx.spawn(async move |this, cx| {
            // 1. Timelines → per-field plans.
            let mut plans: Vec<FieldPlan> = Vec::new();
            for field in fields {
                let icon_steps = timeline_steps(&this, cx, GridSource::Icon, field, &icon).await;
                let radar_steps = if field == GriddedField::PrecipRate {
                    timeline_steps(&this, cx, GridSource::Radar, field, &radar).await
                } else {
                    Vec::new()
                };
                if icon_steps.is_empty() && radar_steps.is_empty() {
                    continue; // logged inside; retry next cycle
                }
                plans.push(plan::plan_field(
                    field,
                    &radar_steps,
                    &icon_steps,
                    window,
                    selected,
                ));
            }

            // 2. Whatever the cache already holds shows up instantly (e.g.
            // a layer toggled off and on again). `push_plan_frames` skips
            // fields whose working set the renderer already holds.
            let pushed = this.update(cx, |this, cx| {
                let mut any = false;
                for plan in &plans {
                    any |= this.push_plan_frames(plan);
                }
                if any {
                    cx.notify();
                }
            });
            if pushed.is_err() {
                return; // view dropped
            }

            // 3. Fetch every missing frame of the window, brackets of all
            // fields first, then outward (the interleaved order), with up
            // to FETCH_CONCURRENCY downloads in flight. Tokio tasks run
            // eagerly, so spawning ahead of the await gives real
            // parallelism; results are applied in fetch order, which keeps
            // the first paint (the bracket pair) first.
            let queue = plan::interleave_fetches(&plans);
            let mut pending = queue.into_iter();
            let mut in_flight: VecDeque<(usize, FrameKey, FetchTask)> = VecDeque::new();
            loop {
                // Top the window up with the next uncached steps.
                while in_flight.len() < FETCH_CONCURRENCY {
                    let Some((idx, (source, valid_time))) = pending.next() else {
                        break;
                    };
                    let key = FrameKey {
                        source,
                        field: plans[idx].field,
                        valid_time,
                    };
                    match this.update(cx, |this, _| this.gridded_weather.cache.contains(&key)) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(_) => return,
                    }
                    let provider = match source {
                        GridSource::Radar => Arc::clone(&radar),
                        GridSource::Icon => Arc::clone(&icon),
                    };
                    let Some(task) = spawn_frame_fetch(&this, cx, provider, &key) else {
                        continue; // unrepresentable time; logged inside
                    };
                    in_flight.push_back((idx, key, task));
                }
                let Some((idx, key, task)) = in_flight.pop_front() else {
                    break; // nothing pending, nothing in flight: done
                };
                let frame = match task.await {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(
                            field = ?key.field,
                            source = ?key.source,
                            valid_time = key.valid_time,
                            %err,
                            "gridded weather fetch failed"
                        );
                        continue; // retried next cycle
                    }
                };
                let applied = this.update(cx, |this, cx| {
                    this.gridded_weather.cache.insert(key, frame, selected);
                    if this.push_plan_frames(&plans[idx]) {
                        tracing::debug!(
                            field = ?key.field,
                            source = ?key.source,
                            valid_time = key.valid_time,
                            cached = this.gridded_weather.cache.len(),
                            "weather frame fetched"
                        );
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    return;
                }
            }
        }));
    }
}

/// A frame download running on the tokio bridge.
type FetchTask = Task<anyhow::Result<WeatherGridFrame>>;

/// The advertised valid times (unix seconds, ascending) for (source,
/// field), from the TTL cache or a fresh provider probe. Empty on error
/// (logged; the cycle skips the source and retries next time).
async fn timeline_steps(
    this: &WeakEntity<MapView>,
    cx: &mut AsyncApp,
    source: GridSource,
    field: GriddedField,
    provider: &Arc<dyn GriddedWeatherProvider>,
) -> Vec<i64> {
    let cached = this.update(cx, |this, _| {
        this.gridded_weather.fresh_timeline(source, field)
    });
    let timeline = match cached {
        Ok(Some(timeline)) => Some(timeline),
        Ok(None) => fetch_timeline(this, cx, source, field, provider).await,
        Err(_) => None, // view dropped
    };
    timeline
        .map(|timeline| {
            timeline
                .steps
                .iter()
                .map(|step| step.valid_time.timestamp())
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_timeline(
    this: &WeakEntity<MapView>,
    cx: &mut AsyncApp,
    source: GridSource,
    field: GriddedField,
    provider: &Arc<dyn GriddedWeatherProvider>,
) -> Option<Arc<GriddedTimeline>> {
    let weather_field = convert::weather_field(field);
    let provider = Arc::clone(provider);
    let task = this
        .update(cx, |_, cx| {
            Tokio::spawn_result(cx, async move {
                anyhow::Ok(provider.timeline(weather_field).await?)
            })
        })
        .ok()?;
    match task.await {
        Ok(timeline) => {
            tracing::debug!(
                ?source,
                ?field,
                steps = timeline.steps.len(),
                "timeline fetched"
            );
            let timeline = Arc::new(timeline);
            this.update(cx, |this, _| {
                this.gridded_weather
                    .store_timeline(source, field, Arc::clone(&timeline));
            })
            .ok();
            Some(timeline)
        }
        Err(err) => {
            tracing::warn!(?source, ?field, %err, "gridded weather timeline failed");
            None
        }
    }
}

/// Spawn one frame download + decode + conversion on the tokio bridge.
/// The tokio task starts immediately; awaiting the returned [`FetchTask`]
/// only collects the result, so several can run concurrently. `None` when
/// the valid time is unrepresentable or the view is gone.
fn spawn_frame_fetch(
    this: &WeakEntity<MapView>,
    cx: &mut AsyncApp,
    provider: Arc<dyn GriddedWeatherProvider>,
    key: &FrameKey,
) -> Option<FetchTask> {
    let field = key.field;
    let weather_field = convert::weather_field(field);
    let Some(valid) = DateTime::from_timestamp(key.valid_time, 0) else {
        tracing::warn!(?key, "unrepresentable frame valid time skipped");
        return None;
    };
    this.update(cx, |_, cx| {
        Tokio::spawn_result(cx, async move {
            let grid = provider.fetch(weather_field, valid).await?;
            // Conversion (a value memcopy) stays off the UI thread.
            anyhow::Ok(convert::frame_from_grid(field, &grid))
        })
    })
    .ok()
}

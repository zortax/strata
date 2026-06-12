//! In-process ingestion orchestration: drives the `strata-ingest` library
//! runner on the tokio bridge, maps its [`IngestEvent`] stream into the
//! progress-panel view-model, and enforces one-run-at-a-time.
//!
//! Layering:
//! - pure decision logic ([`plan_from_needs`], [`sanitize_plan`],
//!   [`auto_ingest_decision`]) — unit-tested, no IO;
//! - pure event mapping ([`IngestEventMapper`] →
//!   [`IngestProgressVm`](super::ingest_progress::IngestProgressVm));
//! - the [`IngestManager`] run slot (token + task + last result);
//! - `impl AppState` glue that wires all of it to gpui tasks and
//!   [`Tokio::spawn`]. User-facing outcomes surface as
//!   [`AppStateEvent::IngestNotice`] — the root view turns them into window
//!   notifications (state has no `Window`).

use std::rc::Rc;
use std::time::Duration;

use gpui::{AppContext as _, Context, SharedString, Task};
use gpui_tokio::Tokio;
use strata_data::domain::Country;
use strata_ingest::{
    AeroNeed, AllOptions, BasemapNeed, CancellationToken, ElevationNeed, EventSink, IngestEvent,
    IngestJob, IngestNeeds, Ingestion, TerrainNeed, error_chain,
};

use super::ingest_progress::IngestProgressVm;
use super::{AppState, AppStateEvent};

/// Grace period between app start and the auto-triggered ingest run, so the
/// window is up and the startup feeds are not competing for IO.
const AUTO_INGEST_DELAY: Duration = Duration::from_millis(800);

/// How long the finished/cancelled/failed state lingers in the progress
/// panel before it dismisses itself.
const FINISHED_LINGER: Duration = Duration::from_millis(1500);

/// Notice shown when aero data is needed but no openAIP key is available.
/// NEVER log or echo the key itself.
const MISSING_KEY_NOTICE: &str = "No openAIP API key configured — airspace data cannot be downloaded. \
     Set the key in Settings (or OPENAIP_API_KEY in .env).";

/// Notice shown when a manual download is requested with the
/// enabled-country set empty (a legal state — nothing is kept current).
const NO_COUNTRIES_NOTICE: &str = "No countries enabled — enable at least one in Settings → Countries \
     to download data.";

/// One of the three independently runnable dataset families.
// Constructed by the settings modal (next phase) via
// `AppState::run_ingest_dataset`; until then only matched on.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestDataset {
    /// The five openAIP datasets (airspaces, airports, navaids, reporting
    /// points, obstacles) — ingested as one unit; needs the API key.
    Aero,
    /// Protomaps vector basemap extract.
    Basemap,
    /// Copernicus DEM hillshade tiles.
    Terrain,
}

/// Which dataset families one run executes, in the fixed order
/// aero → basemap → terrain → elevation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngestPlan {
    pub aero: bool,
    pub basemap: bool,
    pub terrain: bool,
    /// The elevation-grid **backfill** stage only: a terrain run writes the
    /// elevation grid as part of itself, so this is planned exclusively
    /// when elevation is missing while terrain is *not* being run (existing
    /// installs — it pools from the warm dem-cache in seconds).
    pub elevation: bool,
}

impl IngestPlan {
    pub const FULL: Self = Self {
        aero: true,
        basemap: true,
        terrain: true,
        // The terrain stage pools the elevation grid itself; the separate
        // backfill stage would be redundant work.
        elevation: false,
    };

    pub fn single(dataset: IngestDataset) -> Self {
        let mut plan = Self::default();
        match dataset {
            IngestDataset::Aero => plan.aero = true,
            IngestDataset::Basemap => plan.basemap = true,
            IngestDataset::Terrain => plan.terrain = true,
        }
        plan
    }

    pub fn is_empty(self) -> bool {
        !(self.aero || self.basemap || self.terrain || self.elevation)
    }

    /// Human enumeration of the planned parts
    /// ("aero, basemap, terrain, elevation").
    pub fn describe(self) -> String {
        let mut parts = Vec::new();
        if self.aero {
            parts.push("aero");
        }
        if self.basemap {
            parts.push("basemap");
        }
        if self.terrain {
            parts.push("terrain");
        }
        if self.elevation {
            parts.push("elevation");
        }
        parts.join(", ")
    }
}

/// What the needed-data inspection decided (spec: aero `Missing|Stale` →
/// aero, basemap `Missing` → basemap, terrain `Missing` → terrain; a
/// `Partial` terrain run is not auto-restarted; elevation
/// `Missing|Partial` → the elevation backfill (the pass is cheap and
/// idempotent — a coverage gap is always worth rerunning), **unless** a
/// terrain run is planned anyway — terrain writes the elevation grid
/// itself).
pub fn plan_from_needs(needs: &IngestNeeds) -> IngestPlan {
    let terrain = matches!(needs.terrain, TerrainNeed::Missing);
    IngestPlan {
        aero: !matches!(needs.aero, AeroNeed::Current(_)),
        basemap: matches!(needs.basemap, BasemapNeed::Missing),
        terrain,
        elevation: !matches!(needs.elevation, ElevationNeed::Present { .. }) && !terrain,
    }
}

/// Drops the aero part when no openAIP API key is available (the other
/// parts run keyless). The flag reports that aero was dropped — the caller
/// surfaces the "set the key in Settings" notice instead of failing
/// silently.
pub fn sanitize_plan(mut plan: IngestPlan, key_present: bool) -> (IngestPlan, bool) {
    let dropped_aero = plan.aero && !key_present;
    plan.aero &= key_present;
    (plan, dropped_aero)
}

/// Outcome of the startup auto-trigger decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoIngestDecision {
    /// What to run (possibly empty — nothing needed or key missing).
    pub plan: IngestPlan,
    /// Aero data is needed but no API key is configured: notify the user,
    /// pointing at Settings.
    pub missing_api_key: bool,
}

/// The full startup decision: `config.ingest.auto` gates everything, then
/// [`plan_from_needs`] + [`sanitize_plan`]. Mirrors exactly what
/// [`AppState::start_auto_ingest`] executes.
pub fn auto_ingest_decision(
    needs: &IngestNeeds,
    auto: bool,
    key_present: bool,
) -> AutoIngestDecision {
    if !auto {
        return AutoIngestDecision {
            plan: IngestPlan::default(),
            missing_api_key: false,
        };
    }
    let (plan, missing_api_key) = sanitize_plan(plan_from_needs(needs), key_present);
    AutoIngestDecision {
        plan,
        missing_api_key,
    }
}

// --- notices ----------------------------------------------------------------

/// Severity of an [`IngestNotice`]; the root view maps it to a
/// gpui-component notification type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A user-facing message about ingestion (run finished/failed, key missing,
/// run rejected). Carried by [`AppStateEvent::IngestNotice`]; the root view
/// shows it as a window notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestNotice {
    pub level: NoticeLevel,
    pub message: SharedString,
}

// --- run bookkeeping ---------------------------------------------------------

/// How a finished run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestRunOutcome {
    Success,
    Cancelled,
    /// Flattened error chain of the failing step.
    Failed(String),
}

/// Result of the most recent ingest run (for the settings modal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestRunResult {
    pub plan: IngestPlan,
    pub outcome: IngestRunOutcome,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

impl IngestRunResult {
    /// One-line human summary ("Data download complete (aero, basemap)").
    pub fn message(&self) -> String {
        match &self.outcome {
            IngestRunOutcome::Success => {
                format!("Data download complete ({}).", self.plan.describe())
            }
            IngestRunOutcome::Cancelled => "Data download cancelled.".to_string(),
            IngestRunOutcome::Failed(error) => format!("Data download failed: {error}"),
        }
    }
}

/// The single-run-at-a-time slot plus the last finished result. Pure state
/// (testable without gpui) — the gpui task is attached separately and only
/// kept so dropping `AppState` aborts the run.
#[derive(Default)]
pub struct IngestManager {
    active: Option<ActiveRun>,
    /// The just-finished run's consumer task, parked because [`Self::finish`]
    /// runs *inside* that task — dropping it there would cancel its own
    /// linger/dismiss tail. Dropped (long completed) when the next run
    /// finishes.
    retired_task: Option<Task<()>>,
    last_result: Option<IngestRunResult>,
}

struct ActiveRun {
    cancel: CancellationToken,
    /// Consumer task driving events → VM and the final wrap-up; holds the
    /// tokio-bridge task alive (dropping it aborts the run).
    task: Option<Task<()>>,
}

impl IngestManager {
    /// Claims the run slot: returns the fresh run's cancellation token, or
    /// `None` while another run is active.
    pub fn try_begin(&mut self) -> Option<CancellationToken> {
        if self.active.is_some() {
            return None;
        }
        let cancel = CancellationToken::new();
        self.active = Some(ActiveRun {
            cancel: cancel.clone(),
            task: None,
        });
        Some(cancel)
    }

    /// Attaches the consumer task to the active run (keeps it alive).
    fn attach_task(&mut self, task: Task<()>) {
        if let Some(active) = &mut self.active {
            active.task = Some(task);
        }
    }

    /// Releases the run slot and records the result. The run's task is
    /// parked, not dropped — `finish` is called from within it.
    pub fn finish(&mut self, result: IngestRunResult) {
        if let Some(active) = self.active.take() {
            self.retired_task = active.task;
        }
        self.last_result = Some(result);
    }

    /// Cancels the active run; `false` when idle.
    pub fn cancel(&self) -> bool {
        match &self.active {
            Some(active) => {
                active.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    pub fn last_result(&self) -> Option<&IngestRunResult> {
        self.last_result.as_ref()
    }
}

// --- event → view-model mapping ----------------------------------------------

/// Maps the runner's [`IngestEvent`] stream onto the progress-panel
/// view-model: every `JobStarted` claims a VM row, progress/terminal events
/// are forwarded to it. A multi-country run starts the *same* [`IngestJob`]
/// once per country (sequentially); each `JobStarted` claims a fresh row
/// and later events for that job address the latest one. Pure — drives
/// only [`IngestProgressVm`] mutations.
#[derive(Default)]
pub struct IngestEventMapper {
    rows: std::collections::HashMap<IngestJob, usize>,
}

/// Panel row label for a job. The runner's event label carries the country
/// ("airspaces DE", "terrain AT"); render it as "openAIP airspaces (DE)" /
/// "terrain (AT)". Labels without a country suffix (single-pass bbox
/// overrides) pass through unchanged.
fn row_label(job: IngestJob, event_label: &str) -> String {
    let (base, country) = split_country_suffix(event_label);
    let base = match job {
        IngestJob::Basemap | IngestJob::Terrain | IngestJob::Elevation => base.to_string(),
        _aero => format!("openAIP {base}"),
    };
    match country {
        Some(code) => format!("{base} ({code})"),
        None => base,
    }
}

/// Splits a trailing country code off a job label ("airspaces DE" →
/// ("airspaces", Some("DE"))). Only an exact uppercase alpha-2 code of a
/// supported country counts — anything else stays part of the label.
fn split_country_suffix(label: &str) -> (&str, Option<&str>) {
    if let Some((base, suffix)) = label.rsplit_once(' ')
        && suffix.len() == 2
        && suffix.chars().all(|c| c.is_ascii_uppercase())
        && Country::from_code(suffix).is_some()
    {
        return (base, Some(suffix));
    }
    (label, None)
}

impl IngestEventMapper {
    pub fn apply(&mut self, vm: &mut IngestProgressVm, event: &IngestEvent) {
        match event {
            IngestEvent::JobStarted { job, label } => {
                let row = vm.job_started(row_label(*job, label), "starting…", None);
                // Overwrites any previous country's row for this job —
                // events of one job are sequential, so the latest row is
                // always the live one.
                self.rows.insert(*job, row);
            }
            IngestEvent::Progress {
                job,
                done,
                total,
                detail,
            } => {
                if let Some(&row) = self.rows.get(job) {
                    vm.job_total(row, *total);
                    vm.job_progress(row, *done, detail.clone());
                }
            }
            IngestEvent::JobFinished { job, summary } => {
                if let Some(&row) = self.rows.get(job) {
                    let done = vm.jobs.get(row).map(|j| j.done).unwrap_or_default();
                    vm.job_progress(row, done, summary.clone());
                    vm.job_done(row);
                }
            }
            IngestEvent::JobFailed { job, error } => {
                if let Some(&row) = self.rows.get(job) {
                    vm.job_failed(row, error.clone());
                }
            }
            // The run wrap-up is driven by the channel closing (the runner
            // and its sink drop), not by RunFinished: a multi-step run
            // emits one RunFinished per entry point.
            IngestEvent::RunFinished => {}
        }
    }
}

// --- orchestration -----------------------------------------------------------

/// Runs the planned steps sequentially on the library runner, stopping at
/// the first error (cancellation surfaces as `IngestError::Cancelled` from
/// the step's pre-flight check). Order: aero → basemap → terrain →
/// elevation (the backfill never coexists with terrain in one plan, see
/// [`plan_from_needs`]).
async fn run_plan(
    runner: &Ingestion,
    plan: IngestPlan,
    basemap_maxzoom: u8,
) -> Result<(), strata_ingest::IngestError> {
    let defaults = AllOptions::default();
    if plan.aero {
        runner.aero().await?;
    }
    if plan.basemap {
        runner.basemap(basemap_maxzoom).await?;
    }
    if plan.terrain {
        runner
            .terrain(defaults.terrain_minzoom, defaults.terrain_maxzoom)
            .await?;
    }
    if plan.elevation {
        runner.elevation().await?;
    }
    Ok(())
}

/// Manual trigger API — consumed by the settings modal (next phase); the
/// banner button and auto-trigger only exercise a subset today.
#[allow(dead_code)]
impl AppState {
    /// Starts a full aero + basemap + terrain run. Returns whether a run was
    /// started (`false`: already running, or nothing left after the key
    /// check — both surfaced as an [`AppStateEvent::IngestNotice`]).
    pub fn run_full_ingest(&mut self, cx: &mut Context<Self>) -> bool {
        self.start_ingest(IngestPlan::FULL, cx)
    }

    /// Starts a single-dataset run.
    pub fn run_ingest_dataset(&mut self, dataset: IngestDataset, cx: &mut Context<Self>) -> bool {
        self.start_ingest(IngestPlan::single(dataset), cx)
    }

    /// Cancels the running ingest, if any.
    pub fn cancel_ingest(&mut self) -> bool {
        self.ingest.cancel()
    }

    /// Outcome of the most recent finished run.
    pub fn last_ingest_result(&self) -> Option<&IngestRunResult> {
        self.ingest.last_result()
    }
}

impl AppState {
    /// Inspects the data dir in the background and ingests whatever is
    /// missing or stale ([`plan_from_needs`]). Notifies "up to date" when
    /// nothing is needed (manual trigger — silence would look broken).
    pub fn run_needed_ingest(&mut self, cx: &mut Context<Self>) {
        self.spawn_inspect_then_ingest(false, cx);
    }

    pub fn ingest_running(&self) -> bool {
        self.ingest.is_running()
    }

    // --- startup auto-trigger ----------------------------------------------

    /// Auto-ingest on startup (gated on `config.ingest.auto`): after a
    /// short grace period, inspect the data dir and run whatever is needed.
    pub(super) fn start_auto_ingest(&mut self, cx: &mut Context<Self>) {
        if !self.config.ingest.auto {
            return;
        }
        self.spawn_inspect_then_ingest(true, cx);
    }

    /// Shared inspect → plan → start pipeline of the auto trigger, the
    /// countries-changed trigger ([`AppState::set_enabled_countries`], the
    /// `auto` flavor) and the manual "download what's missing" entry
    /// points. The inspection covers the enabled-country set: a freshly
    /// enabled country flips the aggregated needs to missing until it is
    /// ingested, so the plan covers it per (dataset, country).
    pub(super) fn spawn_inspect_then_ingest(&mut self, auto: bool, cx: &mut Context<Self>) {
        let data_dir = self.data_dir.clone();
        let countries = self.config.enabled_countries();
        // The empty selection is legal and means "keep nothing current":
        // skip instead of inspecting (the no-country aggregate reports
        // missing and would schedule pointless empty runs). Existing data
        // stays on disk; the app keeps running on it.
        if countries.is_empty() {
            if auto {
                tracing::info!("auto-ingest skipped: no countries enabled");
            } else {
                self.emit_ingest_notice(NoticeLevel::Info, NO_COUNTRIES_NOTICE, cx);
            }
            return;
        }
        cx.spawn(async move |this, cx| {
            if auto {
                cx.background_executor().timer(AUTO_INGEST_DELAY).await;
            }
            // Read-only meta lookups + one COUNT — still IO, keep it off
            // the UI thread.
            let needs = {
                let countries = countries.clone();
                cx.background_spawn(async move { strata_ingest::inspect(&data_dir, &countries) })
                    .await
            };
            this.update(cx, |this, cx| {
                if auto {
                    // The decision (incl. the key check) is taken here so the
                    // missing-key notice fires even when nothing else runs;
                    // `start_ingest` re-sanitizing the already keyless plan
                    // is a no-op, so the notice cannot double up.
                    let key_present = this.config.openaip_api_key().is_some();
                    let decision = auto_ingest_decision(&needs, true, key_present);
                    if decision.missing_api_key {
                        this.emit_ingest_notice(NoticeLevel::Warning, MISSING_KEY_NOTICE, cx);
                    }
                    if decision.plan.is_empty() {
                        tracing::info!(?countries, "auto-ingest: nothing to do");
                        return;
                    }
                    tracing::info!(plan = ?decision.plan, ?countries, "auto-ingest starting");
                    this.start_ingest(decision.plan, cx);
                } else {
                    let plan = plan_from_needs(&needs);
                    if plan.is_empty() {
                        this.emit_ingest_notice(
                            NoticeLevel::Info,
                            "All data is already up to date.",
                            cx,
                        );
                        return;
                    }
                    tracing::info!(?plan, "manual ingest of missing/stale data");
                    this.start_ingest(plan, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // --- run plumbing -------------------------------------------------------

    /// Emits an [`AppStateEvent::IngestNotice`]; the root view renders it
    /// as a window notification.
    fn emit_ingest_notice(
        &mut self,
        level: NoticeLevel,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        cx.emit(AppStateEvent::IngestNotice(IngestNotice {
            level,
            message: message.into(),
        }));
    }

    /// Starts `plan` (after the API-key check) unless a run is already in
    /// flight. The run executes on the tokio bridge; a gpui task consumes
    /// the event stream into the progress panel and wraps up (data reload,
    /// notice, linger, dismiss) when the stream closes.
    fn start_ingest(&mut self, plan: IngestPlan, cx: &mut Context<Self>) -> bool {
        // Covers the direct manual entry points (`run_full_ingest`,
        // `run_ingest_dataset`); the inspect pipeline already returned
        // early. A run over zero countries would do nothing but flash the
        // progress panel.
        if self.config.enabled_countries().is_empty() {
            self.emit_ingest_notice(NoticeLevel::Info, NO_COUNTRIES_NOTICE, cx);
            return false;
        }
        let api_key = self.config.openaip_api_key();
        let (plan, dropped_aero) = sanitize_plan(plan, api_key.is_some());
        if dropped_aero {
            self.emit_ingest_notice(NoticeLevel::Warning, MISSING_KEY_NOTICE, cx);
        }
        if plan.is_empty() {
            return false;
        }
        let Some(cancel) = self.ingest.try_begin() else {
            self.emit_ingest_notice(
                NoticeLevel::Info,
                "A data download is already in progress.",
                cx,
            );
            return false;
        };

        // The run executes over the enabled-country set (the runner loops
        // countries internally and writes per-country completion meta).
        let mut config = strata_ingest::IngestConfig::new(
            self.data_dir.clone(),
            self.config.enabled_countries(),
        );
        config.openaip_api_key = api_key;
        let basemap_maxzoom = self.config.ingest.basemap_maxzoom;

        let (sink, mut events) = EventSink::channel();
        // The runner lives inside the tokio future; when it returns, the
        // runner (and with it the sink) drops, closing the event stream —
        // that close is the consumer's end-of-run signal (robust against
        // the one-RunFinished-per-entry-point stream shape).
        let run = Tokio::spawn(cx, {
            let cancel = cancel.clone();
            async move {
                let runner = Ingestion::new(config, sink, cancel);
                run_plan(&runner, plan, basemap_maxzoom).await
            }
        });

        // The panel's ✕ cancels the token; the run unwinds through the
        // normal completion path.
        self.update_ingest_progress(cx, |vm| {
            let cancel = cancel.clone();
            vm.on_cancel = Some(Rc::new(move || cancel.cancel()));
        });

        let task = cx.spawn(async move |this, cx| {
            let mut mapper = IngestEventMapper::default();
            while let Some(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    this.update_ingest_progress(cx, |vm| mapper.apply(vm, &event));
                });
                if alive.is_err() {
                    return; // app state dropped; `run` drops with us → abort
                }
            }

            let outcome = match run.await {
                Ok(Ok(())) => IngestRunOutcome::Success,
                Ok(Err(err)) if err.is_cancelled() => IngestRunOutcome::Cancelled,
                Ok(Err(err)) => IngestRunOutcome::Failed(error_chain(&err)),
                Err(join_err) => IngestRunOutcome::Failed(format!("ingest task: {join_err}")),
            };
            let result = IngestRunResult {
                plan,
                outcome,
                finished_at: chrono::Utc::now(),
            };

            if this
                .update(cx, |this, cx| {
                    let level = match &result.outcome {
                        IngestRunOutcome::Success => NoticeLevel::Success,
                        IngestRunOutcome::Cancelled => NoticeLevel::Info,
                        IngestRunOutcome::Failed(_) => NoticeLevel::Error,
                    };
                    let message = result.message();
                    tracing::info!(outcome = ?result.outcome, "ingest run finished");
                    this.ingest.finish(result);
                    this.update_ingest_progress(cx, |vm| vm.on_cancel = None);
                    // Pick up what the run produced (store/basemap/meta →
                    // DataReloaded fan-out: snapshots, warm feed, sources).
                    this.refresh_data(cx);
                    this.emit_ingest_notice(level, message, cx);
                })
                .is_err()
            {
                return;
            }

            // Let the final state linger, then hide the panel — unless a
            // new run claimed it meanwhile (its own wrap-up will dismiss).
            cx.background_executor().timer(FINISHED_LINGER).await;
            this.update(cx, |this, cx| {
                if !this.ingest.is_running() {
                    this.update_ingest_progress(cx, |vm| vm.dismiss());
                }
            })
            .ok();
        });
        self.ingest.attach_task(task);
        true
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use strata_data::domain::AiracCycle;
    use strata_ingest::{AiracInfo, CountryNeeds};

    use super::*;

    fn airac_info() -> AiracInfo {
        AiracInfo {
            cycle: AiracCycle::current(),
            ingested_at: Utc::now(),
        }
    }

    fn needs(aero: AeroNeed, basemap: BasemapNeed, terrain: TerrainNeed) -> IngestNeeds {
        IngestNeeds {
            aero,
            basemap,
            terrain,
            // Neutral "present" keeps the aero/basemap/terrain decision
            // tests focused; the elevation matrix has its own test below.
            elevation: ElevationNeed::Present { tiles: 1 },
            // The plan logic consumes only the aggregated fields.
            countries: Vec::new(),
        }
    }

    #[test]
    fn plan_covers_missing_and_stale_aero_but_not_current() {
        let all_missing = needs(
            AeroNeed::Missing,
            BasemapNeed::Missing,
            TerrainNeed::Missing,
        );
        assert_eq!(plan_from_needs(&all_missing), IngestPlan::FULL);

        let stale = needs(
            AeroNeed::Stale(airac_info()),
            BasemapNeed::Present { maxzoom: Some(13) },
            TerrainNeed::Present { tiles: 100 },
        );
        assert_eq!(
            plan_from_needs(&stale),
            IngestPlan {
                aero: true,
                ..IngestPlan::default()
            }
        );

        let current = needs(
            AeroNeed::Current(airac_info()),
            BasemapNeed::Present { maxzoom: None },
            TerrainNeed::Present { tiles: 1 },
        );
        assert!(plan_from_needs(&current).is_empty());
    }

    #[test]
    fn partial_terrain_is_not_auto_restarted() {
        let partial = needs(
            AeroNeed::Current(airac_info()),
            BasemapNeed::Present { maxzoom: Some(13) },
            TerrainNeed::Partial { tiles: 42 },
        );
        assert!(plan_from_needs(&partial).is_empty());
    }

    /// The elevation backfill (gate follow-up): a missing elevation grid
    /// triggers the elevation stage — except when a terrain run is planned
    /// anyway, because terrain writes the grid itself.
    #[test]
    fn missing_elevation_plans_the_backfill_unless_terrain_runs() {
        let current = |elevation| IngestNeeds {
            aero: AeroNeed::Current(airac_info()),
            basemap: BasemapNeed::Present { maxzoom: Some(13) },
            terrain: TerrainNeed::Present { tiles: 100 },
            elevation,
            countries: Vec::new(),
        };

        // Existing install (terrain present, elevation missing — the
        // pre-elevation-schema case): backfill only.
        let plan = plan_from_needs(&current(ElevationNeed::Missing));
        assert_eq!(
            plan,
            IngestPlan {
                aero: false,
                basemap: false,
                terrain: false,
                elevation: true
            }
        );
        assert!(!plan.is_empty());
        assert_eq!(plan.describe(), "elevation");

        // Elevation present: nothing.
        assert!(plan_from_needs(&current(ElevationNeed::Present { tiles: 486 })).is_empty());

        // Partial coverage (a bbox-limited smoke run wrote the meta row but
        // only a handful of tiles): run-it, same as missing.
        let plan = plan_from_needs(&current(ElevationNeed::Partial { tiles: 12 }));
        assert!(plan.elevation && !plan.terrain);
        assert_eq!(plan.describe(), "elevation");

        // Fresh install: terrain runs and pools elevation itself — no
        // redundant backfill stage.
        let fresh = IngestNeeds {
            aero: AeroNeed::Missing,
            basemap: BasemapNeed::Missing,
            terrain: TerrainNeed::Missing,
            elevation: ElevationNeed::Missing,
            countries: Vec::new(),
        };
        let plan = plan_from_needs(&fresh);
        assert_eq!(plan, IngestPlan::FULL);
        assert!(plan.terrain && !plan.elevation);

        // Partial terrain is not auto-restarted, but the elevation
        // backfill still runs (it reuses the warm dem-cache).
        let partial = IngestNeeds {
            aero: AeroNeed::Current(airac_info()),
            basemap: BasemapNeed::Present { maxzoom: Some(13) },
            terrain: TerrainNeed::Partial { tiles: 42 },
            elevation: ElevationNeed::Missing,
            countries: Vec::new(),
        };
        let plan = plan_from_needs(&partial);
        assert!(!plan.terrain && plan.elevation);

        // The keyless sanitizer never touches the elevation stage.
        let (sanitized, dropped) = sanitize_plan(plan, false);
        assert!(!dropped);
        assert!(sanitized.elevation);

        // And the startup auto-trigger passes the backfill through.
        let decision = auto_ingest_decision(&partial, true, true);
        assert!(decision.plan.elevation);
        assert!(auto_ingest_decision(&partial, false, true).plan.is_empty());
    }

    /// The per-country decision matrix. `inspect` aggregates the
    /// per-country needs into the four flat fields as the worst case
    /// across the *requested* (= enabled) countries — the aggregation
    /// itself is strata-ingest's, tested there; this pins what the app
    /// plans for the documented aggregate shapes: enabling a country with
    /// no data flips the aggregates to missing until it's ingested, a
    /// fully-current set plans nothing, and one country's stale AIRAC
    /// re-runs aero alone.
    #[test]
    fn plan_covers_missing_and_stale_per_country() {
        let current = |country| CountryNeeds {
            country,
            aero: AeroNeed::Current(airac_info()),
            basemap: BasemapNeed::Present { maxzoom: Some(13) },
            terrain: TerrainNeed::Present { tiles: 100 },
            elevation: ElevationNeed::Present { tiles: 10 },
        };
        let missing = |country| CountryNeeds {
            country,
            aero: AeroNeed::Missing,
            basemap: BasemapNeed::Missing,
            terrain: TerrainNeed::Missing,
            elevation: ElevationNeed::Missing,
        };

        // Germany alone, fully ingested (the migrated 5 GB store): nothing.
        let de_only = IngestNeeds {
            aero: AeroNeed::Current(airac_info()),
            basemap: BasemapNeed::Present { maxzoom: Some(13) },
            terrain: TerrainNeed::Present { tiles: 100 },
            elevation: ElevationNeed::Present { tiles: 10 },
            countries: vec![current(Country::DE)],
        };
        assert!(plan_from_needs(&de_only).is_empty());

        // Austria freshly enabled on top: every aggregate goes missing
        // until AT is ingested → the full run covers it (the runner loops
        // the enabled countries and writes per-country completion meta).
        let at_enabled = IngestNeeds {
            aero: AeroNeed::Missing,
            basemap: BasemapNeed::Missing,
            terrain: TerrainNeed::Missing,
            elevation: ElevationNeed::Missing,
            countries: vec![current(Country::DE), missing(Country::AT)],
        };
        assert_eq!(plan_from_needs(&at_enabled), IngestPlan::FULL);

        // Both ingested, AT's AIRAC went stale: aero re-runs, nothing else.
        let at_stale = IngestNeeds {
            aero: AeroNeed::Stale(airac_info()),
            basemap: BasemapNeed::Present { maxzoom: Some(13) },
            terrain: TerrainNeed::Present { tiles: 200 },
            elevation: ElevationNeed::Present { tiles: 20 },
            countries: vec![
                current(Country::DE),
                CountryNeeds {
                    aero: AeroNeed::Stale(airac_info()),
                    ..current(Country::AT)
                },
            ],
        };
        assert_eq!(
            plan_from_needs(&at_stale),
            IngestPlan {
                aero: true,
                ..IngestPlan::default()
            }
        );

        // Disabling AT again (only DE requested): its absence from the
        // inspection means nothing is re-planned for it — country
        // selection scopes ingestion, never the store contents.
        assert!(plan_from_needs(&de_only).is_empty());
    }

    #[test]
    fn sanitize_drops_aero_without_a_key_and_reports_it() {
        let (plan, dropped) = sanitize_plan(IngestPlan::FULL, false);
        assert!(dropped);
        assert_eq!(
            plan,
            IngestPlan {
                aero: false,
                ..IngestPlan::FULL
            }
        );

        let (plan, dropped) = sanitize_plan(IngestPlan::FULL, true);
        assert!(!dropped);
        assert_eq!(plan, IngestPlan::FULL);

        // Aero-only without a key → nothing left, but the notice fires.
        let (plan, dropped) = sanitize_plan(IngestPlan::single(IngestDataset::Aero), false);
        assert!(dropped);
        assert!(plan.is_empty());

        // No aero in the plan → key irrelevant.
        let (plan, dropped) = sanitize_plan(IngestPlan::single(IngestDataset::Basemap), false);
        assert!(!dropped);
        assert_eq!(plan, IngestPlan::single(IngestDataset::Basemap));
    }

    /// The startup decision matrix: needs × auto × key.
    #[test]
    fn auto_ingest_decision_matrix() {
        let all_missing = needs(
            AeroNeed::Missing,
            BasemapNeed::Missing,
            TerrainNeed::Missing,
        );
        let aero_only = needs(
            AeroNeed::Missing,
            BasemapNeed::Present { maxzoom: Some(13) },
            TerrainNeed::Present { tiles: 7 },
        );
        let all_current = needs(
            AeroNeed::Current(airac_info()),
            BasemapNeed::Present { maxzoom: Some(13) },
            TerrainNeed::Present { tiles: 7 },
        );

        // auto off → never anything, never a notice.
        for n in [&all_missing, &aero_only, &all_current] {
            for key in [false, true] {
                let d = auto_ingest_decision(n, false, key);
                assert!(d.plan.is_empty());
                assert!(!d.missing_api_key);
            }
        }

        // auto on, key present → run exactly what's needed.
        let d = auto_ingest_decision(&all_missing, true, true);
        assert_eq!(d.plan, IngestPlan::FULL);
        assert!(!d.missing_api_key);
        let d = auto_ingest_decision(&aero_only, true, true);
        assert_eq!(d.plan, IngestPlan::single(IngestDataset::Aero));
        let d = auto_ingest_decision(&all_current, true, true);
        assert!(d.plan.is_empty());
        assert!(!d.missing_api_key);

        // auto on, no key → keyless parts still run, missing key reported
        // when aero was needed.
        let d = auto_ingest_decision(&all_missing, true, false);
        assert_eq!(
            d.plan,
            IngestPlan {
                aero: false,
                ..IngestPlan::FULL
            }
        );
        assert!(d.missing_api_key);
        let d = auto_ingest_decision(&aero_only, true, false);
        assert!(d.plan.is_empty());
        assert!(d.missing_api_key, "must not fail silently");
        let d = auto_ingest_decision(&all_current, true, false);
        assert!(d.plan.is_empty());
        assert!(!d.missing_api_key, "nothing needed → no key nag");
    }

    #[test]
    fn manager_allows_one_run_at_a_time() {
        let mut manager = IngestManager::default();
        assert!(!manager.is_running());
        assert!(!manager.cancel(), "cancel while idle is a no-op");

        let token = manager.try_begin().expect("slot free");
        assert!(manager.is_running());
        assert!(manager.try_begin().is_none(), "second run rejected");

        assert!(manager.cancel());
        assert!(token.is_cancelled(), "cancel reaches the run's token");
        // Cancelled but not yet finished: the slot stays claimed until the
        // run unwinds.
        assert!(manager.is_running());

        let result = IngestRunResult {
            plan: IngestPlan::FULL,
            outcome: IngestRunOutcome::Cancelled,
            finished_at: Utc::now(),
        };
        manager.finish(result.clone());
        assert!(!manager.is_running());
        assert_eq!(manager.last_result(), Some(&result));
        assert!(manager.try_begin().is_some(), "slot reusable after finish");
    }

    /// `finish` runs inside the run's own consumer task; dropping that task
    /// there would cancel its linger/dismiss tail. It must be parked.
    #[test]
    fn finish_parks_the_runs_task_instead_of_dropping_it() {
        let mut manager = IngestManager::default();
        manager.try_begin().expect("slot free");
        manager.attach_task(Task::ready(()));
        manager.finish(IngestRunResult {
            plan: IngestPlan::FULL,
            outcome: IngestRunOutcome::Success,
            finished_at: Utc::now(),
        });
        assert!(
            manager.retired_task.is_some(),
            "the finishing task survives its own finish() call"
        );
    }

    #[test]
    fn mapper_drives_the_vm_through_a_two_job_run() {
        let mut vm = IngestProgressVm::default();
        let mut mapper = IngestEventMapper::default();

        mapper.apply(
            &mut vm,
            &IngestEvent::JobStarted {
                job: IngestJob::AeroAirspaces,
                label: "airspaces".into(),
            },
        );
        assert!(vm.visible);
        assert_eq!(vm.jobs.len(), 1);
        assert_eq!(vm.jobs[0].label.as_ref(), "openAIP airspaces");
        assert_eq!(vm.overall_fraction(), None, "spinner phase");

        mapper.apply(
            &mut vm,
            &IngestEvent::Progress {
                job: IngestJob::AeroAirspaces,
                done: 0,
                total: None,
                detail: "fetching…".into(),
            },
        );
        assert_eq!(vm.jobs[0].detail.as_ref(), "fetching…");

        mapper.apply(
            &mut vm,
            &IngestEvent::JobFinished {
                job: IngestJob::AeroAirspaces,
                summary: "1234 fetched, 1230 normalized".into(),
            },
        );
        assert_eq!(
            vm.jobs[0].state,
            super::super::ingest_progress::JobState::Done
        );
        assert_eq!(vm.jobs[0].detail.as_ref(), "1234 fetched, 1230 normalized");

        mapper.apply(
            &mut vm,
            &IngestEvent::JobStarted {
                job: IngestJob::Basemap,
                label: "basemap".into(),
            },
        );
        mapper.apply(
            &mut vm,
            &IngestEvent::Progress {
                job: IngestJob::Basemap,
                done: 50,
                total: Some(200),
                detail: "12.5 MiB written".into(),
            },
        );
        assert_eq!(vm.jobs[1].label.as_ref(), "basemap");
        assert_eq!(vm.jobs[1].done, 50);
        assert_eq!(vm.jobs[1].total, Some(200));
        assert_eq!(vm.overall_fraction(), Some((1.0 + 0.25) / 2.0));

        mapper.apply(
            &mut vm,
            &IngestEvent::JobFailed {
                job: IngestJob::Basemap,
                error: "aborted".into(),
            },
        );
        assert_eq!(
            vm.jobs[1].state,
            super::super::ingest_progress::JobState::Failed
        );
        assert_eq!(vm.jobs[1].detail.as_ref(), "aborted");
        assert!(!vm.any_running());

        // RunFinished is a no-op for the VM (wrap-up rides on channel close).
        mapper.apply(&mut vm, &IngestEvent::RunFinished);
        assert_eq!(vm.jobs.len(), 2);
    }

    /// Row labels prefer the runner's event label, which carries the
    /// country ("airspaces DE") — rendered as "openAIP airspaces (DE)".
    /// Labels without a recognizable country suffix pass through.
    #[test]
    fn row_labels_carry_the_country() {
        assert_eq!(
            row_label(IngestJob::AeroAirspaces, "airspaces DE"),
            "openAIP airspaces (DE)"
        );
        assert_eq!(
            row_label(IngestJob::AeroReportingPoints, "reporting points AT"),
            "openAIP reporting points (AT)"
        );
        assert_eq!(row_label(IngestJob::Terrain, "terrain AT"), "terrain (AT)");
        assert_eq!(row_label(IngestJob::Basemap, "basemap CH"), "basemap (CH)");
        assert_eq!(
            row_label(IngestJob::Elevation, "elevation DE"),
            "elevation (DE)"
        );

        // Country-less labels (bbox-override passes, older runners).
        assert_eq!(
            row_label(IngestJob::AeroAirspaces, "airspaces"),
            "openAIP airspaces"
        );
        assert_eq!(row_label(IngestJob::Basemap, "basemap"), "basemap");

        // Only an exact uppercase supported alpha-2 code counts as a
        // country suffix.
        assert_eq!(split_country_suffix("airspaces XX"), ("airspaces XX", None));
        assert_eq!(split_country_suffix("airspaces de"), ("airspaces de", None));
        assert_eq!(
            split_country_suffix("airspaces DE"),
            ("airspaces", Some("DE"))
        );
    }

    /// A multi-country run starts the same [`IngestJob`] once per country
    /// (sequentially): every `JobStarted` claims a fresh row and later
    /// events for the job drive the latest row.
    #[test]
    fn mapper_gives_each_country_its_own_row() {
        let mut vm = IngestProgressVm::default();
        let mut mapper = IngestEventMapper::default();

        mapper.apply(
            &mut vm,
            &IngestEvent::JobStarted {
                job: IngestJob::AeroAirspaces,
                label: "airspaces DE".into(),
            },
        );
        mapper.apply(
            &mut vm,
            &IngestEvent::JobFinished {
                job: IngestJob::AeroAirspaces,
                summary: "750 fetched".into(),
            },
        );
        mapper.apply(
            &mut vm,
            &IngestEvent::JobStarted {
                job: IngestJob::AeroAirspaces,
                label: "airspaces AT".into(),
            },
        );
        mapper.apply(
            &mut vm,
            &IngestEvent::Progress {
                job: IngestJob::AeroAirspaces,
                done: 0,
                total: None,
                detail: "fetching…".into(),
            },
        );

        assert_eq!(vm.jobs.len(), 2);
        assert_eq!(vm.jobs[0].label.as_ref(), "openAIP airspaces (DE)");
        assert_eq!(
            vm.jobs[0].state,
            super::super::ingest_progress::JobState::Done,
            "the finished DE row stays finished"
        );
        assert_eq!(vm.jobs[1].label.as_ref(), "openAIP airspaces (AT)");
        assert_eq!(
            vm.jobs[1].detail.as_ref(),
            "fetching…",
            "progress drives the latest (AT) row"
        );

        mapper.apply(
            &mut vm,
            &IngestEvent::JobStarted {
                job: IngestJob::Terrain,
                label: "terrain AT".into(),
            },
        );
        assert_eq!(vm.jobs[2].label.as_ref(), "terrain (AT)");
    }

    #[test]
    fn mapper_ignores_events_for_unknown_jobs() {
        let mut vm = IngestProgressVm::default();
        let mut mapper = IngestEventMapper::default();
        // Progress without a preceding JobStarted (cannot happen per the
        // lib contract, but must not panic or invent rows).
        mapper.apply(
            &mut vm,
            &IngestEvent::Progress {
                job: IngestJob::Terrain,
                done: 1,
                total: Some(2),
                detail: "…".into(),
            },
        );
        assert!(vm.jobs.is_empty());
        assert!(!vm.visible);
    }

    #[test]
    fn run_result_messages() {
        let base = IngestRunResult {
            plan: IngestPlan {
                aero: true,
                basemap: true,
                ..IngestPlan::default()
            },
            outcome: IngestRunOutcome::Success,
            finished_at: Utc::now(),
        };
        assert_eq!(base.message(), "Data download complete (aero, basemap).");
        let cancelled = IngestRunResult {
            outcome: IngestRunOutcome::Cancelled,
            ..base.clone()
        };
        assert_eq!(cancelled.message(), "Data download cancelled.");
        let failed = IngestRunResult {
            outcome: IngestRunOutcome::Failed("basemap: timeout".into()),
            ..base
        };
        assert_eq!(failed.message(), "Data download failed: basemap: timeout");
    }
}

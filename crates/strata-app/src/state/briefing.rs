//! Briefing state (plan §5.3): the flight's NOTAM snapshot, its
//! relevance-ranked briefing list, the flight-panel NOTAM badge, and the
//! ICAO FPL export plumbing. The Briefing tab and the Flight ▸ Export menu
//! render exclusively off this module's API.
//!
//! # NOTAM snapshot semantics (design §3.4)
//!
//! The flight *stores the snapshot it was planned with*:
//! [`AppState::refresh_notams`] fetches via the configured
//! [`NotamProvider`] and lands the result as
//! [`FlightDoc::notam_snapshot`] — document state, so it persists with
//! the file, marks the flight dirty and reschedules the compute like any
//! other edit. Relevance ([`OpenFlight::briefing`]) is *derived* state:
//! recomputed against the current route whenever a compute run lands (the
//! corridor is a compute output) and immediately after a snapshot refresh.
//!
//! # The provider seam (plan §2.4)
//!
//! [`build_notam_provider`] is the one wiring site: autorouter credentials
//! in the config's `[autorouter]` section construct the live
//! [`AutorouterClient`]; without them there is **no** NOTAM provider —
//! the Briefing tab renders the honest not-configured state
//! ([`CREDENTIALS_MISSING`]) instead of fixture data. The provider is
//! rebuilt whenever the credentials change in settings
//! ([`AppState::rebuild_notam_provider`]).

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use gpui::{AppContext as _, Context};
use gpui_tokio::Tokio;
use serde::{Deserialize, Serialize};
use strata_data::domain::{IcaoCode, Notam};
use strata_data::providers::autorouter::AutorouterClient;
use strata_data::providers::{NotamProvider, TimeWindow as ProviderWindow};
use strata_plan::compute::ComputedFlight;
use strata_plan::flight::{FlightDoc, NOTAM_SNAPSHOT_FORMAT_VERSION, NotamSnapshot};
use strata_plan::fpl::{self, FplError, PilotInfo};
use strata_plan::notam_relevance::{self, AltitudeBand, RelevanceInput, RelevantNotam, TimeWindow};
use strata_plan::units::Minutes;

use crate::config::Config;

use super::{AppState, AppStateEvent, ComputeState, OpenFlight};

/// The German FIRs every briefing queries (plan §2.4). A VFR flight in
/// scope crosses at most these; the relevance filter discards what the
/// route never comes near.
pub const GERMAN_FIRS: [&str; 3] = ["EDGG", "EDMM", "EDWW"];

/// How far ahead of the departure anchor the snapshot window reaches —
/// generous enough that ETA drift from route edits never needs a refetch.
const SNAPSHOT_WINDOW_AHEAD: Duration = Duration::hours(24);
/// Slack before the departure anchor (late off-blocks, NOTAMs starting
/// "now").
const SNAPSHOT_WINDOW_BEHIND: Duration = Duration::hours(1);

// --- provider seam ------------------------------------------------------------

/// The Briefing tab's honest message while no autorouter credentials are
/// configured (the empty state and the disabled refresh both show it).
pub const CREDENTIALS_MISSING: &str =
    "Autorouter credentials not configured — set them in Settings.";

/// Which NOTAM source the config selects (the pure decision behind
/// [`build_notam_provider`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotamSource {
    /// No autorouter credentials in the config — the app has no NOTAM
    /// provider, and the Briefing tab says so ([`CREDENTIALS_MISSING`]).
    NotConfigured,
    /// The live autorouter.aero client.
    Autorouter,
}

/// [`NotamSource::Autorouter`] exactly when the `[autorouter]` config
/// section carries both credentials (non-blank).
pub fn notam_source(config: &Config) -> NotamSource {
    if config.autorouter.credentials().is_some() {
        NotamSource::Autorouter
    } else {
        NotamSource::NotConfigured
    }
}

/// **The provider wiring site.** Constructs the NOTAM provider the app
/// uses for every briefing fetch: the autorouter client when the config
/// carries credentials, `None` otherwise — there is no fixture fallback
/// at runtime.
pub fn build_notam_provider(config: &Config) -> Option<Arc<dyn NotamProvider>> {
    let (email, password) = config.autorouter.credentials()?;
    tracing::info!("NOTAM provider: autorouter.aero (credentials configured)");
    Some(Arc::new(AutorouterClient::new(email, password)))
}

// --- the snapshot payload -----------------------------------------------------

/// Typed shape of [`NotamSnapshot::payload`] — the app's briefing layer
/// owns it (the planning core round-trips the `Value` verbatim). Bumping
/// this shape means bumping [`NOTAM_SNAPSHOT_FORMAT_VERSION`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotamSnapshotPayload {
    /// The decoded NOTAMs as fetched (aerodrome + FIR queries, merged).
    pub notams: Vec<Notam>,
    /// The validity window the fetch covered — also the briefing window
    /// the relevance filter judges against.
    pub window: TimeWindow,
}

/// Wraps a payload into the document snapshot container.
pub fn encode_snapshot(
    payload: &NotamSnapshotPayload,
    taken_at: DateTime<Utc>,
) -> Result<NotamSnapshot, serde_json::Error> {
    Ok(NotamSnapshot::new(taken_at, serde_json::to_value(payload)?))
}

/// Decodes the document snapshot. `None` (with a warning) for a payload
/// this build cannot read — a newer format version or a malformed value;
/// the briefing then renders as never-fetched rather than failing the
/// document load.
pub fn decode_snapshot(snapshot: &NotamSnapshot) -> Option<NotamSnapshotPayload> {
    if snapshot.format_version > NOTAM_SNAPSHOT_FORMAT_VERSION {
        tracing::warn!(
            version = snapshot.format_version,
            "NOTAM snapshot from a newer format version; ignoring it"
        );
        return None;
    }
    match serde_json::from_value(snapshot.payload.clone()) {
        Ok(payload) => Some(payload),
        Err(err) => {
            tracing::warn!(%err, "malformed NOTAM snapshot payload; ignoring it");
            None
        }
    }
}

// --- fetch --------------------------------------------------------------------

/// What one snapshot fetch queries (pure derivation from the document).
#[derive(Debug, Clone, PartialEq)]
pub struct NotamFetchScope {
    /// The flight's aerodromes (route order + alternates) — item A query.
    pub locations: Vec<IcaoCode>,
    /// FIRs queried for en-route/area NOTAMs.
    pub firs: Vec<IcaoCode>,
    /// Validity window of the fetch.
    pub window: TimeWindow,
}

/// The fetch scope for `doc`: its aerodromes (the same list the relevance
/// ranking groups by), the German FIRs, and a window from one hour before
/// the departure anchor to 24 h after it (`now` when no departure time is
/// set).
pub fn fetch_scope(doc: &FlightDoc, now: DateTime<Utc>) -> NotamFetchScope {
    let anchor = doc.departure_time.unwrap_or(now);
    NotamFetchScope {
        locations: notam_relevance::route_aerodromes(&doc.route, &doc.alternates),
        firs: GERMAN_FIRS
            .iter()
            .map(|fir| IcaoCode::new(fir).expect("FIR constants are valid ICAO codes"))
            .collect(),
        window: TimeWindow::new(
            anchor - SNAPSHOT_WINDOW_BEHIND,
            anchor + SNAPSHOT_WINDOW_AHEAD,
        ),
    }
}

/// Runs the provider queries for `scope` and merges the results,
/// deduplicated by NOTAM id (a NOTAM filed against both a queried
/// aerodrome and its FIR appears once).
async fn fetch_notams(
    provider: Arc<dyn NotamProvider>,
    scope: NotamFetchScope,
) -> anyhow::Result<Vec<Notam>> {
    let window = ProviderWindow {
        from: scope.window.from,
        to: scope.window.to,
    };
    let mut notams = if scope.locations.is_empty() {
        Vec::new()
    } else {
        provider
            .notams_by_locations(&scope.locations, window)
            .await?
    };
    for fir in &scope.firs {
        notams.extend(provider.notams_by_fir(fir, window).await?);
    }
    let mut seen = std::collections::HashSet::new();
    notams.retain(|notam| seen.insert(notam.id));
    Ok(notams)
}

/// Foreground bookkeeping of the one in-flight NOTAM fetch. Generation-
/// guarded like the compute pipeline: a newer refresh or a flight switch
/// ([`Self::invalidate`]) makes the in-flight result land nowhere.
#[derive(Debug, Default)]
pub struct NotamFetchState {
    generation: u64,
    /// A fetch is running (the Briefing tab's refresh spinner).
    pub fetching: bool,
    /// The last fetch failed with this message (cleared by the next
    /// refresh and by flight switches).
    pub last_error: Option<String>,
}

impl NotamFetchState {
    /// Claims a fetch generation and enters the fetching state.
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.fetching = true;
        self.last_error = None;
        self.generation
    }

    /// Records a finished fetch; `false` means the result is stale (a
    /// newer refresh or a flight switch superseded it) and must be
    /// dropped.
    fn finish(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.fetching {
            return false;
        }
        self.fetching = false;
        true
    }

    /// Drops any in-flight fetch result (flight switched/closed).
    pub(crate) fn invalidate(&mut self) {
        self.generation += 1;
        self.fetching = false;
        self.last_error = None;
    }
}

// --- relevance (derived state on the open flight) -------------------------------

/// The relevance-ranked briefing list derived from the document snapshot
/// and the latest computed corridor — view-facing state on
/// [`OpenFlight::briefing`], never persisted (the snapshot is).
#[derive(Debug, Clone, PartialEq)]
pub struct BriefingRelevance {
    /// Timestamp of the snapshot the list was computed from.
    pub taken_at: DateTime<Utc>,
    /// Ordered briefing list (see `strata_plan::notam_relevance`).
    pub relevant: Vec<RelevantNotam>,
}

/// Pure core of [`AppState::refresh_briefing_relevance`]: snapshot +
/// computed outputs → the briefing list.
///
/// Degrades honestly while the flight is not computable: with no corridor
/// the geometric class can't match, but aerodrome and FIR NOTAMs still
/// brief (empty corridor, no altitude band). The flight window is
/// departure → ETA when both exist; departure + 24 h without an ETA
/// (conservative: more NOTAMs count as active, never fewer); the briefing
/// window itself when no departure time is set.
pub fn derive_briefing(
    doc: &FlightDoc,
    computed: Option<&ComputedFlight>,
) -> Option<BriefingRelevance> {
    let snapshot = doc.notam_snapshot.as_ref()?;
    let payload = decode_snapshot(snapshot)?;
    let empty = strata_plan::corridor::Corridor {
        params: strata_plan::corridor::CorridorParams::default(),
        samples: Vec::new(),
        crossings: Vec::new(),
    };
    let corridor = computed.map_or(&empty, |computed| &computed.corridor);
    let relevant = notam_relevance::relevant_notams(&RelevanceInput {
        notams: &payload.notams,
        route: &doc.route,
        alternates: &doc.alternates,
        corridor,
        briefing_window: payload.window,
        flight_window: flight_window(doc, computed, payload.window),
        altitude_band: computed.and_then(|computed| AltitudeBand::from_phases(&computed.phases)),
    });
    Some(BriefingRelevance {
        taken_at: snapshot.taken_at,
        relevant,
    })
}

/// Off-blocks → ETA; see [`derive_briefing`] for the fallbacks.
fn flight_window(
    doc: &FlightDoc,
    computed: Option<&ComputedFlight>,
    briefing_window: TimeWindow,
) -> TimeWindow {
    let Some(departure) = doc.departure_time else {
        return briefing_window;
    };
    if let Some(arrival) = computed
        .and_then(|computed| computed.navlog.rows.last())
        .and_then(|row| row.eta)
        .filter(|arrival| *arrival > departure)
    {
        return TimeWindow::new(departure, arrival);
    }
    let duration = computed
        .map(|computed| computed.navlog.totals.ete)
        .filter(|ete| ete.0 > 0.0)
        .unwrap_or(Minutes(24.0 * 60.0));
    TimeWindow::new(
        departure,
        departure + Duration::milliseconds((duration.0 * 60_000.0) as i64),
    )
}

// --- the NOTAM badge ------------------------------------------------------------

/// Flight-panel NOTAM badge state (design §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotamBadge {
    /// No usable snapshot — never fetched (or unreadable). Renders as an
    /// em-dash.
    NotFetched,
    /// Snapshot present, no relevant NOTAM active during the flight
    /// (green).
    Clear,
    /// Relevant NOTAMs active during the flight, none of the red class
    /// (amber).
    Relevant,
    /// An active restriction-activation NOTAM (ED-R/D/P class) intersects
    /// the corridor (red).
    RestrictionActive,
}

/// Pure badge derivation from the derived briefing list (`None` = no
/// usable snapshot).
pub fn notam_badge(briefing: Option<&BriefingRelevance>) -> NotamBadge {
    let Some(briefing) = briefing else {
        return NotamBadge::NotFetched;
    };
    let active = || {
        briefing
            .relevant
            .iter()
            .filter(|entry| entry.active_during_flight)
    };
    let restriction_in_corridor = active().any(|entry| {
        matches!(
            entry.relevance,
            notam_relevance::NotamRelevance::RouteCorridor { .. }
        ) && notam_relevance::is_restriction_activation(&entry.notam)
    });
    if restriction_in_corridor {
        NotamBadge::RestrictionActive
    } else if active().next().is_some() {
        NotamBadge::Relevant
    } else {
        NotamBadge::Clear
    }
}

// --- FPL export -------------------------------------------------------------------

/// Outcome of generating the ICAO FPL for the open flight — what the
/// Briefing tab's preview renders and the export action gates on.
#[derive(Debug, Clone, PartialEq)]
pub enum FplOutcome {
    /// No flight is open.
    NoFlight,
    /// The FPL needs computed outputs (EET) or a resolved aircraft;
    /// carries the user-facing explanation.
    NotComputed(String),
    /// Generation failed local validation — the typed per-item error
    /// (its `Display` names the ICAO item and the reason).
    Invalid(FplError),
    /// The locally validated message text.
    Ready(String),
}

/// Pure core of [`AppState::icao_fpl`].
pub fn fpl_outcome(
    flight: Option<&OpenFlight>,
    aircraft: Option<&strata_plan::AircraftProfile>,
    pilot: &PilotInfo,
) -> FplOutcome {
    let Some(flight) = flight else {
        return FplOutcome::NoFlight;
    };
    let Some(aircraft) = aircraft else {
        let reason = match &flight.doc.aircraft_id {
            Some(id) => format!("aircraft profile \"{id}\" is not in the library"),
            None => "no aircraft selected".to_owned(),
        };
        return FplOutcome::NotComputed(reason);
    };
    let Some(computed) = flight.computed.as_deref() else {
        let reason = match &flight.compute_state {
            ComputeState::Pending => "the flight has not been computed yet".to_owned(),
            ComputeState::NotComputable(gap) => gap.to_string(),
            ComputeState::Failed(error) => format!("the last compute failed: {error}"),
            // Computed without outputs cannot happen; phrase it honestly.
            ComputeState::Computed => "computed outputs are unavailable".to_owned(),
        };
        return FplOutcome::NotComputed(reason);
    };
    match fpl::generate(&flight.doc, aircraft, computed, pilot) {
        Ok(message) => FplOutcome::Ready(message),
        Err(err) => FplOutcome::Invalid(err),
    }
}

/// Suggested file name for the exported FPL text: the slugged flight name
/// (or route summary) + `-fpl.txt`.
pub fn fpl_file_name(doc: &FlightDoc) -> String {
    format!("{}-fpl.txt", flight_slug(doc))
}

/// Suggested file name for the exported briefing PDF: the slugged flight
/// name (or route summary) + `-briefing.pdf`.
pub fn pdf_file_name(doc: &FlightDoc) -> String {
    format!("{}-briefing.pdf", flight_slug(doc))
}

/// The flight's file-name slug: lowercased name (route summary for blank
/// names) with everything non-alphanumeric collapsed to single dashes.
fn flight_slug(doc: &FlightDoc) -> String {
    let base = if doc.name.trim().is_empty() {
        crate::flight_io::flights::route_summary(doc)
    } else {
        doc.name.clone()
    };
    let mut slug = String::new();
    for c in base.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "flight" } else { slug }.to_owned()
}

/// A finished export, surfaced to the user (the root view renders these
/// as window notifications, like `IngestNotice`).
#[derive(Debug, Clone, PartialEq)]
pub enum ExportNotice {
    FplSaved(PathBuf),
    FplFailed(String),
    PdfSaved(PathBuf),
    PdfFailed(String),
}

impl AppState {
    /// Rebuilds the NOTAM provider from the current config — called after
    /// the settings dialog commits autorouter credentials, so the change
    /// applies immediately (and a fresh client drops any cached token of
    /// the previous credentials).
    pub(crate) fn rebuild_notam_provider(&mut self) {
        self.notam_provider = build_notam_provider(&self.config);
    }

    /// Refreshes the open flight's NOTAM snapshot: fetches via the
    /// configured provider for the document's scope and lands the result
    /// as `doc.notam_snapshot` (a document edit — dirty flag, compute
    /// reschedule and `FlightChanged` follow from the edit funnel).
    /// Emits [`AppStateEvent::BriefingChanged`] around the fetch state.
    /// No-op outside planning mode and without a configured provider
    /// (the Briefing tab disables refresh and explains why).
    pub fn refresh_notams(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = &self.flight else {
            return;
        };
        let Some(provider) = self.notam_provider.clone() else {
            tracing::debug!("NOTAM refresh requested without autorouter credentials");
            return;
        };
        let scope = fetch_scope(&flight.doc, Utc::now());
        let generation = self.notam_fetch.begin();
        cx.emit(AppStateEvent::BriefingChanged);
        cx.notify();

        // The autorouter client is reqwest-based → tokio bridge (the
        // fixture provider doesn't care where it runs).
        let fetch = Tokio::spawn_result(cx, async move {
            fetch_notams(provider, scope.clone())
                .await
                .map(|notams| (notams, scope))
        });
        self.notam_fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                if !this.notam_fetch.finish(generation) {
                    tracing::debug!(generation, "dropping stale NOTAM fetch result");
                    return;
                }
                match result {
                    Ok((notams, scope)) => {
                        let taken_at = Utc::now();
                        tracing::info!(
                            count = notams.len(),
                            locations = scope.locations.len(),
                            "NOTAM snapshot refreshed"
                        );
                        let payload = NotamSnapshotPayload {
                            notams,
                            window: scope.window,
                        };
                        match encode_snapshot(&payload, taken_at) {
                            Ok(snapshot) => {
                                // Snapshot-only fast path (mirroring the
                                // notes-only edit): dirty + epoch +
                                // `FlightChanged`, but no compute
                                // reschedule and no winds prefetch — no
                                // computed output reads the snapshot, and
                                // cancelling an in-flight run for it
                                // would be pure churn.
                                if let Some(flight) = &mut this.flight {
                                    flight.doc.notam_snapshot = Some(snapshot);
                                    flight.dirty = true;
                                    flight.edit_epoch += 1;
                                    cx.emit(AppStateEvent::FlightChanged);
                                }
                                // The list should not lag the snapshot it
                                // shows.
                                this.refresh_briefing_relevance(cx);
                            }
                            Err(err) => {
                                tracing::warn!(%err, "encoding NOTAM snapshot failed");
                                this.notam_fetch.last_error = Some(err.to_string());
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, "NOTAM fetch failed");
                        this.notam_fetch.last_error = Some(err.to_string());
                    }
                }
                cx.emit(AppStateEvent::BriefingChanged);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Drops the running NOTAM fetch and invalidates its generation —
    /// called on flight install/close so a late result never lands on
    /// another document.
    pub(crate) fn cancel_notam_fetch(&mut self) {
        self.notam_fetch_task = None;
        self.notam_fetch.invalidate();
    }

    /// Re-derives [`OpenFlight::briefing`] from the document snapshot and
    /// the latest computed outputs. Called whenever a compute run lands
    /// (the corridor changed) and after a snapshot refresh; emits
    /// [`AppStateEvent::BriefingChanged`] only when the list changed.
    pub(crate) fn refresh_briefing_relevance(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = &mut self.flight else {
            return;
        };
        let next = derive_briefing(&flight.doc, flight.computed.as_deref());
        if flight.briefing == next {
            return;
        }
        flight.briefing = next;
        cx.emit(AppStateEvent::BriefingChanged);
        cx.notify();
    }

    /// The flight-panel NOTAM badge for the open flight.
    pub fn notam_badge(&self) -> NotamBadge {
        notam_badge(self.flight.as_ref().and_then(|f| f.briefing.as_ref()))
    }

    /// The ICAO FPL for the open flight: generated from the document, the
    /// resolved aircraft, the computed EET and the configured pilot data;
    /// locally validated per item. Pure read — the Briefing tab calls this
    /// for the preview, the export action gates on `Ready`.
    pub fn icao_fpl(&self) -> FplOutcome {
        fpl_outcome(
            self.flight.as_ref(),
            self.flight_aircraft(),
            &self.config.pilot,
        )
    }

    /// Exports the briefing PDF for the open flight. The caller converts
    /// the document/computed state into the [`strata_brief::BriefingInput`]
    /// (the pure, cheap part — `ui::context_tabs::briefing::input`); this
    /// method owns the slow half: typst rendering on the **background
    /// executor** (≈ a second — [`AppState::pdf_exporting`] drives the
    /// spinner, the UI thread never blocks), then the platform save dialog,
    /// the async write, and the [`ExportNotice`] either way. One export at
    /// a time; no-op outside planning mode.
    pub fn export_briefing_pdf(
        &mut self,
        input: strata_brief::BriefingInput,
        cx: &mut Context<Self>,
    ) {
        if self.pdf_exporting {
            tracing::debug!("briefing PDF export already running");
            return;
        }
        let Some(flight) = &self.flight else {
            return;
        };
        let file_name = pdf_file_name(&flight.doc);
        self.pdf_exporting = true;
        cx.notify();

        let render = cx.background_spawn(async move { strata_brief::render_briefing(&input) });
        cx.spawn(async move |this, cx| {
            let rendered = render.await;
            // The slow part is over — stop the spinner before the (modal,
            // user-paced) save dialog, and bail out on render errors.
            let receiver = this.update(cx, |this, cx| {
                this.pdf_exporting = false;
                cx.notify();
                match &rendered {
                    Ok(_) => {
                        let directory = dirs::document_dir()
                            .or_else(dirs::home_dir)
                            .unwrap_or_else(|| PathBuf::from("."));
                        Some(cx.prompt_for_new_path(&directory, Some(&file_name)))
                    }
                    Err(err) => {
                        tracing::warn!(%err, "briefing PDF render failed");
                        cx.emit(AppStateEvent::ExportFinished(ExportNotice::PdfFailed(
                            err.to_string(),
                        )));
                        None
                    }
                }
            });
            let (Ok(Some(receiver)), Ok(bytes)) = (receiver, rendered) else {
                return;
            };
            // Outer Err = channel dropped, inner Err = portal failure,
            // None = cancelled — none of them export anything.
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let path = with_extension(path, "pdf");
            let write_path = path.clone();
            let result = cx
                .background_spawn(async move { std::fs::write(&write_path, &bytes) })
                .await;
            this.update(cx, |_, cx| {
                let notice = match result {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "briefing PDF exported");
                        ExportNotice::PdfSaved(path)
                    }
                    Err(err) => {
                        tracing::warn!(path = %path.display(), %err, "briefing PDF export failed");
                        ExportNotice::PdfFailed(err.to_string())
                    }
                };
                cx.emit(AppStateEvent::ExportFinished(notice));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Exports the FPL message as plain text: platform save dialog (the
    /// Save As… recipe), async write, [`ExportNotice`] on the event
    /// channel either way. No-op (warned) unless the FPL is `Ready`.
    pub fn export_fpl(&mut self, cx: &mut Context<Self>) {
        let message = match self.icao_fpl() {
            FplOutcome::Ready(message) => message,
            outcome => {
                tracing::warn!(?outcome, "FPL export requested while not exportable");
                return;
            }
        };
        let directory = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = self
            .flight
            .as_ref()
            .map(|flight| fpl_file_name(&flight.doc));
        let receiver = cx.prompt_for_new_path(&directory, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            // Outer Err = channel dropped, inner Err = portal failure,
            // None = cancelled — none of them export anything.
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let path = with_extension(path, "txt");
            let write_path = path.clone();
            let result = cx
                .background_spawn(
                    async move { std::fs::write(&write_path, format!("{message}\n")) },
                )
                .await;
            this.update(cx, |_, cx| {
                let notice = match result {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "ICAO FPL exported");
                        ExportNotice::FplSaved(path)
                    }
                    Err(err) => {
                        tracing::warn!(path = %path.display(), %err, "FPL export failed");
                        ExportNotice::FplFailed(err.to_string())
                    }
                };
                cx.emit(AppStateEvent::ExportFinished(notice));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Forces the export's extension onto the picked path (the portal may
/// return whatever the user typed).
fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    if path.extension().is_some_and(|ext| ext == extension) {
        path
    } else {
        path.with_extension(extension)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::LatLon;
    use strata_data::providers::autorouter::FixtureNotamProvider;
    use strata_plan::flight::{FreePoint, NamedPoint, NamedPointKind, RoutePoint, RouteWaypoint};
    use strata_plan::notam_relevance::NotamRelevance;

    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid")
    }

    fn airport(id: &str, lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Named(NamedPoint {
            kind: NamedPointKind::Airport,
            id: id.to_owned(),
            name: id.to_owned(),
            position: LatLon::new(lat, lon).expect("valid"),
        })
    }

    fn corpus() -> Vec<Notam> {
        FixtureNotamProvider::builtin().notams().to_vec()
    }

    fn icao(code: &str) -> IcaoCode {
        IcaoCode::new(code).expect("valid")
    }

    // --- provider seam -------------------------------------------------------

    /// Without both credentials the app has no NOTAM provider (the
    /// Briefing tab's not-configured empty state); with them, the
    /// autorouter client. No fixture path exists at runtime.
    #[test]
    fn notam_source_requires_both_credentials() {
        let mut config = Config::default();
        assert_eq!(notam_source(&config), NotamSource::NotConfigured);
        assert!(build_notam_provider(&config).is_none());

        config.autorouter.email = Some("user@example.com".to_owned());
        assert_eq!(notam_source(&config), NotamSource::NotConfigured);
        assert!(build_notam_provider(&config).is_none());

        config.autorouter.password = Some("  ".to_owned());
        assert_eq!(
            notam_source(&config),
            NotamSource::NotConfigured,
            "blank = unset"
        );

        config.autorouter.password = Some("hunter2".to_owned());
        assert_eq!(notam_source(&config), NotamSource::Autorouter);
        assert!(build_notam_provider(&config).is_some());
    }

    // --- snapshot serde --------------------------------------------------------

    /// The payload round-trips through the document container *and* the
    /// flight file format (the doc's pretty-JSON serde).
    #[test]
    fn snapshot_payload_round_trips_through_the_flight_document() {
        let payload = NotamSnapshotPayload {
            notams: corpus().into_iter().take(3).collect(),
            window: TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0)),
        };
        let taken_at = utc(2026, 6, 16, 8, 30);
        let snapshot = encode_snapshot(&payload, taken_at).expect("encodes");
        assert_eq!(snapshot.format_version, NOTAM_SNAPSHOT_FORMAT_VERSION);
        assert_eq!(snapshot.taken_at, taken_at);

        let mut doc = FlightDoc::new("EDDF → EDDM");
        doc.notam_snapshot = Some(snapshot);
        let json = doc.to_json_string().expect("serializes");
        let loaded = FlightDoc::from_json_str(&json).expect("loads");
        let restored = loaded.notam_snapshot.expect("snapshot survives");
        assert_eq!(restored.taken_at, taken_at);
        assert_eq!(decode_snapshot(&restored).expect("decodes"), payload);
    }

    #[test]
    fn unreadable_snapshots_decode_to_none() {
        // A future format version is ignored, not misread.
        let mut newer = NotamSnapshot::new(utc(2026, 6, 16, 8, 0), serde_json::json!({}));
        newer.format_version = NOTAM_SNAPSHOT_FORMAT_VERSION + 1;
        assert_eq!(decode_snapshot(&newer), None);

        // A malformed payload is ignored.
        let malformed = NotamSnapshot::new(
            utc(2026, 6, 16, 8, 0),
            serde_json::json!({ "notams": "not-a-list" }),
        );
        assert_eq!(decode_snapshot(&malformed), None);
    }

    // --- fetch scope -------------------------------------------------------------

    #[test]
    fn fetch_scope_covers_route_aerodromes_firs_and_the_departure_window() {
        let mut doc = FlightDoc::new("EDDF → EDDM");
        doc.route = vec![
            RouteWaypoint::new(airport("EDDF", 50.03, 8.57)),
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: None,
                position: LatLon::new(49.5, 10.0).expect("valid"),
            })),
            RouteWaypoint::new(airport("EDDM", 48.35, 11.79)),
        ];
        doc.alternates = vec![airport("EDDS", 48.69, 9.22)];
        doc.departure_time = Some(utc(2026, 6, 16, 9, 0));

        let scope = fetch_scope(&doc, utc(2026, 6, 1, 0, 0));
        assert_eq!(
            scope.locations,
            vec![icao("EDDF"), icao("EDDM"), icao("EDDS")]
        );
        assert_eq!(scope.firs, vec![icao("EDGG"), icao("EDMM"), icao("EDWW")]);
        assert_eq!(scope.window.from, utc(2026, 6, 16, 8, 0));
        assert_eq!(scope.window.to, utc(2026, 6, 17, 9, 0));

        // No departure time → anchored at `now`.
        doc.departure_time = None;
        let scope = fetch_scope(&doc, utc(2026, 6, 1, 12, 0));
        assert_eq!(scope.window.from, utc(2026, 6, 1, 11, 0));
    }

    #[test]
    fn flight_window_prefers_the_displayed_navlog_eta() {
        let (_dir, doc, computed) = computed_flight();
        let mut computed = (*computed).clone();
        let departure = doc.departure_time.expect("fixture has departure");
        let displayed_arrival = departure + Duration::minutes(73);
        computed.phases.total_duration = Minutes(20.0);
        computed.navlog.totals.ete = Minutes(73.0);
        computed
            .navlog
            .rows
            .last_mut()
            .expect("destination row")
            .eta = Some(displayed_arrival);

        let fallback = TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0));
        let window = flight_window(&doc, Some(&computed), fallback);
        assert_eq!(window.from, departure);
        assert_eq!(window.to, displayed_arrival);

        computed
            .navlog
            .rows
            .last_mut()
            .expect("destination row")
            .eta = None;
        let window = flight_window(&doc, Some(&computed), fallback);
        assert_eq!(window.to, departure + Duration::minutes(73));
    }

    #[tokio::test]
    async fn fetch_merges_and_dedupes_location_and_fir_queries() {
        let provider: Arc<dyn NotamProvider> = Arc::new(FixtureNotamProvider::builtin());
        let scope = NotamFetchScope {
            locations: vec![icao("EDDF"), icao("EDDM")],
            firs: vec![icao("EDGG"), icao("EDMM"), icao("EDWW")],
            window: TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0)),
        };
        let notams = fetch_notams(provider, scope).await.expect("fixture fetch");
        let ids: Vec<String> = notams.iter().map(|n| n.id.to_string()).collect();
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len(), "no duplicate ids: {ids:?}");
        // Aerodrome NOTAMs and FIR NOTAMs both present.
        assert!(ids.contains(&"A1234/26".to_owned()), "{ids:?}");
        assert!(ids.contains(&"D0452/26".to_owned()), "{ids:?}");
    }

    // --- fetch-state generations ----------------------------------------------------

    #[test]
    fn stale_and_invalidated_fetches_never_land() {
        let mut state = NotamFetchState::default();
        let first = state.begin();
        assert!(state.fetching);

        // A newer refresh supersedes the first.
        let second = state.begin();
        assert!(!state.finish(first));
        assert!(state.finish(second));
        assert!(!state.fetching);
        assert!(!state.finish(second), "double finish rejected");

        // A flight switch drops the in-flight fetch.
        let third = state.begin();
        state.invalidate();
        assert!(!state.fetching);
        assert!(!state.finish(third));
    }

    // --- relevance derivation + badge -------------------------------------------------

    /// EDDF → EDDM with the corpus snapshot but no computed corridor:
    /// aerodrome NOTAMs brief, geometric classes need the compute.
    #[test]
    fn derive_briefing_degrades_to_aerodromes_without_a_compute() {
        let mut doc = FlightDoc::new("EDDF → EDDM");
        doc.route = vec![
            RouteWaypoint::new(airport("EDDF", 50.0333, 8.5706)),
            RouteWaypoint::new(airport("EDDM", 48.3538, 11.7861)),
        ];
        doc.departure_time = Some(utc(2026, 6, 16, 9, 0));
        let payload = NotamSnapshotPayload {
            notams: corpus(),
            window: TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0)),
        };
        doc.notam_snapshot =
            Some(encode_snapshot(&payload, utc(2026, 6, 16, 8, 30)).expect("encodes"));

        let briefing = derive_briefing(&doc, None).expect("snapshot present");
        assert_eq!(briefing.taken_at, utc(2026, 6, 16, 8, 30));
        assert!(!briefing.relevant.is_empty());
        assert!(
            briefing
                .relevant
                .iter()
                .all(|entry| matches!(entry.relevance, NotamRelevance::Aerodrome(_))),
            "no corridor/FIR classes without a corridor"
        );

        // No snapshot → no briefing.
        doc.notam_snapshot = None;
        assert_eq!(derive_briefing(&doc, None), None);
    }

    /// End-to-end glue: the corpus snapshot against a *computed* corridor
    /// (EDFE → EDQN passes through the Frankfurt glider area and the GPS
    /// jamming circle) classifies geometrically and drives the badge.
    #[test]
    fn derive_briefing_with_a_computed_corridor_feeds_the_badge() {
        let (_dir, mut doc, computed) = computed_flight();
        let payload = NotamSnapshotPayload {
            notams: corpus(),
            window: TimeWindow::new(utc(2026, 6, 16, 8, 0), utc(2026, 6, 17, 9, 0)),
        };
        doc.notam_snapshot =
            Some(encode_snapshot(&payload, utc(2026, 6, 16, 8, 30)).expect("encodes"));

        let briefing = derive_briefing(&doc, Some(&computed)).expect("briefing derives");
        let corridor_ids: Vec<String> = briefing
            .relevant
            .iter()
            .filter(|entry| matches!(entry.relevance, NotamRelevance::RouteCorridor { .. }))
            .map(|entry| entry.notam.id.to_string())
            .collect();
        assert_eq!(
            corridor_ids,
            vec![
                // GPS jamming + glider activity over the departure, then —
                // entering ~22 km down the track — the *off-route* EDDF
                // NOTAMs, whose 5 NM aerodrome circles the corridor clips
                // (EDDF is not a flight aerodrome here, so they classify
                // geometrically, like a route PIB would include them).
                "E0231/26".to_owned(),
                "W0903/26".to_owned(),
                "A1300/26".to_owned(),
                "A1234/26".to_owned(),
            ],
        );

        // Active corridor warnings, none of the restriction class: amber.
        assert_eq!(notam_badge(Some(&briefing)), NotamBadge::Relevant);
    }

    fn relevant(text: &str, relevance: NotamRelevance, active: bool) -> RelevantNotam {
        RelevantNotam {
            notam: Notam::parse(text).expect("test NOTAM parses"),
            relevance,
            active_during_flight: active,
        }
    }

    const EDR_ACTIVATION: &str = "D0001/26 NOTAMN\nQ) EDMM/QRRCA/IV/BO/W/000/100/4942N01156E010\nA) EDMM B) 2606160700 C) 2606181500\nE) ED-R ACT";
    const PJE_WARNING: &str = "W0001/26 NOTAMN\nQ) EDGG/QWPLW/IV/M/W/000/050/5000N00815E003\nA) EDGG B) 2606160700 C) 2606181500\nE) PJE";
    const RWY_CLOSED: &str = "A0001/26 NOTAMN\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nA) EDDF B) 2606160700 C) 2606181500\nE) RWY CLSD";

    #[test]
    fn badge_states_cover_the_design_table() {
        // Never fetched: em-dash.
        assert_eq!(notam_badge(None), NotamBadge::NotFetched);

        let briefing = |relevant: Vec<RelevantNotam>| BriefingRelevance {
            taken_at: utc(2026, 6, 16, 8, 30),
            relevant,
        };

        // Snapshot fetched, nothing relevant: green.
        assert_eq!(notam_badge(Some(&briefing(Vec::new()))), NotamBadge::Clear);

        // Relevant but nothing active during the flight: green.
        let inactive = briefing(vec![relevant(
            RWY_CLOSED,
            NotamRelevance::Aerodrome(icao("EDDF")),
            false,
        )]);
        assert_eq!(notam_badge(Some(&inactive)), NotamBadge::Clear);

        // Active relevant NOTAMs, none of the red class: amber.
        let amber = briefing(vec![
            relevant(RWY_CLOSED, NotamRelevance::Aerodrome(icao("EDDF")), true),
            relevant(
                PJE_WARNING,
                NotamRelevance::RouteCorridor {
                    distance_nm: strata_plan::units::NauticalMiles(0.0),
                },
                true,
            ),
        ]);
        assert_eq!(notam_badge(Some(&amber)), NotamBadge::Relevant);

        // An active ED-R activation in the corridor: red.
        let red = briefing(vec![relevant(
            EDR_ACTIVATION,
            NotamRelevance::RouteCorridor {
                distance_nm: strata_plan::units::NauticalMiles(0.0),
            },
            true,
        )]);
        assert_eq!(notam_badge(Some(&red)), NotamBadge::RestrictionActive);

        // The same activation merely FIR-listed or inactive: not red.
        let fir_only = briefing(vec![relevant(EDR_ACTIVATION, NotamRelevance::Fir, true)]);
        assert_eq!(notam_badge(Some(&fir_only)), NotamBadge::Relevant);
        let inactive_edr = briefing(vec![relevant(
            EDR_ACTIVATION,
            NotamRelevance::RouteCorridor {
                distance_nm: strata_plan::units::NauticalMiles(0.0),
            },
            false,
        )]);
        assert_eq!(notam_badge(Some(&inactive_edr)), NotamBadge::Clear);
    }

    // --- FPL export ----------------------------------------------------------------

    /// An [`OpenFlight`] shell around `doc` (the struct literal — the
    /// installing constructor is private to the flight module).
    fn open_flight(doc: FlightDoc, computed: Option<Arc<ComputedFlight>>) -> OpenFlight {
        let compute_state = if computed.is_some() {
            ComputeState::Computed
        } else {
            ComputeState::Pending
        };
        OpenFlight {
            doc,
            path: None,
            dirty: false,
            computed,
            compute_state,
            briefing: None,
            compute_generation: Default::default(),
            edit_epoch: 0,
            instance_id: 0,
            write_seq: 0,
            elevation_cache: None,
        }
    }

    /// A computed two-leg flight over a temp store (the compute test
    /// recipe), ready for FPL generation.
    fn computed_flight() -> (tempfile::TempDir, FlightDoc, Arc<ComputedFlight>) {
        use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId, Store};
        use strata_plan::flight::PlannedAltitude;

        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::open(&dir.path().join("store.sqlite")).expect("store opens");
        for lon in [8.0, 8.5, 9.0] {
            let id = ElevationTileId::containing(50.0, lon);
            let tile = ElevationTile::new(id, vec![250; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE])
                .expect("tile");
            store.put_elevation_tile(&tile).expect("tile stored");
        }

        let aircraft = crate::flight_io::aircraft::example_c172();
        let mut doc = FlightDoc::new("test export");
        doc.route = vec![
            RouteWaypoint::new(airport("EDFE", 50.0, 8.0)),
            RouteWaypoint::new(airport("EDQN", 50.0, 9.0)),
        ];
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(
            strata_data::domain::MetersAmsl::from_feet(4500.0),
        ));
        doc.departure_time = Some(utc(2026, 6, 16, 9, 30));
        doc.aircraft_id = Some(aircraft.id.clone());
        doc.loading.fuel = strata_plan::units::Liters(150.0);

        let (outcome, _) = crate::state::flight::compute::run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::new(store)),
            Arc::new(crate::sources::WindsAloftFrames::default()),
            &strata_plan::compute::ComputeParams::default(),
            None,
        );
        let crate::state::flight::compute::ComputeOutcome::Computed(computed) = outcome else {
            panic!("test flight computes: {outcome:?}");
        };
        (dir, doc, computed)
    }

    fn pilot() -> PilotInfo {
        PilotInfo {
            pilot_in_command: "Test Pilot".to_owned(),
            persons_on_board: Some(2),
            aircraft_color: None,
        }
    }

    #[test]
    fn fpl_outcome_walks_the_readiness_ladder() {
        let pilot = pilot();
        assert_eq!(fpl_outcome(None, None, &pilot), FplOutcome::NoFlight);

        // Flight open, no aircraft selected.
        let flight = open_flight(FlightDoc::new("x"), None);
        let FplOutcome::NotComputed(reason) = fpl_outcome(Some(&flight), None, &pilot) else {
            panic!("no aircraft → NotComputed");
        };
        assert!(reason.contains("no aircraft"), "{reason}");

        // Aircraft resolved but nothing computed yet.
        let aircraft = crate::flight_io::aircraft::example_c172();
        let mut doc = FlightDoc::new("x");
        doc.aircraft_id = Some(aircraft.id.clone());
        let flight = open_flight(doc, None);
        let FplOutcome::NotComputed(reason) = fpl_outcome(Some(&flight), Some(&aircraft), &pilot)
        else {
            panic!("uncomputed → NotComputed");
        };
        assert!(reason.contains("not been computed"), "{reason}");
    }

    #[test]
    fn fpl_generates_and_validates_for_a_computed_flight() {
        let (_dir, doc, computed) = computed_flight();
        let aircraft = crate::flight_io::aircraft::example_c172();
        let flight = open_flight(doc, Some(computed));

        let FplOutcome::Ready(message) = fpl_outcome(Some(&flight), Some(&aircraft), &pilot())
        else {
            panic!("computed flight with pilot data is exportable");
        };
        assert!(message.starts_with("(FPL-DEXAA-VG"), "{message}");
        assert!(message.contains("-EDFE0930"), "{message}");
        assert!(message.contains("C/TEST PILOT"), "{message}");
        assert!(message.ends_with(')'), "{message}");

        // Missing pilot-in-command: generation fails item 19 validation,
        // surfaced as the typed error (the Briefing tab renders it).
        let outcome = fpl_outcome(Some(&flight), Some(&aircraft), &PilotInfo::default());
        assert_eq!(
            outcome,
            FplOutcome::Invalid(FplError::MissingData {
                item: 19,
                what: "the pilot in command"
            })
        );
    }

    #[test]
    fn fpl_file_names_are_slugged_with_a_fallback() {
        let mut doc = FlightDoc::new("EDFE → EDQN");
        assert_eq!(fpl_file_name(&doc), "edfe-edqn-fpl.txt");
        // Blank name → the route summary ("No route" for an empty one).
        doc.name = "  ".to_owned();
        assert_eq!(fpl_file_name(&doc), "no-route-fpl.txt");
        // A name with no sluggable characters → the generic fallback.
        doc.name = "→→".to_owned();
        assert_eq!(fpl_file_name(&doc), "flight-fpl.txt");
    }

    #[test]
    fn export_extensions_are_forced() {
        assert_eq!(
            with_extension(PathBuf::from("/tmp/plan"), "txt"),
            PathBuf::from("/tmp/plan.txt")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/plan.txt"), "txt"),
            PathBuf::from("/tmp/plan.txt")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/plan.text"), "txt"),
            PathBuf::from("/tmp/plan.txt")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/brief"), "pdf"),
            PathBuf::from("/tmp/brief.pdf")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/brief.pdf"), "pdf"),
            PathBuf::from("/tmp/brief.pdf")
        );
    }

    #[test]
    fn pdf_file_names_share_the_slug() {
        let doc = FlightDoc::new("Bavaria Test Hop");
        assert_eq!(pdf_file_name(&doc), "bavaria-test-hop-briefing.pdf");
        assert_eq!(fpl_file_name(&doc), "bavaria-test-hop-fpl.txt");
    }
}

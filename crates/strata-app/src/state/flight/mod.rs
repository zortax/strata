//! The open flight document on [`AppState`] (plan §5.1): mode flag,
//! mutation API, dirty tracking and the library/file plumbing.
//!
//! Planning mode exists **only** while [`AppState::flight`] is `Some` —
//! the default launch is the untouched explorer. Views subscribe to the
//! flight events on [`AppStateEvent`]:
//!
//! - `FlightOpened` — a document was installed (new/open/replace),
//! - `FlightChanged` — the document, dirty flag or path changed,
//! - `FlightComputed` — a compute run landed (see [`compute`]),
//! - `FlightClosed` — planning mode ended.
//!
//! Every document mutation funnels through [`AppState::edit_flight_doc`],
//! which marks dirty, emits `FlightChanged` and schedules the debounced
//! background compute. Saving never blocks the UI thread; the dirty flag
//! clears only when no edit raced the write (generation-checked).

pub mod compute;
pub mod ops;
pub mod render_route;
pub mod winds;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use gpui::{AppContext as _, Context, Task};
use strata_data::domain::{LatLon, Meters, MetersAmsl};
use strata_plan::aircraft::AircraftId;
use strata_plan::compute::{ComputedFlight, NotComputable};
use strata_plan::conflict::{ConflictKind, ConflictLocation};
use strata_plan::flight::{PlannedAltitude, RoutePoint};
use strata_plan::{AircraftProfile, FlightDoc};

use crate::flight_io;
use compute::ComputeGeneration;

use super::{AppState, AppStateEvent};

/// Default cruise altitude seeded into **new** flight documents, in feet
/// AMSL — the one place this default lives. With it, a fresh route computes
/// as soon as it has two waypoints; legs inherit it unless they carry their
/// own altitude (design §3.1 — the flight panel exposes the cruise quick-set
/// and per-leg overrides). Loaded documents are installed verbatim.
pub const DEFAULT_CRUISE_ALTITUDE_FEET: f64 = 3000.0;

/// [`DEFAULT_CRUISE_ALTITUDE_FEET`] as the typed document altitude.
pub fn default_cruise_altitude() -> PlannedAltitude {
    PlannedAltitude::Amsl(MetersAmsl::from_feet(DEFAULT_CRUISE_ALTITUDE_FEET))
}

/// Trailing-edge debounce of the aircraft editor's disk writes (the
/// editor commits a profile on every parseable keystroke — see
/// [`AppState::upsert_aircraft_profile`]).
const AIRCRAFT_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// The pending, debounced aircraft-profile write. Dropping it cancels the
/// trailing-edge timer; the flush always writes the profile's *current*
/// library state, so the last edit wins.
pub(crate) struct PendingAircraftSave {
    id: AircraftId,
    _timer: Task<()>,
}

/// A fresh flight document with the app's planning defaults applied (the
/// default cruise altitude) — what [`AppState::new_flight`] installs.
pub fn new_flight_doc(name: impl Into<String>) -> FlightDoc {
    let mut doc = FlightDoc::new(name);
    doc.cruise_altitude = Some(default_cruise_altitude());
    doc
}

/// What a flight-panel badge asks to focus (design §3.1: "clicking a badge
/// opens the relevant surface"). The UI's request vocabulary; resolved into
/// a broadcastable [`PlanningFocus`] by
/// [`AppState::request_planning_focus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusRequest {
    /// The profile drawer, scrubbed to the first conflict among `kinds`
    /// (the Terrain and Airspace badges).
    Conflicts(&'static [ConflictKind]),
    /// The context panel's Loading tab (the W&B badge).
    Loading,
    /// The context panel's Fuel tab (the Fuel badge).
    Fuel,
    /// The context panel's Briefing tab (the NOTAM badge).
    Briefing,
}

/// A resolved badge-navigation target, carried by
/// [`AppStateEvent::PlanningFocusRequested`] (design §4: "conflicts carry a
/// location so every badge can navigate to its cause").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanningFocus {
    /// Expand the profile drawer onto its Profile tab; when a matching
    /// conflict exists the scrub already sits on it (set before the event)
    /// and the map flies to `target`.
    Profile { target: Option<ConflictTarget> },
    /// Switch the context panel to its Loading tab.
    Loading,
    /// Switch the context panel to its Fuel tab.
    Fuel,
    /// Switch the context panel to its Briefing tab.
    Briefing,
}

/// Where a conflict lives, resolved against the computed flight: the
/// along-track scrub position plus the geographic point the map flies to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConflictTarget {
    pub along_track: Meters,
    pub position: LatLon,
}

/// The first conflict among `kinds` **along track** that resolves to a
/// location: station conflicts directly, leg conflicts at the leg's
/// midpoint. The engine lists conflicts grouped by kind, not by route
/// position — a badge click should land on the earliest trouble of any
/// matching kind, so the resolved targets are ordered by along-track
/// distance here, at the navigation site. Document-level conflicts (W&B,
/// fuel) have no place to go and are skipped.
pub fn first_conflict_target(
    computed: &ComputedFlight,
    kinds: &[ConflictKind],
) -> Option<ConflictTarget> {
    computed
        .conflicts
        .iter()
        .filter(|conflict| kinds.contains(&conflict.kind))
        .filter_map(|conflict| match conflict.location {
            ConflictLocation::Station {
                along_track,
                position,
            } => Some(ConflictTarget {
                along_track,
                position,
            }),
            ConflictLocation::Leg { index } => {
                let leg = computed.legs.get(index)?;
                let before: f64 = computed.legs[..index]
                    .iter()
                    .map(|leg| leg.distance.0)
                    .sum();
                Some(ConflictTarget {
                    along_track: Meters(before + leg.distance.0 / 2.0),
                    position: leg.midpoint,
                })
            }
            ConflictLocation::Flight => None,
        })
        .min_by(|a, b| a.along_track.0.total_cmp(&b.along_track.0))
}

/// Outcome state of the most recent finished compute run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComputeState {
    /// No run has finished for the current document generation yet.
    #[default]
    Pending,
    /// The latest run produced [`OpenFlight::computed`].
    Computed,
    /// The document is not computable in its current editing state (fewer
    /// than two distinct waypoints, no aircraft selected, …) — a normal
    /// interim state, not an error. Carries the typed reason; its `Display`
    /// is the user-facing phrasing.
    NotComputable(NotComputable),
    /// The latest run failed (store/source error, inconsistent profile).
    Failed(String),
}

/// The open flight document and everything derived from it.
pub struct OpenFlight {
    pub doc: FlightDoc,
    /// File the document was loaded from / saved to; `None` until the
    /// first save (Save → Save As… in the menu).
    pub path: Option<PathBuf>,
    /// Unsaved changes exist (the title-strip dot).
    pub dirty: bool,
    /// Latest computed outputs; `None` until the first successful run and
    /// whenever the latest run produced no result (see [`Self::compute_state`]).
    pub computed: Option<Arc<ComputedFlight>>,
    /// Outcome of the most recent finished run.
    pub compute_state: ComputeState,
    /// NOTAM briefing list derived from the document snapshot and the
    /// computed corridor; `None` until a usable snapshot exists (see
    /// `state::briefing` — relevance is recomputed on every compute
    /// landing and after snapshot refreshes, never persisted).
    pub briefing: Option<crate::state::briefing::BriefingRelevance>,
    /// Generation bookkeeping: every *compute-relevant* doc edit claims a
    /// new generation, and only the matching run's result lands (stale
    /// results are dropped).
    pub compute_generation: ComputeGeneration,
    /// Monotonic edit counter guarding async dirty-clearing: **every**
    /// document mutation bumps it — including notes-only edits, which
    /// deliberately do *not* claim a compute generation (the fast path in
    /// [`OpenFlight::apply_notes_only_edit`]) — so a save completion only
    /// clears the dirty flag when no edit of any kind raced the write.
    pub edit_epoch: u64,
    /// Process-unique identity of this open document *instance*. The
    /// per-document counters above restart at zero on every install and
    /// are meaningless to compare across documents, so an async save
    /// completion must check it still talks to the same instance before
    /// adopting its path or clearing the dirty flag — otherwise a save of
    /// flight A racing a switch to flight B makes B inherit A's file path
    /// (and the next plain Save of B overwrites A's file).
    pub instance_id: u64,
    /// Monotonic count of *started* writes for this document. A save
    /// completion only adopts its path (and possibly clears dirty) when it
    /// was the latest-started write — two racing Save As… calls both write
    /// their files, but the document keeps pointing at the last path the
    /// user picked, not whichever write happened to finish last.
    pub write_seq: u64,
    /// Decoded elevation tiles reused across compute runs while their
    /// coverage contains the route envelope (see
    /// [`compute::ElevationCache`]); freed with the flight, invalidated by
    /// post-ingest data reloads.
    pub elevation_cache: Option<compute::ElevationCache>,
}

impl OpenFlight {
    fn new(doc: FlightDoc, path: Option<PathBuf>) -> Self {
        static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            doc,
            path,
            dirty: false,
            computed: None,
            compute_state: ComputeState::default(),
            briefing: None,
            compute_generation: ComputeGeneration::default(),
            edit_epoch: 0,
            instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
            write_seq: 0,
            elevation_cache: None,
        }
    }

    /// Whether a save completion that captured (`instance_id`,
    /// `write_seq`) at save start may mutate this document: the write must
    /// belong to *this* instance (not a flight that was since replaced)
    /// and be the latest-started one (no Save As… raced past it). The
    /// dirty flag additionally needs the edit-epoch check — an edit during
    /// the write keeps the document dirty even when the completion is
    /// accepted.
    pub(crate) fn accepts_save_completion(&self, instance_id: u64, write_seq: u64) -> bool {
        self.instance_id == instance_id && self.write_seq == write_seq
    }

    /// The notes-only fast path (pure core): applies the notes edit, marks
    /// dirty and bumps the edit epoch **without** claiming a compute
    /// generation — nav-log notes are the one document field no computed
    /// output depends on, so rescheduling the debounced compute (and
    /// superseding a possibly in-flight run) for every keystroke in the
    /// Notes column would be pure waste. Surfaces read notes from the
    /// document (the drawer's inputs, the PDF conversion's overlay), never
    /// from the computed rows' copies. Returns whether anything changed.
    pub(crate) fn apply_notes_only_edit(&mut self, index: usize, notes: String) -> bool {
        if !ops::set_waypoint_notes(&mut self.doc, index, notes) {
            return false;
        }
        self.dirty = true;
        self.edit_epoch += 1;
        true
    }
}

// The flight API is consumed by the title-bar menu / flight panel / map
// editing phases building on this state core; not every entry point has
// its UI caller in-tree yet.
#[allow(dead_code)]
impl AppState {
    /// Whether planning mode is active (design §2: mode =
    /// `flight.is_some()`).
    pub fn planning_mode(&self) -> bool {
        self.flight.is_some()
    }

    /// The loaded aircraft profile for `id`, if any.
    pub fn aircraft_profile(&self, id: &AircraftId) -> Option<&AircraftProfile> {
        self.aircraft_library.iter().find(|p| &p.id == id)
    }

    /// The aircraft profile the open flight references, if resolved.
    pub fn flight_aircraft(&self) -> Option<&AircraftProfile> {
        let id = self.flight.as_ref()?.doc.aircraft_id.as_ref()?;
        self.aircraft_profile(id)
    }

    // --- open / new / close ------------------------------------------------

    /// Creates and installs a fresh flight document with the planning
    /// defaults applied ([`new_flight_doc`]; planning mode on). Replaces
    /// any open flight — the UI is responsible for save prompts.
    pub fn new_flight(&mut self, name: impl Into<String>, cx: &mut Context<Self>) {
        self.install_flight(new_flight_doc(name), None, cx);
    }

    /// Loads `path` in the background and installs it (planning mode on).
    /// A failed load leaves the current state untouched (with a warning).
    pub fn open_flight(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let load_path = path.clone();
        let load = cx.background_spawn(async move { flight_io::load_flight(&load_path) });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| match result {
                Ok(doc) => this.install_flight(doc, Some(path), cx),
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "opening flight failed");
                }
            })
            .ok();
        })
        .detach();
    }

    /// Closes the open flight (planning mode off). Unsaved changes are
    /// discarded — the UI prompts before calling this.
    pub fn close_flight(&mut self, cx: &mut Context<Self>) {
        if self.flight.take().is_none() {
            return;
        }
        self.flight_compute_task = None;
        self.cancel_notam_fetch();
        self.flight_winds.stop();
        self.reset_profile_scrub(cx);
        self.reset_route_highlight(cx);
        cx.emit(AppStateEvent::FlightClosed);
        cx.notify();
    }

    /// Clears any leftover scrub cursor (the position belongs to the
    /// previous flight's track). Emits `ProfileScrubChanged` only when a
    /// scrub was actually live.
    fn reset_profile_scrub(&mut self, cx: &mut Context<Self>) {
        if self.profile_scrub.take().is_some() {
            cx.emit(AppStateEvent::ProfileScrubChanged);
        }
    }

    /// Clears any leftover hover highlight (the id belongs to the previous
    /// flight's route). Emits `RouteHighlightChanged` only when a
    /// highlight was actually live.
    fn reset_route_highlight(&mut self, cx: &mut Context<Self>) {
        if self.route_highlight.take().is_some() {
            cx.emit(AppStateEvent::RouteHighlightChanged);
        }
    }

    // --- badge navigation -------------------------------------------------

    /// The badge-navigation funnel (design §3.1): resolves `request`
    /// against the computed conflicts, moves the shared profile scrub onto
    /// the found conflict (so the drawer crosshair and the map marker are
    /// already there) and emits
    /// [`AppStateEvent::PlanningFocusRequested`] — the drawer expands, the
    /// context panel switches tab, the map flies, each from its own
    /// subscription. No-op outside planning mode.
    pub fn request_planning_focus(&mut self, request: FocusRequest, cx: &mut Context<Self>) {
        if self.flight.is_none() {
            return;
        }
        let focus = match request {
            FocusRequest::Conflicts(kinds) => {
                let target = self
                    .flight
                    .as_ref()
                    .and_then(|flight| flight.computed.as_deref())
                    .and_then(|computed| first_conflict_target(computed, kinds));
                PlanningFocus::Profile { target }
            }
            FocusRequest::Loading => PlanningFocus::Loading,
            FocusRequest::Fuel => PlanningFocus::Fuel,
            FocusRequest::Briefing => PlanningFocus::Briefing,
        };
        if let PlanningFocus::Profile {
            target: Some(target),
        } = focus
        {
            self.set_profile_scrub(Some(target.along_track), cx);
        }
        cx.emit(AppStateEvent::PlanningFocusRequested(focus));
        cx.notify();
    }

    /// Installs `doc` as the open flight, replacing any previous one
    /// (`FlightClosed` then `FlightOpened`), and kicks off the first
    /// compute + winds prefetch. Opened-from-file flights join the
    /// recent list.
    fn install_flight(&mut self, doc: FlightDoc, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.flight.is_some() {
            self.flight_compute_task = None;
            cx.emit(AppStateEvent::FlightClosed);
        }
        // An in-flight NOTAM fetch belongs to the previous document; its
        // result must not land on this one.
        self.cancel_notam_fetch();
        self.reset_profile_scrub(cx);
        self.reset_route_highlight(cx);
        if let Some(path) = &path {
            self.note_recent_flight(path, cx);
        }
        tracing::info!(
            name = %doc.name,
            path = path.as_ref().map(|p| p.display().to_string()),
            "flight opened"
        );
        self.flight = Some(OpenFlight::new(doc, path));
        cx.emit(AppStateEvent::FlightOpened);
        cx.notify();
        self.schedule_flight_compute(cx);
        self.maybe_prefetch_flight_winds(cx);
    }

    // --- save ----------------------------------------------------------------

    /// Saves to the document's path. Returns `false` when no flight is open
    /// or the document has no path yet — the caller routes to Save As….
    /// The write is async + atomic; on success the dirty flag clears unless
    /// an edit raced the write.
    pub fn save_flight(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.flight.as_ref().and_then(|f| f.path.clone()) else {
            return false;
        };
        self.write_flight_to(path, cx).detach();
        true
    }

    /// Saves to `path` and adopts it as the document's path.
    pub fn save_flight_as(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.write_flight_to(path, cx).detach();
    }

    /// Saves to the document's path — or the deduplicated library default
    /// for never-saved flights — and returns the completion task instead
    /// of detaching it, so callers that tear the window down afterwards
    /// (the close-window guard) can await the write first: process exit
    /// does not wait for detached tasks, and a save lost that way is
    /// silent data loss. `None` when no flight is open.
    pub fn save_flight_to_known_path(&mut self, cx: &mut Context<Self>) -> Option<Task<()>> {
        let path = match self.flight.as_ref()?.path.clone() {
            Some(path) => path,
            None => self.default_flight_path()?,
        };
        Some(self.write_flight_to(path, cx))
    }

    /// A free file path for the open flight in the library directory
    /// (`<data_dir>/flights/<slug>.strata-flight`, deduplicated) — what
    /// the Save flow uses when the document has no path yet.
    pub fn default_flight_path(&self) -> Option<PathBuf> {
        let flight = self.flight.as_ref()?;
        let dir = flight_io::flights_dir(&self.data_dir);
        let name = if flight.doc.name.trim().is_empty() {
            flight_io::flights::route_summary(&flight.doc)
        } else {
            flight.doc.name.clone()
        };
        Some(flight_io::allocate_flight_path(&dir, &name))
    }

    /// Starts the async atomic write and returns the foreground completion
    /// task (callers either `.detach()` it or await it before closing the
    /// window). The completion only mutates the open flight when it is
    /// still the same document instance's latest-started write — a flight
    /// switch or a later Save As… racing the write invalidates it: the
    /// file is still written (and joins the recent list), the in-memory
    /// document just does not adopt it.
    fn write_flight_to(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Task<()> {
        let Some(flight) = &mut self.flight else {
            return Task::ready(());
        };
        let doc = flight.doc.clone();
        // The edit epoch at save time: an edit during the async write —
        // including a notes-only edit — bumps it, and the completion must
        // then keep the dirty flag set.
        let epoch = flight.edit_epoch;
        // Identity + ordering at save time (see accepts_save_completion).
        let instance = flight.instance_id;
        flight.write_seq += 1;
        let seq = flight.write_seq;
        let write_path = path.clone();
        // The write ticket is captured with the snapshot: an older save
        // completing late can never overwrite a newer one on disk.
        let ticket = crate::fsutil::WriteTicket::next();
        let write = cx.background_spawn(async move {
            flight_io::save_flight_ordered(&write_path, &doc, ticket)
        });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.note_recent_flight(&path, cx);
                        if let Some(flight) = &mut this.flight
                            && flight.accepts_save_completion(instance, seq)
                        {
                            flight.path = Some(path);
                            if flight.edit_epoch == epoch {
                                flight.dirty = false;
                            }
                            cx.emit(AppStateEvent::FlightChanged);
                            cx.notify();
                        }
                    }
                    Err(err) => {
                        // The document stays dirty; the failure is logged
                        // (surfacing it as a window notification is the
                        // menu phase's concern).
                        tracing::warn!(path = %path.display(), %err, "saving flight failed");
                    }
                }
            })
            .ok();
        })
    }

    /// Adds `path` to the config's recent list and persists the config off
    /// the UI thread (ordered — see [`AppState::persist_config`]).
    fn note_recent_flight(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if !self.config.note_recent_flight(path) {
            return;
        }
        self.persist_config("recent flights", cx);
    }

    // --- document mutation ----------------------------------------------------

    /// Marks the open flight dirty and reschedules compute — the generic
    /// "the document changed" signal for callers that mutated `doc`
    /// directly instead of going through a setter below.
    pub fn mark_flight_dirty(&mut self, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |_| true);
    }

    /// Appends a waypoint to the end of the route.
    pub fn append_waypoint(&mut self, point: RoutePoint, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::append_waypoint(doc, point));
    }

    /// Inserts a waypoint at `index` (0 = before the departure,
    /// `route.len()` = append). Out-of-range indices are a warned no-op —
    /// the UI passes indices from the same document generation.
    pub fn insert_waypoint(&mut self, index: usize, point: RoutePoint, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::insert_waypoint(doc, index, point));
    }

    /// Removes the waypoint at `index` (out-of-range: warned no-op).
    pub fn remove_waypoint(&mut self, index: usize, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::remove_waypoint(doc, index));
    }

    /// Reorders the route: the waypoint at `from` moves to position `to`
    /// (indices into the current route; list drag-reorder semantics). Leg
    /// plans travel with the waypoint they are stored on.
    pub fn move_waypoint(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::move_waypoint(doc, from, to));
    }

    /// Replaces the *point* of the waypoint at `index` — the map-drag /
    /// re-snap operation. The leg plan (altitude/wind) stays.
    pub fn replace_waypoint_point(
        &mut self,
        index: usize,
        point: RoutePoint,
        cx: &mut Context<Self>,
    ) {
        self.edit_flight_doc(cx, |doc| ops::replace_waypoint_point(doc, index, point));
    }

    /// Sets (or clears) the planned altitude of the leg *from* waypoint
    /// `index` to its successor.
    pub fn set_leg_altitude(
        &mut self,
        index: usize,
        altitude: Option<PlannedAltitude>,
        cx: &mut Context<Self>,
    ) {
        self.edit_flight_doc(cx, |doc| ops::set_leg_altitude(doc, index, altitude));
    }

    /// Sets (or clears) the flight's default cruise altitude.
    pub fn set_cruise_altitude(
        &mut self,
        altitude: Option<PlannedAltitude>,
        cx: &mut Context<Self>,
    ) {
        self.edit_flight_doc(cx, |doc| ops::set_cruise_altitude(doc, altitude));
    }

    /// Sets (or clears) the alternate destination. The document model holds
    /// a list, but planning consumes the first alternate (design §3.4) —
    /// this replaces the whole list.
    pub fn set_alternate(&mut self, alternate: Option<RoutePoint>, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::set_alternate(doc, alternate));
    }

    /// Selects the aircraft profile the flight is planned with.
    pub fn set_flight_aircraft(&mut self, id: Option<AircraftId>, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::set_aircraft(doc, id));
    }

    /// Sets (or clears) the planned departure time (UTC).
    pub fn set_departure_time(&mut self, time: Option<DateTime<Utc>>, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::set_departure_time(doc, time));
    }

    /// Renames the flight (the panel header's inline edit).
    pub fn set_flight_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_flight_doc(cx, |doc| ops::set_name(doc, name));
    }

    /// Sets the nav-log notes of the waypoint at `index` (the profile
    /// drawer's Notes column — persisted on the document per leg row).
    ///
    /// **Notes-only fast path:** marks dirty and emits `FlightChanged`
    /// like any edit, but skips the recompute and the winds prefetch —
    /// notes feed no computed output (see
    /// [`OpenFlight::apply_notes_only_edit`]).
    pub fn set_waypoint_notes(&mut self, index: usize, notes: String, cx: &mut Context<Self>) {
        let Some(flight) = &mut self.flight else {
            tracing::warn!("flight edit while no flight is open");
            return;
        };
        if !flight.apply_notes_only_edit(index, notes) {
            return;
        }
        cx.emit(AppStateEvent::FlightChanged);
        cx.notify();
    }

    /// The one funnel for compute-relevant document mutations: applies
    /// `edit` and — when it reports a change — marks dirty, bumps the edit
    /// epoch, emits [`AppStateEvent::FlightChanged`] and schedules the
    /// debounced compute plus the winds prefetch. Returns whether anything
    /// changed. (Nav-log notes go through [`Self::set_waypoint_notes`]'s
    /// fast path instead — same bookkeeping, no recompute.)
    pub fn edit_flight_doc(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut FlightDoc) -> bool,
    ) -> bool {
        let Some(flight) = &mut self.flight else {
            tracing::warn!("flight edit while no flight is open");
            return false;
        };
        if !edit(&mut flight.doc) {
            return false;
        }
        flight.dirty = true;
        flight.edit_epoch += 1;
        cx.emit(AppStateEvent::FlightChanged);
        cx.notify();
        self.schedule_flight_compute(cx);
        self.maybe_prefetch_flight_winds(cx);
        true
    }

    // --- aircraft library -----------------------------------------------------

    /// Loads the aircraft profile library in the background, seeding the
    /// two bundled examples on first run (empty `aircraft/` dir). Called at
    /// startup and by the aircraft manager after profile edits.
    pub fn reload_aircraft_library(&mut self, cx: &mut Context<Self>) {
        let dir = flight_io::aircraft_dir(&self.data_dir);
        cx.spawn(async move |this, cx| {
            let profiles = cx
                .background_spawn(async move {
                    if let Err(err) = flight_io::ensure_example_aircraft(&dir) {
                        tracing::warn!(%err, "seeding example aircraft failed");
                    }
                    flight_io::list_aircraft(&dir)
                })
                .await;
            this.update(cx, |this, cx| {
                tracing::debug!(count = profiles.len(), "aircraft library loaded");
                this.aircraft_library = profiles;
                // The open flight may reference a profile that just became
                // resolvable (startup ordering) or changed — recompute.
                this.broadcast_aircraft_library_change(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Library scan for the open dialog (background-friendly helper around
    /// [`flight_io::list_flights`]).
    pub fn flights_dir(&self) -> PathBuf {
        flight_io::flights_dir(&self.data_dir)
    }

    /// The aircraft profile directory (`<data_dir>/aircraft/`).
    pub fn aircraft_dir(&self) -> PathBuf {
        flight_io::aircraft_dir(&self.data_dir)
    }

    /// Installs `profile` into the in-memory library (replacing the entry
    /// with the same id, keeping the id sort) — the aircraft manager's
    /// save-on-change funnel. Emits
    /// [`AppStateEvent::AircraftLibraryChanged`] and reschedules the open
    /// flight's compute, so a flight planned with the edited profile
    /// updates live.
    ///
    /// The **disk write is trailing-edge debounced**: the editor commits
    /// on every parseable keystroke, and writing each one would fsync the
    /// same file several times a second while making concurrent-write
    /// races likely. The in-memory upsert and the broadcast stay
    /// immediate; the file write coalesces onto the last edit, flushing
    /// early when the edits switch to a different profile and on app quit
    /// (see [`AppState::flush_pending_aircraft_save_on_quit`]).
    pub fn upsert_aircraft_profile(&mut self, profile: AircraftProfile, cx: &mut Context<Self>) {
        let id = profile.id.clone();
        match self
            .aircraft_library
            .iter_mut()
            .find(|p| p.id == profile.id)
        {
            Some(slot) => *slot = profile,
            None => {
                self.aircraft_library.push(profile);
                self.aircraft_library.sort_by(|a, b| a.id.cmp(&b.id));
            }
        }
        self.broadcast_aircraft_library_change(cx);

        // A pending write for a *different* profile flushes now instead of
        // being cancelled.
        if self
            .pending_aircraft_save
            .as_ref()
            .is_some_and(|pending| pending.id != id)
        {
            self.flush_pending_aircraft_save(cx);
        }
        // (Re)arm the trailing-edge timer; replacing the previous pending
        // save drops — and thereby cancels — its timer.
        self.pending_aircraft_save = Some(PendingAircraftSave {
            id,
            _timer: cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AIRCRAFT_SAVE_DEBOUNCE).await;
                this.update(cx, |this, cx| this.flush_pending_aircraft_save(cx))
                    .ok();
            }),
        });
    }

    /// Writes the pending profile's *current* library state to disk
    /// (ordered, off the UI thread) and clears the pending slot. No-op
    /// when nothing is pending or the profile was deleted meanwhile.
    fn flush_pending_aircraft_save(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_aircraft_save.take() else {
            return;
        };
        if let Some(write) = self.spawn_aircraft_write(&pending.id, cx) {
            write.detach();
        }
    }

    /// App-quit flush of the pending aircraft write (registered via
    /// `cx.on_app_quit` in [`AppState::new`]): the returned future is
    /// polled through gpui's shutdown window, so the last keystrokes reach
    /// disk even when the app quits inside the debounce window — process
    /// exit does not wait for detached tasks.
    pub(crate) fn flush_pending_aircraft_save_on_quit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = ()> + use<> {
        let write = self
            .pending_aircraft_save
            .take()
            .and_then(|pending| self.spawn_aircraft_write(&pending.id, cx));
        async move {
            if let Some(write) = write {
                write.await;
            }
        }
    }

    /// Spawns the ordered background write of `id`'s current library
    /// state. `None` when `id` is no longer in the library (deleted
    /// meanwhile — nothing to write).
    fn spawn_aircraft_write(&self, id: &AircraftId, cx: &mut Context<Self>) -> Option<Task<()>> {
        let profile = self.aircraft_profile(id)?.clone();
        let dir = self.aircraft_dir();
        let ticket = crate::fsutil::WriteTicket::next();
        Some(cx.background_spawn(async move {
            if let Err(err) = flight_io::save_aircraft_ordered(&dir, &profile, ticket) {
                tracing::warn!(id = %profile.id, %err, "saving aircraft profile failed");
            }
        }))
    }

    /// Removes the profile `id` from the library and deletes its file off
    /// the UI thread. A flight referencing the deleted profile recomputes
    /// into the benign `UnknownAircraft` not-computable state.
    pub fn delete_aircraft_profile(&mut self, id: &AircraftId, cx: &mut Context<Self>) {
        let before = self.aircraft_library.len();
        self.aircraft_library.retain(|p| &p.id != id);
        if self.aircraft_library.len() == before {
            return;
        }
        // A pending debounced write of the deleted profile must not race
        // the file removal (the flush would no-op now that the profile
        // left the library, but cancelling the timer is cleaner).
        if self
            .pending_aircraft_save
            .as_ref()
            .is_some_and(|pending| &pending.id == id)
        {
            self.pending_aircraft_save = None;
        }
        let dir = self.aircraft_dir();
        let id = id.clone();
        cx.background_spawn(async move {
            if let Err(err) = flight_io::delete_aircraft(&dir, &id) {
                tracing::warn!(%id, %err, "deleting aircraft profile failed");
            }
        })
        .detach();
        self.broadcast_aircraft_library_change(cx);
    }

    /// The shared tail of every library mutation: event, notify, recompute.
    fn broadcast_aircraft_library_change(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppStateEvent::AircraftLibraryChanged);
        cx.notify();
        if self.flight.is_some() {
            self.schedule_flight_compute(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use strata_plan::flight::{FreePoint, RouteWaypoint};

    use super::*;

    /// New flights carry the seeded cruise default — the document computes
    /// as soon as it has two waypoints (the phase-4 gate's complaint) —
    /// while everything else stays at the model's own defaults.
    #[test]
    fn new_flight_doc_seeds_the_default_cruise_altitude() {
        let doc = new_flight_doc("EDFE → EDQN");
        assert_eq!(doc.name, "EDFE → EDQN");
        assert_eq!(doc.cruise_altitude, Some(default_cruise_altitude()));
        let PlannedAltitude::Amsl(meters) = default_cruise_altitude() else {
            panic!("default is a plain AMSL altitude");
        };
        assert!((meters.as_feet() - DEFAULT_CRUISE_ALTITUDE_FEET).abs() < 1e-9);

        // The seed is the only deviation from the bare document model.
        let mut bare = FlightDoc::new("EDFE → EDQN");
        bare.cruise_altitude = doc.cruise_altitude;
        assert_eq!(doc, bare);
    }

    /// A freshly installed document: clean (a brand-new or just-loaded
    /// flight has nothing worth saving), nothing computed yet, generation
    /// at its never-applies zero state.
    #[test]
    fn open_flight_starts_clean_and_pending() {
        let flight = OpenFlight::new(FlightDoc::new("EDFE → EDQN"), None);
        assert!(!flight.dirty);
        assert!(flight.computed.is_none());
        assert_eq!(flight.compute_state, ComputeState::Pending);
        assert_eq!(flight.compute_generation, ComputeGeneration::default());
        assert!(flight.path.is_none());

        let path = PathBuf::from("/flights/trip.strata-flight");
        let flight = OpenFlight::new(FlightDoc::new("x"), Some(path.clone()));
        assert_eq!(flight.path, Some(path));
    }

    // --- badge navigation ---------------------------------------------------

    /// A minimal computed flight: `legs` (10 km each, midpoints at
    /// distinct longitudes) + `conflicts`; everything else empty.
    fn computed_with(
        leg_count: usize,
        conflicts: Vec<strata_plan::conflict::Conflict>,
    ) -> ComputedFlight {
        use strata_plan::corridor::{Corridor, CorridorParams};
        use strata_plan::fuel::FuelLadder;
        use strata_plan::navlog::{NavLog, NavLogTotals};
        use strata_plan::perf::PhasePlan;
        use strata_plan::units::{Liters, Minutes, NauticalMiles};
        use strata_plan::wb::WbReport;

        ComputedFlight {
            legs: (0..leg_count)
                .map(|index| strata_plan::compute::ComputedLeg {
                    index,
                    from: format!("W{index}"),
                    to: format!("W{}", index + 1),
                    distance: Meters(10_000.0),
                    true_track: strata_plan::units::DegreesTrue::new(90.0),
                    magnetic_track: strata_plan::units::DegreesTrue::new(90.0)
                        .to_magnetic(strata_plan::units::MagneticVariation(0.0)),
                    midpoint: LatLon::new(50.0, 8.0 + index as f64).unwrap(),
                })
                .collect(),
            corridor: Corridor {
                params: CorridorParams::default(),
                samples: Vec::new(),
                crossings: Vec::new(),
            },
            winds: Vec::new(),
            phases: PhasePlan {
                segments: Vec::new(),
                toc: None,
                tod: None,
                total_duration: Minutes(0.0),
                total_fuel: Liters(0.0),
            },
            weight_balance: WbReport {
                states: Vec::new(),
                burn_track: Vec::new(),
            },
            fuel: FuelLadder {
                taxi: Liters(0.0),
                trip: Liters(0.0),
                contingency: Liters(0.0),
                alternate: Liters(0.0),
                final_reserve: Liters(0.0),
                extra: Liters(0.0),
                minimum_required: Liters(0.0),
                loaded: Liters(0.0),
                margin: Liters(0.0),
            },
            conflicts,
            navlog: NavLog {
                rows: Vec::new(),
                totals: NavLogTotals {
                    distance: NauticalMiles(0.0),
                    ete: Minutes(0.0),
                    fuel: Liters(0.0),
                },
            },
        }
    }

    fn conflict(
        kind: ConflictKind,
        location: strata_plan::conflict::ConflictLocation,
    ) -> strata_plan::conflict::Conflict {
        strata_plan::conflict::Conflict {
            kind,
            severity: strata_plan::conflict::ConflictSeverity::Warning,
            location,
            message: "test".to_owned(),
        }
    }

    #[test]
    fn first_conflict_target_orders_along_track_and_skips_other_kinds() {
        let station = LatLon::new(49.0, 9.5).unwrap();
        let computed = computed_with(
            3,
            vec![
                // Wrong kind first — skipped even though it has a location
                // and sits earliest along track.
                conflict(
                    ConflictKind::Airspace,
                    ConflictLocation::Station {
                        along_track: Meters(2_000.0),
                        position: LatLon::new(48.0, 8.0).unwrap(),
                    },
                ),
                // The engine lists kinds in its own order: an obstacle
                // *later in the list* but *earlier along track* must win.
                conflict(
                    ConflictKind::Terrain,
                    ConflictLocation::Station {
                        along_track: Meters(20_000.0),
                        position: LatLon::new(47.0, 7.0).unwrap(),
                    },
                ),
                conflict(
                    ConflictKind::Obstacle,
                    ConflictLocation::Station {
                        along_track: Meters(12_500.0),
                        position: station,
                    },
                ),
            ],
        );
        let target =
            first_conflict_target(&computed, &[ConflictKind::Terrain, ConflictKind::Obstacle])
                .expect("a terrain conflict exists");
        assert_eq!(target.along_track, Meters(12_500.0), "earliest along track");
        assert_eq!(target.position, station);
    }

    /// Mixed location flavors sort together: a leg conflict whose midpoint
    /// lies before every station conflict is the navigation target.
    #[test]
    fn first_conflict_target_sorts_leg_midpoints_with_stations() {
        let computed = computed_with(
            3,
            vec![
                conflict(
                    ConflictKind::Airspace,
                    ConflictLocation::Station {
                        along_track: Meters(18_000.0),
                        position: LatLon::new(50.0, 10.0).unwrap(),
                    },
                ),
                // Leg 0 midpoint = 5 km — earliest along track.
                conflict(ConflictKind::Airspace, ConflictLocation::Leg { index: 0 }),
            ],
        );
        let target =
            first_conflict_target(&computed, &[ConflictKind::Airspace]).expect("conflicts resolve");
        assert_eq!(target.along_track, Meters(5_000.0));
        assert_eq!(target.position, computed.legs[0].midpoint);
    }

    #[test]
    fn leg_conflicts_resolve_to_the_leg_midpoint() {
        let computed = computed_with(
            3,
            vec![conflict(
                ConflictKind::Airspace,
                ConflictLocation::Leg { index: 1 },
            )],
        );
        let target = first_conflict_target(&computed, &[ConflictKind::Airspace])
            .expect("the leg conflict resolves");
        // Legs are 10 km each: leg 1's midpoint sits at 15 km.
        assert_eq!(target.along_track, Meters(15_000.0));
        assert_eq!(target.position, computed.legs[1].midpoint);
    }

    /// Document-level conflicts have no location; a stale leg index (the
    /// compute belongs to an older route) resolves to nothing rather than
    /// panicking.
    #[test]
    fn locationless_and_stale_conflicts_yield_no_target() {
        let computed = computed_with(
            1,
            vec![
                conflict(ConflictKind::Fuel, ConflictLocation::Flight),
                conflict(ConflictKind::Airspace, ConflictLocation::Leg { index: 7 }),
            ],
        );
        assert_eq!(
            first_conflict_target(&computed, &[ConflictKind::Fuel]),
            None
        );
        assert_eq!(
            first_conflict_target(&computed, &[ConflictKind::Airspace]),
            None
        );
        assert_eq!(
            first_conflict_target(&computed_with(1, Vec::new()), &[ConflictKind::Terrain]),
            None
        );
    }

    /// The save path's race guard: a doc edit between save start and save
    /// completion bumps the edit epoch, so the completion must keep dirty.
    #[test]
    fn save_epoch_detects_racing_edits() {
        let mut flight = OpenFlight::new(FlightDoc::new("x"), None);
        flight.dirty = true;

        // No racing edit: epochs match → dirty clears.
        let epoch = flight.edit_epoch;
        assert_eq!(flight.edit_epoch, epoch);

        // A racing edit (every mutation bumps the epoch — the funnel and
        // the notes fast path alike).
        let epoch = flight.edit_epoch;
        flight.edit_epoch += 1;
        assert_ne!(
            flight.edit_epoch, epoch,
            "the completion sees the bump and keeps dirty set"
        );
    }

    /// A save started on flight A must not mutate flight B: every install
    /// gets a fresh instance identity, and the completion guard rejects a
    /// completion captured on a different instance — B keeps its own path
    /// and dirty flag even when the epochs happen to collide (they restart
    /// at zero per document and are meaningless across instances).
    #[test]
    fn save_completion_rejects_a_replaced_document() {
        let mut a = OpenFlight::new(FlightDoc::new("A"), None);
        a.write_seq += 1;
        let (instance, seq) = (a.instance_id, a.write_seq);
        assert!(a.accepts_save_completion(instance, seq));

        let b = OpenFlight::new(FlightDoc::new("B"), None);
        assert_ne!(
            b.instance_id, a.instance_id,
            "every install gets a fresh identity"
        );
        assert!(
            !b.accepts_save_completion(instance, seq),
            "A's save completion must not adopt its path onto B"
        );
    }

    /// Two writes racing on the same flight (rapid Save As… to different
    /// paths): only the latest-*started* write may adopt its path —
    /// regardless of which completion lands last.
    #[test]
    fn only_the_latest_started_write_adopts_its_path() {
        let mut flight = OpenFlight::new(FlightDoc::new("x"), None);
        flight.write_seq += 1;
        let first = flight.write_seq;
        // A second Save As… starts before the first write completes.
        flight.write_seq += 1;
        let second = flight.write_seq;

        let id = flight.instance_id;
        assert!(
            !flight.accepts_save_completion(id, first),
            "the stale write must not win, in either completion order"
        );
        assert!(flight.accepts_save_completion(id, second));
    }

    /// A note typed during a save must keep the dirty flag set even though
    /// the fast path claims no compute generation.
    #[test]
    fn notes_edits_race_saves_via_the_edit_epoch() {
        let mut flight = OpenFlight::new(FlightDoc::new("x"), None);
        flight.doc.route = vec![RouteWaypoint::new(RoutePoint::Free(FreePoint {
            name: Some("A".to_owned()),
            position: LatLon::new(50.0, 8.0).unwrap(),
        }))];

        let save_epoch = flight.edit_epoch;
        assert!(flight.apply_notes_only_edit(0, "racing note".to_owned()));
        assert_ne!(
            flight.edit_epoch, save_epoch,
            "the save completion must not clear dirty"
        );
    }

    // --- the notes-only fast path -------------------------------------------

    /// Editing a nav-log note marks dirty and bumps the edit epoch but
    /// never claims a compute generation — the recompute is skipped, an
    /// in-flight run for an earlier edit still lands.
    #[test]
    fn notes_only_edits_skip_the_compute_generation() {
        let mut flight = OpenFlight::new(FlightDoc::new("x"), None);
        flight.doc.route = vec![
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: Some("A".to_owned()),
                position: LatLon::new(50.0, 8.0).unwrap(),
            })),
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: Some("B".to_owned()),
                position: LatLon::new(50.0, 9.0).unwrap(),
            })),
        ];

        // A regular edit scheduled a compute that is still in flight.
        let generation = flight.compute_generation.schedule();
        let epoch_before = flight.edit_epoch;

        assert!(flight.apply_notes_only_edit(0, "remember the photo stop".to_owned()));
        assert!(flight.dirty, "notes edits still mark the document dirty");
        assert_eq!(flight.edit_epoch, epoch_before + 1);
        assert_eq!(flight.doc.route[0].notes, "remember the photo stop");
        assert_eq!(
            flight.compute_generation.current(),
            generation,
            "no compute generation claimed"
        );
        assert!(
            flight.compute_generation.try_apply(generation),
            "the in-flight run for the earlier edit still lands"
        );

        // No-op edits (same text, bad index) change nothing at all.
        let epoch = flight.edit_epoch;
        assert!(!flight.apply_notes_only_edit(0, "remember the photo stop".to_owned()));
        assert!(!flight.apply_notes_only_edit(7, "out of range".to_owned()));
        assert_eq!(flight.edit_epoch, epoch);
    }
}

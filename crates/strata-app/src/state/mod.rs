//! Application state: the local store, dataset/AIRAC bookkeeping, live
//! weather, the gridded-weather time slider, and the current selection.
//!
//! Weather runs through [`gpui_tokio`]: the aviationweather provider is
//! reqwest-based and needs a tokio reactor, which gpui's executor is not.
//! `Tokio::spawn_result` hands the fetch to the bridge runtime and the
//! result is awaited from a normal gpui foreground task.

pub mod briefing;
pub mod flight;
pub mod ingest;
pub mod ingest_progress;
pub mod weather_time;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use strata_data::domain::{
    AiracCycle, BoundingBox, Country, IcaoCode, LatLon, Metar, Meters, Sigmet, Taf,
    weather_bboxes,
};
use strata_data::providers::aviationweather::{AviationWeatherClient, CachedWeatherProvider};
use strata_data::providers::{WeatherProvider as _, WeatherQuery};
use strata_data::store::{Dataset, DatasetMeta, Feature, Store};
use gpui::{AppContext as _, Context, EventEmitter, Task};
use gpui_tokio::Tokio;

use crate::config::Config;
use crate::tile_sources::MbTilesSource;
// `OpenFlight`/`ComputeState` are what the planning panels read off
// `AppState::flight`; re-exported alongside the mutation API in `flight`.
#[allow(unused_imports)]
pub use flight::{ComputeState, OpenFlight};
use flight::winds::FlightWinds;
// `IngestDataset`/`IngestRunResult` are re-exported for the settings modal
// (next phase) — the manual run/cancel/last-result API on `AppState`.
#[allow(unused_imports)]
pub use ingest::{IngestDataset, IngestNotice, IngestRunResult, NoticeLevel};
use ingest::IngestManager;
use ingest_progress::IngestProgressVm;
use weather_time::WeatherTime;

/// Cadence of the dataset-meta / late-store / late-basemap re-check, so the
/// UI picks up data produced by an out-of-process `strata-ingest` run without
/// a restart (in-process runs trigger the reload directly on completion).
pub const DATA_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

const STORE_FILE: &str = "store.sqlite";
const BASEMAP_FILE: &str = strata_data::paths::BASEMAP_FILE;

/// Typed state-change notifications; views subscribe to react to specific
/// changes instead of observing every notify.
#[derive(Debug, Clone, PartialEq)]
pub enum AppStateEvent {
    /// METARs/TAFs/SIGMETs replaced (or a fetch failed).
    WeatherUpdated,
    /// Airport positions for weather stations finished loading.
    StationsLoaded,
    /// Store / basemap / dataset meta changed after an ingest finished while
    /// the app was running.
    DataReloaded,
    SelectionChanged,
    /// The gridded-weather time selection moved (slider drag, ± buttons).
    WeatherTimeChanged,
    /// A fetch cycle re-anchored the slider window at a fresh "now". The
    /// UI re-syncs the thumb, but no fetch follow-up is scheduled — the
    /// cycle that re-anchored already plans against the shifted window
    /// (a follow-up would cancel it mid-flight).
    WeatherTimeReanchored,
    /// The ingest progress view-model changed (see
    /// [`AppState::update_ingest_progress`]); the progress panel re-renders
    /// and re-evaluates its mount animation.
    IngestProgressChanged,
    /// A user-facing ingest message (run finished/failed/cancelled, key
    /// missing, run rejected); the root view shows it as a window
    /// notification (state has no `Window`).
    IngestNotice(IngestNotice),
    /// A flight document was installed ([`AppState::flight`] is now `Some`)
    /// — planning mode begins. Replacing an open flight emits
    /// [`Self::FlightClosed`] first.
    FlightOpened,
    /// The open flight's document, dirty flag or path changed. Document
    /// mutations also schedule the debounced background compute (see
    /// `state::flight::compute`).
    FlightChanged,
    /// A compute run landed: [`OpenFlight::computed`] /
    /// [`OpenFlight::compute_state`] are current for the document
    /// generation (stale runs are dropped, never emitted).
    FlightComputed,
    /// The flight was closed — planning mode ends, explorer untouched.
    FlightClosed,
    /// The profile scrub position changed ([`AppState::profile_scrub`]).
    /// The map view re-pushes the route (the renderer repositions the
    /// scrub marker from its retained geometry); the profile drawer moves
    /// its crosshair — both surfaces sync through this one event.
    ProfileScrubChanged,
    /// The hovered route point changed ([`AppState::route_highlight`] —
    /// the flight panel's row hover). The map view re-pushes the route;
    /// the renderer emphasizes the matching handle from its retained
    /// geometry (an instance/uniform change, never a re-tessellation).
    RouteHighlightChanged,
    /// The corridor outline visibility ([`AppState::corridor_visible`]) or
    /// the configured corridor half-width changed. The map view re-pushes
    /// the route with the new `corridor_halfwidth_m`; the drawer re-renders
    /// its header controls. (A width change additionally reschedules the
    /// compute — the new corridor lands via [`Self::FlightComputed`].)
    CorridorChanged,
    /// The profile weather-overlay toggles ([`AppState::profile_weather`])
    /// changed — the profile view rebuilds its scene, the drawer header
    /// re-renders its toggle buttons.
    ProfileWeatherChanged,
    /// A flight-panel badge asked to navigate to its cause (design §3.1).
    /// The profile drawer expands onto its Profile tab, the context panel
    /// switches to the requested tab, the map flies to the conflict — each
    /// surface reacts to this one event, none reaches into another.
    PlanningFocusRequested(flight::PlanningFocus),
    /// The aircraft profile library changed (profile edited/created/deleted
    /// in the aircraft manager, or the startup load finished). Open flights
    /// referencing a changed profile recompute automatically.
    AircraftLibraryChanged,
    /// The briefing surface changed: the NOTAM fetch state
    /// ([`AppState::notam_fetch`]) moved, or the derived relevance list on
    /// [`OpenFlight::briefing`] was recomputed (snapshot refresh / compute
    /// landing). The Briefing tab and the flight-panel NOTAM badge
    /// re-render on this one event.
    BriefingChanged,
    /// A file export finished (the root view shows it as a window
    /// notification, like [`Self::IngestNotice`]).
    ExportFinished(briefing::ExportNotice),
}

/// The profile drawer's weather-overlay toggles (design §3.3). Session
/// view state — deliberately not in the config: a fresh launch starts with
/// both overlays on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileWeatherOverlays {
    /// The freezing-level line (per-leg ISA estimate from leg winds).
    pub freezing: bool,
    /// The forecast cloud-base band (see
    /// `ui::profile_view::ProfileSeries::cloud_base_m` for the data seam).
    pub cloud_base: bool,
}

impl Default for ProfileWeatherOverlays {
    fn default() -> Self {
        Self {
            freezing: true,
            cloud_base: true,
        }
    }
}

#[derive(Default)]
pub struct WeatherState {
    pub metars: HashMap<IcaoCode, Metar>,
    pub tafs: HashMap<IcaoCode, Taf>,
    pub sigmets: Vec<Sigmet>,
    pub last_fetched_at: Option<Instant>,
    pub fetching: bool,
    pub last_error: Option<String>,
}

pub struct AppState {
    /// The user configuration loaded at startup (config file + clamping).
    /// Mutated by the UI (theme toggle, later the settings modal); writers
    /// persist via `Config::save_if_changed`.
    pub config: Config,
    pub data_dir: PathBuf,
    /// `None` when the store failed to open (see `store_error`).
    pub store: Option<Arc<Store>>,
    /// A second connection to the same store, dedicated to terrain-tile
    /// reads so they never queue behind the feature feed / hit-test / search
    /// queries serializing on `store`'s connection mutex (WAL supports
    /// multiple connections on one path). Falls back to sharing `store`.
    pub terrain_store: Option<Arc<Store>>,
    /// A third connection, dedicated to the flight compute's bulk reads
    /// (elevation prefetch, corridor airspace/obstacle queries) so a
    /// per-keystroke compute never stalls the UI's hit-test/search — nor
    /// vice versa. Falls back to sharing `store`.
    pub compute_store: Option<Arc<Store>>,
    pub store_error: Option<String>,
    /// Vector basemap archive; `None` until a basemap ingest ran.
    pub basemap: Option<Arc<MbTilesSource>>,
    pub dataset_meta: HashMap<Dataset, DatasetMeta>,
    pub weather: WeatherState,
    /// Anchor + selection of the gridded-weather time slider.
    pub weather_time: WeatherTime,
    /// What the ingest progress panel shows; populated by the ingest
    /// orchestration through [`Self::update_ingest_progress`].
    pub ingest_progress: IngestProgressVm,
    /// Features under the last map click; stacked airspaces are the norm.
    pub selection: Vec<Feature>,
    /// Latest camera pose, pushed by the map view for the status bar.
    pub camera: Option<strata_render::CameraSnapshot>,
    /// Cursor position as `(lat, lon)` degrees.
    pub cursor: Option<(f64, f64)>,
    /// UI mode driven by the title-bar sun/moon toggle. Dark applies the
    /// "Oldworld" gpui theme, light "Pastel Light" (defaults: dark).
    pub dark_mode: bool,
    /// Id of the renderer's active [`strata_render::MapTheme`]. Follows the
    /// configured resolution (`auto` = the map theme named after the active
    /// UI theme, mode default as fallback) until the layers-panel picker
    /// overrides it; the next mode toggle re-applies the configured
    /// resolution (see `ui::theme`).
    pub map_theme_id: &'static str,
    /// The open flight document; `Some` = planning mode (see
    /// `state::flight` for the mutation API and events).
    pub flight: Option<OpenFlight>,
    /// Profile-drawer scrub position along the open flight's track, meters
    /// from departure; `None` = no scrub cursor. View state shared between
    /// the drawer and the map marker — never document state (see
    /// [`Self::set_profile_scrub`]). Planning mode only; flight open/close
    /// resets it.
    pub profile_scrub: Option<Meters>,
    /// The route point id (the [`RenderRoute`] vertex-id contract — route
    /// index, or `ALTERNATE_ID_BASE + i` for alternates) whose map handle
    /// is hover-emphasized; set/cleared by the flight panel's row hover.
    /// View state like [`Self::profile_scrub`] — never document state.
    /// Planning mode only; flight open/close resets it.
    ///
    /// [`RenderRoute`]: strata_render::features::RenderRoute
    pub route_highlight: Option<u64>,
    /// Whether the map draws the profile corridor outline around the track
    /// (design §3.2, the drawer header's eye toggle). View state, not
    /// document state — the route push fills `corridor_halfwidth_m` from
    /// the computed corridor while this is on.
    pub corridor_visible: bool,
    /// The profile's weather-overlay toggles (design §3.3 "weather
    /// overlays (toggle)"): freezing-level line and forecast cloud-base
    /// band. Drawer-header view state like [`Self::corridor_visible`] —
    /// session-only, never persisted; both default on.
    pub profile_weather: ProfileWeatherOverlays,
    /// Loaded aircraft profiles (`<data_dir>/aircraft/*.strata-aircraft`),
    /// sorted by id; seeded with two examples on first run.
    pub aircraft_library: Vec<strata_plan::AircraftProfile>,
    /// In-flight NOTAM fetch bookkeeping (spinner flag, last error,
    /// staleness generation; see `state::briefing`).
    pub notam_fetch: briefing::NotamFetchState,
    /// A briefing-PDF render is running on the background executor (the
    /// export button's spinner; see [`AppState::export_briefing_pdf`]).
    pub pdf_exporting: bool,
    /// The NOTAM source behind every briefing fetch — the autorouter
    /// client when the config carries credentials, `None` otherwise (the
    /// Briefing tab renders its not-configured state; the wiring site is
    /// [`briefing::build_notam_provider`], rebuilt on credential changes
    /// via [`AppState::rebuild_notam_provider`]).
    notam_provider: Option<std::sync::Arc<dyn strata_data::providers::NotamProvider>>,
    /// The running NOTAM fetch (replacing it cancels the previous one).
    notam_fetch_task: Option<Task<()>>,
    /// One-run-at-a-time ingest slot + last result (see `state::ingest`).
    ingest: IngestManager,
    /// Debounced background compute for the open flight (replacing it
    /// cancels the previous debounce/run).
    flight_compute_task: Option<Task<()>>,
    /// Winds-aloft prefetch cache + task (see `state::flight::winds`).
    flight_winds: FlightWinds,
    /// Trailing-edge-debounced aircraft-profile disk write (the editor
    /// commits per keystroke; see [`AppState::upsert_aircraft_profile`]).
    /// Flushed on profile switch and on app quit.
    pending_aircraft_save: Option<flight::PendingAircraftSave>,
    station_positions: Arc<HashMap<IcaoCode, LatLon>>,
    provider: Arc<CachedWeatherProvider<AviationWeatherClient>>,
    weather_task: Option<Task<()>>,
    data_refresh_task: Option<Task<()>>,
}

impl EventEmitter<AppStateEvent> for AppState {}

impl AppState {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let data_dir = resolve_data_dir(config.data_dir.as_deref());
        tracing::info!(data_dir = %data_dir.display(), "using data dir");

        let (store, store_error) = match Store::open(&data_dir.join(STORE_FILE)) {
            Ok(store) => (Some(Arc::new(store)), None),
            Err(err) => {
                tracing::error!(%err, "failed to open store");
                (None, Some(err.to_string()))
            }
        };
        let terrain_store = store
            .as_ref()
            .map(|shared| open_extra_store_connection(&data_dir.join(STORE_FILE), shared, "terrain"));
        let compute_store = store
            .as_ref()
            .map(|shared| open_extra_store_connection(&data_dir.join(STORE_FILE), shared, "compute"));

        // One-shot rename of a pre-multi-country archive
        // (basemap-de.mbtiles → basemap.mbtiles) before opening it.
        strata_data::paths::migrate_legacy_basemap(&data_dir);
        let basemap_path = data_dir.join(BASEMAP_FILE);
        let basemap = match MbTilesSource::open(&basemap_path) {
            Ok(source) => Some(Arc::new(source)),
            Err(err) => {
                tracing::warn!(
                    path = %basemap_path.display(),
                    %err,
                    "basemap archive unavailable — map renders without a basemap"
                );
                None
            }
        };

        let dataset_meta = store.as_deref().map(load_dataset_meta).unwrap_or_default();

        let dark_mode = config.mode.is_dark();
        let map_theme_id = resolve_map_theme_id(&config);
        let notam_provider = briefing::build_notam_provider(&config);

        let mut this = Self {
            config,
            data_dir,
            store,
            terrain_store,
            compute_store,
            store_error,
            basemap,
            dataset_meta,
            weather: WeatherState::default(),
            weather_time: WeatherTime::new(chrono::Utc::now()),
            ingest_progress: IngestProgressVm::default(),
            selection: Vec::new(),
            camera: None,
            cursor: None,
            dark_mode,
            map_theme_id,
            flight: None,
            profile_scrub: None,
            route_highlight: None,
            corridor_visible: false,
            profile_weather: ProfileWeatherOverlays::default(),
            aircraft_library: Vec::new(),
            notam_fetch: briefing::NotamFetchState::default(),
            pdf_exporting: false,
            notam_provider,
            notam_fetch_task: None,
            ingest: IngestManager::default(),
            flight_compute_task: None,
            flight_winds: FlightWinds::new(),
            pending_aircraft_save: None,
            station_positions: Arc::new(HashMap::new()),
            provider: Arc::new(CachedWeatherProvider::with_default_ttls(
                AviationWeatherClient::new(),
            )),
            weather_task: None,
            data_refresh_task: None,
        };
        this.load_station_positions(cx);
        this.start_weather_loop(cx);
        this.start_data_refresh_loop(cx);
        this.start_auto_ingest(cx);
        this.reload_aircraft_library(cx);
        // Quit-time flush of the debounced aircraft write: the future is
        // polled through gpui's shutdown window, so the last keystrokes
        // reach disk even when the app quits inside the debounce window.
        cx.on_app_quit(|this, cx| this.flush_pending_aircraft_save_on_quit(cx))
            .detach();
        this
    }

    /// Mutate the ingest-progress view-model and notify the UI (emits
    /// [`AppStateEvent::IngestProgressChanged`]). The ingest orchestration
    /// drives the progress panel exclusively through this method, e.g.
    /// `state.update_ingest_progress(cx, |vm| vm.job_progress(job, done, detail))`.
    pub fn update_ingest_progress<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut IngestProgressVm) -> R,
    ) -> R {
        let result = f(&mut self.ingest_progress);
        cx.emit(AppStateEvent::IngestProgressChanged);
        cx.notify();
        result
    }

    /// Persists the current config off the UI thread: diff-aware atomic
    /// write, with the ordering ticket captured *here*, together with the
    /// snapshot — so a stale snapshot completing late can never overwrite
    /// a newer one (detached background writes have no completion-order
    /// guarantee). Failure only warns; the in-memory change already
    /// happened. `what` names the changed value for the log.
    pub(crate) fn persist_config(&self, what: &'static str, cx: &mut Context<Self>) {
        let config = self.config.clone();
        let ticket = crate::fsutil::WriteTicket::next();
        cx.background_spawn(async move {
            if let Err(err) = config.save_if_changed_ordered(ticket) {
                tracing::warn!(%err, what, "failed to persist config");
            }
        })
        .detach();
    }

    /// The enabled-country set (normalized; may be empty = nothing
    /// auto-ingested). Scopes ingestion and the live METAR/TAF fetch area
    /// — never rendering: the map stays viewport-driven over whatever the
    /// store holds.
    // The settings modal's read side; internal consumers go through
    // `config.enabled_countries()` directly.
    #[allow(dead_code)]
    pub fn enabled_countries(&self) -> Vec<Country> {
        self.config.enabled_countries()
    }

    /// Replaces the enabled-country set (the settings Countries page).
    /// Persists the config, re-scopes the live weather fetch on the spot
    /// (the TTL cache keys by bbox, so no invalidation is needed; a fetch
    /// already in flight finishes with the old scope and the next cycle
    /// corrects it), and — `config.ingest.auto` permitting — inspects the
    /// data dir and downloads whatever the new set is missing (one run at
    /// a time as always; with auto off the manual download buttons cover
    /// it, they run the enabled set too). An empty set is legal: nothing
    /// gets auto-ingested, existing data stays on disk untouched. Returns
    /// whether the set changed.
    pub fn set_enabled_countries(&mut self, countries: Vec<Country>, cx: &mut Context<Self>) -> bool {
        let next = crate::config::normalize_countries(countries);
        if next == self.config.enabled_countries() {
            return false;
        }
        tracing::info!(countries = ?next, "enabled countries changed");
        self.config.countries = next;
        self.persist_config("enabled countries", cx);
        self.fetch_weather(cx);
        if self.config.ingest.auto {
            self.spawn_inspect_then_ingest(true, cx);
        }
        cx.notify();
        true
    }

    /// Whether any aeronautical dataset has ever been ingested.
    pub fn has_data(&self) -> bool {
        [
            Dataset::Airspaces,
            Dataset::Airports,
            Dataset::Navaids,
            Dataset::ReportingPoints,
            Dataset::Obstacles,
        ]
        .iter()
        .any(|d| self.dataset_meta.contains_key(d))
    }

    /// AIRAC cycle of the ingested data (airspaces preferred as canonical).
    pub fn airac(&self) -> Option<&AiracCycle> {
        self.dataset_meta
            .get(&Dataset::Airspaces)
            .and_then(|m| m.airac.as_ref())
            .or_else(|| self.dataset_meta.values().find_map(|m| m.airac.as_ref()))
    }

    pub fn airac_stale(&self) -> bool {
        self.airac().is_some_and(|a| a.is_stale())
    }

    pub fn station_position(&self, station: &IcaoCode) -> Option<LatLon> {
        self.station_positions.get(station).copied()
    }

    pub fn metar_for(&self, station: &IcaoCode) -> Option<&Metar> {
        self.weather.metars.get(station)
    }

    pub fn taf_for(&self, station: &IcaoCode) -> Option<&Taf> {
        self.weather.tafs.get(station)
    }

    pub fn set_selection(&mut self, features: Vec<Feature>, cx: &mut Context<Self>) {
        self.selection = features;
        cx.emit(AppStateEvent::SelectionChanged);
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.set_selection(Vec::new(), cx);
        }
    }

    // --- profile scrub ---------------------------------------------------------

    /// Sets (or clears) the profile-drawer scrub position — meters along
    /// track from departure. The single sync point between the drawer and
    /// the map (design §3.3 "scrub cursor synced with the map marker"):
    /// both surfaces write here and follow
    /// [`AppStateEvent::ProfileScrubChanged`]; the map view pushes the
    /// value into the renderer route's `scrub_along_m`. Outside planning
    /// mode any value collapses to `None`; identical values are a no-op.
    pub fn set_profile_scrub(&mut self, along_track: Option<Meters>, cx: &mut Context<Self>) {
        if update_profile_scrub(&mut self.profile_scrub, along_track, self.flight.is_some()) {
            cx.emit(AppStateEvent::ProfileScrubChanged);
            cx.notify();
        }
    }

    // --- route hover highlight ---------------------------------------------------

    /// Sets (or clears) the hover-emphasized route point — the
    /// [`RenderRoute`] vertex id the flight panel's hovered row maps to.
    /// The map view follows [`AppStateEvent::RouteHighlightChanged`] and
    /// pushes the value into the renderer route's `highlight`. Outside
    /// planning mode any value collapses to `None`; identical values are
    /// a no-op.
    ///
    /// [`RenderRoute`]: strata_render::features::RenderRoute
    pub fn set_route_highlight(&mut self, id: Option<u64>, cx: &mut Context<Self>) {
        if update_route_highlight(&mut self.route_highlight, id, self.flight.is_some()) {
            cx.emit(AppStateEvent::RouteHighlightChanged);
            cx.notify();
        }
    }

    /// Clears the highlight only while it still belongs to `id`: row
    /// hover-leave events can arrive after the next row's hover-enter, and
    /// a stale leave must not clobber the fresher highlight.
    pub fn clear_route_highlight(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.route_highlight == Some(id) {
            self.set_route_highlight(None, cx);
        }
    }

    /// Persists the profile drawer's drag-resized height (config
    /// `[profile_drawer] height_px`). Called on drag release; the
    /// in-memory value updates immediately, the write is diff-aware and
    /// off the UI thread (the [`Self::set_dark_mode`] recipe). No event:
    /// the drawer already renders the new height — config is only the
    /// restart memory.
    pub fn set_profile_drawer_height(&mut self, height_px: f32, cx: &mut Context<Self>) {
        let height_px = if height_px.is_finite() {
            height_px.clamp(
                *crate::config::PROFILE_DRAWER_HEIGHT_RANGE.start(),
                *crate::config::PROFILE_DRAWER_HEIGHT_RANGE.end(),
            )
        } else {
            return;
        };
        if self.config.profile_drawer.height_px == height_px {
            return;
        }
        self.config.profile_drawer.height_px = height_px;
        self.persist_config("profile drawer height", cx);
    }

    /// Sets the profile's weather-overlay toggles (the drawer header's
    /// freezing / cloud-base buttons). Emits
    /// [`AppStateEvent::ProfileWeatherChanged`]; identical values are a
    /// no-op.
    pub fn set_profile_weather(
        &mut self,
        overlays: ProfileWeatherOverlays,
        cx: &mut Context<Self>,
    ) {
        if self.profile_weather == overlays {
            return;
        }
        self.profile_weather = overlays;
        cx.emit(AppStateEvent::ProfileWeatherChanged);
        cx.notify();
    }

    // --- corridor (drawer header controls) -------------------------------------

    /// Shows/hides the map's corridor outline (design §3.2; the drawer
    /// header's eye toggle). Emits [`AppStateEvent::CorridorChanged`] — the
    /// map view re-pushes the route with/without `corridor_halfwidth_m`.
    pub fn set_corridor_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.corridor_visible == visible {
            return;
        }
        self.corridor_visible = visible;
        cx.emit(AppStateEvent::CorridorChanged);
        cx.notify();
    }

    /// Sets the corridor half-width the profile is computed over (design
    /// §3.3 "configurable ±2–5 NM"; config `[profile_drawer]
    /// corridor_half_width_nm`, persisted like the drawer height) and
    /// reschedules the open flight's compute with the new
    /// [`strata_plan::compute::ComputeParams`]. The corridor outline and
    /// the profile silhouette both update when that run lands.
    pub fn set_corridor_half_width_nm(&mut self, half_width_nm: f64, cx: &mut Context<Self>) {
        if !half_width_nm.is_finite() {
            return;
        }
        let half_width_nm = half_width_nm.clamp(
            *crate::config::CORRIDOR_HALF_WIDTH_NM_RANGE.start(),
            *crate::config::CORRIDOR_HALF_WIDTH_NM_RANGE.end(),
        );
        if self.config.profile_drawer.corridor_half_width_nm == half_width_nm {
            return;
        }
        self.config.profile_drawer.corridor_half_width_nm = half_width_nm;
        self.persist_config("corridor half-width", cx);
        cx.emit(AppStateEvent::CorridorChanged);
        cx.notify();
        if self.flight.is_some() {
            self.schedule_flight_compute(cx);
        }
    }

    /// Renderer theme id the current config implies (`map_theme = "auto"`
    /// follows the active UI theme by name, mode default as fallback). The
    /// title-bar mode toggle and the settings modal both re-resolve through
    /// this after changing mode, UI theme or map theme.
    pub fn resolved_map_theme_id(&self) -> &'static str {
        resolve_map_theme_id(&self.config)
    }

    /// Switch dark/light mode and persist the choice (config `mode`) so it
    /// survives restarts. The gpui theme swap itself stays with the caller
    /// (it needs the `Window`); map-theme defaults live in `ui::theme`.
    pub fn set_dark_mode(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.dark_mode = dark;
        self.config.mode = if dark {
            crate::config::ThemeMode::Dark
        } else {
            crate::config::ThemeMode::Light
        };
        self.persist_config("theme mode", cx);
        cx.notify();
    }

    pub fn set_camera(&mut self, snapshot: strata_render::CameraSnapshot, cx: &mut Context<Self>) {
        if self.camera != Some(snapshot) {
            self.camera = Some(snapshot);
            cx.notify();
        }
    }

    /// Cursor geo position, quantized to the displayed precision so mouse
    /// movement doesn't notify more often than the status bar can change.
    pub fn set_cursor(&mut self, lat: f64, lon: f64, cx: &mut Context<Self>) {
        let quantized = (
            (lat * 10_000.0).round() / 10_000.0,
            (lon * 10_000.0).round() / 10_000.0,
        );
        if self.cursor != Some(quantized) {
            self.cursor = Some(quantized);
            cx.notify();
        }
    }

    // --- gridded-weather time slider -----------------------------------------

    /// Slider drag/click: set the selection from a minute offset relative to
    /// the anchor (clamped into the −2 h … +24 h window).
    pub fn set_weather_time_offset(&mut self, minutes: f32, cx: &mut Context<Self>) {
        if self.weather_time.set_offset_minutes(minutes) {
            cx.emit(AppStateEvent::WeatherTimeChanged);
            cx.notify();
        }
    }

    /// Step the slider selection by whole hours (the ± buttons).
    pub fn step_weather_time(&mut self, hours: i64, cx: &mut Context<Self>) {
        if self.weather_time.step(chrono::Duration::hours(hours)) {
            cx.emit(AppStateEvent::WeatherTimeChanged);
            cx.notify();
        }
    }

    /// Re-anchor the slider window at the current wall clock (called by each
    /// gridded-weather fetch cycle; a no-op while the anchor has drifted
    /// less than one slider step). Emits [`AppStateEvent::WeatherTimeReanchored`]
    /// — not `WeatherTimeChanged` — so the map view doesn't schedule a
    /// follow-up cycle that would cancel the very cycle re-anchoring.
    pub fn re_anchor_weather_time(&mut self, cx: &mut Context<Self>) {
        if self.weather_time.re_anchor(chrono::Utc::now()) {
            cx.emit(AppStateEvent::WeatherTimeReanchored);
            cx.notify();
        }
    }

    /// Manual refresh: drop the TTL cache, fetch immediately.
    pub fn refresh_weather(&mut self, cx: &mut Context<Self>) {
        self.provider.invalidate();
        self.fetch_weather(cx);
    }

    /// Cadence of the background METAR/TAF/SIGMET refresh, straight from
    /// config so a settings-modal change applies on the next cycle.
    fn weather_refresh_interval(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.weather.refresh_minutes) * 60)
    }

    fn start_weather_loop(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                // Re-read the interval every cycle so a config change (the
                // settings modal, later) applies without restarting the loop.
                let Ok(interval) = this.update(cx, |this, cx| {
                    this.fetch_weather(cx);
                    this.weather_refresh_interval()
                }) else {
                    break; // app state dropped
                };
                cx.background_executor().timer(interval).await;
            }
        })
        .detach();
    }

    fn fetch_weather(&mut self, cx: &mut Context<Self>) {
        if self.weather.fetching {
            return;
        }
        self.weather.fetching = true;
        cx.notify();

        let provider = self.provider.clone();
        let countries = self.config.enabled_countries();
        let fetch = Tokio::spawn_result(cx, async move {
            // METAR/TAF scope follows the enabled countries, chunked into
            // thinning-safe boxes (aviationweather.gov silently thins
            // stations for oversized boxes — see
            // `strata_data::domain::weather_bboxes`): one request per box,
            // stations from overlapping boxes merged by id.
            let boxes = weather_bboxes(&countries);
            let mut metars: Vec<Metar> = Vec::new();
            let mut tafs: Vec<Taf> = Vec::new();
            for bbox in &boxes {
                merge_by_key(&mut metars, provider.metars(WeatherQuery::Bbox(*bbox)).await?, |m| {
                    &m.station
                });
                merge_by_key(&mut tafs, provider.tafs(WeatherQuery::Bbox(*bbox)).await?, |t| {
                    &t.station
                });
            }
            // SIGMETs ride one union box: the upstream feed is global and
            // only client-filtered, so box size costs nothing.
            let sigmets = match Country::union_bbox(&countries) {
                Some(bbox) => provider.sigmets(bbox).await?,
                None => Vec::new(),
            };
            anyhow::Ok((metars, tafs, sigmets))
        });
        self.weather_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                this.weather.fetching = false;
                match result {
                    Ok((metars, tafs, sigmets)) => {
                        tracing::info!(
                            metars = metars.len(),
                            tafs = tafs.len(),
                            sigmets = sigmets.len(),
                            "weather updated"
                        );
                        this.weather.metars =
                            metars.into_iter().map(|m| (m.station.clone(), m)).collect();
                        this.weather.tafs =
                            tafs.into_iter().map(|t| (t.station.clone(), t)).collect();
                        this.weather.sigmets = sigmets;
                        this.weather.last_fetched_at = Some(Instant::now());
                        this.weather.last_error = None;
                    }
                    Err(err) => {
                        tracing::warn!(%err, "weather fetch failed");
                        this.weather.last_error = Some(err.to_string());
                    }
                }
                cx.emit(AppStateEvent::WeatherUpdated);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Periodically re-check what an ingest (in-process or the CLI) may have
    /// produced while the app runs (mirrors [`Self::start_weather_loop`]).
    /// Startup already loaded everything, so the first check waits one
    /// interval.
    fn start_data_refresh_loop(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DATA_REFRESH_INTERVAL).await;
                if this.update(cx, |this, cx| this.refresh_data(cx)).is_err() {
                    break; // app state dropped
                }
            }
        })
        .detach();
    }

    /// Reload everything [`AppState::new`] snapshotted from disk: retry a
    /// failed store open, pick up a basemap archive that appeared later, and
    /// re-read dataset meta (banner, AIRAC chip). Emits
    /// [`AppStateEvent::DataReloaded`] when anything changed.
    fn refresh_data(&mut self, cx: &mut Context<Self>) {
        let store_path = self.data_dir.join(STORE_FILE);
        let basemap_path = self.data_dir.join(BASEMAP_FILE);
        let existing_store = self.store.clone();
        let need_basemap = self.basemap.is_none();
        // Replacing the task drops a still-running previous refresh.
        self.data_refresh_task = Some(cx.spawn(async move |this, cx| {
            // All IO off the UI thread.
            let refreshed = cx
                .background_spawn(async move {
                    let opened_store = if existing_store.is_some() {
                        None
                    } else {
                        match Store::open(&store_path) {
                            Ok(store) => {
                                let store = Arc::new(store);
                                let terrain =
                                    open_extra_store_connection(&store_path, &store, "terrain");
                                let compute =
                                    open_extra_store_connection(&store_path, &store, "compute");
                                Some((store, terrain, compute))
                            }
                            Err(err) => {
                                // Expected until the first ingest; debug to
                                // avoid log spam every interval.
                                tracing::debug!(%err, "store still unavailable");
                                None
                            }
                        }
                    };
                    let store = opened_store
                        .as_ref()
                        .map(|(store, _, _)| store)
                        .or(existing_store.as_ref());
                    let meta = store.map(|store| load_dataset_meta(store));
                    let basemap = if need_basemap {
                        match MbTilesSource::open(&basemap_path) {
                            Ok(source) => Some(Arc::new(source)),
                            Err(err) => {
                                tracing::debug!(%err, "basemap archive still unavailable");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    (opened_store, meta, basemap)
                })
                .await;
            this.update(cx, |this, cx| {
                let (opened_store, meta, basemap) = refreshed;
                let mut changed = false;
                if let Some((store, terrain, compute)) = opened_store {
                    this.store = Some(store);
                    this.terrain_store = Some(terrain);
                    this.compute_store = Some(compute);
                    this.store_error = None;
                    changed = true;
                }
                if let Some(basemap) = basemap {
                    this.basemap = Some(basemap);
                    changed = true;
                }
                if let Some(meta) = meta
                    && meta != this.dataset_meta
                {
                    let reload_stations = stations_need_reload(
                        &this.dataset_meta,
                        &meta,
                        !this.station_positions.is_empty(),
                    );
                    this.dataset_meta = meta;
                    changed = true;
                    if reload_stations {
                        this.load_station_positions(cx);
                    }
                }
                if changed {
                    tracing::info!("ingested data changed on disk; state reloaded");
                    // An ingest may have rewritten elevation tiles through
                    // the same WAL path; the open flight's decoded tile
                    // cache would serve stale terrain — drop it and
                    // recompute over the fresh data.
                    if let Some(flight) = &mut this.flight {
                        flight.elevation_cache = None;
                    }
                    if this.flight.is_some() {
                        this.schedule_flight_compute(cx);
                    }
                    cx.emit(AppStateEvent::DataReloaded);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Airport positions resolve METAR stations to map dots; loaded once in
    /// the background (the airports table can hold thousands of rows).
    fn load_station_positions(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let positions = cx
                .background_spawn(async move {
                    // Store-wide, not country-scoped: the map shows METAR
                    // dots for whatever airports the store holds.
                    match store.airports_in_bbox(world_bbox()) {
                        Ok(airports) => airports
                            .into_iter()
                            .filter_map(|a| Some((a.ident?, a.position)))
                            .collect::<HashMap<_, _>>(),
                        Err(err) => {
                            tracing::warn!(%err, "loading airport positions failed");
                            HashMap::new()
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                tracing::debug!(count = positions.len(), "station positions loaded");
                this.station_positions = Arc::new(positions);
                cx.emit(AppStateEvent::StationsLoaded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Pure core of [`AppState::set_profile_scrub`]: planning-mode gating
/// (`planning == false` collapses any value to `None`) plus change
/// detection. Returns whether the slot changed (= whether to emit).
fn update_profile_scrub(slot: &mut Option<Meters>, scrub: Option<Meters>, planning: bool) -> bool {
    let next = if planning { scrub } else { None };
    if *slot == next {
        return false;
    }
    *slot = next;
    true
}

/// The whole-world box. Store reads that must cover *everything the store
/// holds* — never a country subset — query with it: rendering and station
/// resolution are country-agnostic by design (country selection scopes
/// ingestion only).
pub(crate) fn world_bbox() -> BoundingBox {
    // Infallible: constant, valid WGS84 bounds.
    BoundingBox::new(-180.0, -90.0, 180.0, 90.0).expect("world bounds are valid")
}

/// Appends the elements of `new` whose key is not already present in
/// `into` (then dedupes within `new` itself too). The weather fetch merges
/// per-box station lists with it — overlapping boxes return the same
/// station twice; the first occurrence wins.
fn merge_by_key<T, K: Eq + std::hash::Hash + Clone>(
    into: &mut Vec<T>,
    new: Vec<T>,
    key: impl Fn(&T) -> &K,
) {
    let mut seen: std::collections::HashSet<K> = into.iter().map(|t| key(t).clone()).collect();
    for item in new {
        if seen.insert(key(&item).clone()) {
            into.push(item);
        }
    }
}

/// The route-highlight twin of [`update_profile_scrub`]: collapses to
/// `None` outside planning mode, dedupes identical values, reports whether
/// anything changed (the caller emits only then).
fn update_route_highlight(slot: &mut Option<u64>, id: Option<u64>, planning: bool) -> bool {
    let next = if planning { id } else { None };
    if *slot == next {
        return false;
    }
    *slot = next;
    true
}

/// An extra connection to the store at `path` dedicated to one consumer
/// (terrain-tile reads, the flight compute) so it doesn't serialize behind
/// the feature feed / hit-test / search on the shared connection. WAL
/// explicitly supports multiple connections per path; the migration re-run
/// is an idempotent no-op. Falls back to sharing `shared` so the consumer
/// still works if the extra open fails.
fn open_extra_store_connection(
    path: &std::path::Path,
    shared: &Arc<Store>,
    purpose: &'static str,
) -> Arc<Store> {
    match Store::open(path) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            tracing::warn!(%err, purpose, "extra store connection failed; sharing the main connection");
            Arc::clone(shared)
        }
    }
}

/// Whether a dataset-meta change requires reloading the station-position
/// table (weather dots resolve METAR stations through it): the airports
/// dataset was re-ingested, or positions never loaded and airports exist
/// now.
fn stations_need_reload(
    old: &HashMap<Dataset, DatasetMeta>,
    new: &HashMap<Dataset, DatasetMeta>,
    have_positions: bool,
) -> bool {
    old.get(&Dataset::Airports) != new.get(&Dataset::Airports)
        || (!have_positions && new.contains_key(&Dataset::Airports))
}

/// `$STRATA_DATA_DIR` (explicit per-invocation override; the pre-rename
/// variable still works, with a deprecation warning), else the config
/// file's `data_dir`, else `~/.local/share/strata`.
fn resolve_data_dir(config_override: Option<&Path>) -> PathBuf {
    use strata_data::paths;
    resolve_data_dir_from(
        paths::env_var_with_legacy(paths::DATA_DIR_ENV, paths::LEGACY_DATA_DIR_ENV),
        config_override,
    )
}

/// Pure core of [`resolve_data_dir`].
fn resolve_data_dir_from(
    env_override: Option<std::ffi::OsString>,
    config_override: Option<&Path>,
) -> PathBuf {
    if let Some(dir) = env_override {
        return PathBuf::from(dir);
    }
    if let Some(dir) = config_override {
        return dir.to_path_buf();
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(strata_data::paths::DIR_NAME)
}

/// Renderer theme id for the configured map theme: `Auto` follows the
/// active UI theme by *name* (the configured theme slot for the current
/// mode), falling back to the mode default when no same-named map theme
/// exists; unknown explicit names fall back to the mode default (with a
/// warning).
fn resolve_map_theme_id(config: &Config) -> &'static str {
    let ui_theme_name = if config.mode.is_dark() {
        &config.ui_theme_dark
    } else {
        &config.ui_theme_light
    };
    let name = config.map_theme.resolved(config.mode, ui_theme_name);
    match strata_render::MapTheme::by_id(name) {
        Some(theme) => theme.id,
        None => {
            tracing::warn!(name, "unknown map theme in config; using the mode default");
            if config.mode.is_dark() {
                strata_render::MapTheme::oldworld().id
            } else {
                strata_render::MapTheme::pastel_light().id
            }
        }
    }
}

fn load_dataset_meta(store: &Store) -> HashMap<Dataset, DatasetMeta> {
    let mut meta = HashMap::new();
    for dataset in [
        Dataset::Airspaces,
        Dataset::Airports,
        Dataset::Navaids,
        Dataset::ReportingPoints,
        Dataset::Obstacles,
        Dataset::TerrainTiles,
    ] {
        // Cross-country summary: the oldest AIRAC cycle wins, which keeps
        // the staleness chip honest for multi-country stores.
        match store.dataset_meta_summary(dataset) {
            Ok(Some(m)) => {
                meta.insert(dataset, m);
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%dataset, %err, "reading dataset meta failed"),
        }
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(dataset: Dataset, source: &str) -> DatasetMeta {
        DatasetMeta {
            dataset,
            country: Country::DE,
            source: source.to_owned(),
            airac: None,
            ingested_at: Default::default(),
        }
    }

    fn metas(entries: &[(Dataset, &str)]) -> HashMap<Dataset, DatasetMeta> {
        entries
            .iter()
            .map(|(dataset, source)| (*dataset, meta(*dataset, source)))
            .collect()
    }

    #[test]
    fn airports_reingest_reloads_stations() {
        let old = metas(&[(Dataset::Airports, "openaip")]);
        let new = metas(&[(Dataset::Airports, "openaip-2")]);
        assert!(stations_need_reload(&old, &new, true));
    }

    #[test]
    fn unrelated_dataset_change_keeps_loaded_stations() {
        let old = metas(&[(Dataset::Airports, "openaip")]);
        let mut new = old.clone();
        new.insert(Dataset::Navaids, meta(Dataset::Navaids, "openaip"));
        assert!(!stations_need_reload(&old, &new, true));
    }

    #[test]
    fn first_airports_ingest_loads_missing_stations() {
        let old = HashMap::new();
        let new = metas(&[(Dataset::Airports, "openaip")]);
        assert!(stations_need_reload(&old, &new, false));
        // Identical airports meta but empty positions (e.g. the table has no
        // idents) still retries the load.
        assert!(stations_need_reload(&new, &new, false));
        assert!(!stations_need_reload(&new, &new, true));
    }

    /// The live-weather scope follows the enabled countries: one
    /// thinning-safe box for the default (Germany) set, several for
    /// far-apart sets — and the SIGMET union always contains every
    /// enabled country's box. (The clustering itself is
    /// `strata_data::domain::weather_bboxes`, tested there; this pins the
    /// app's wiring assumptions.)
    #[test]
    fn weather_scope_follows_enabled_countries() {
        let contains = |outer: &BoundingBox, inner: &BoundingBox| {
            outer.west() <= inner.west()
                && outer.south() <= inner.south()
                && outer.east() >= inner.east()
                && outer.north() >= inner.north()
        };

        // Default set: exactly one box, covering Germany.
        let boxes = weather_bboxes(&[Country::DE]);
        assert_eq!(boxes.len(), 1);
        assert!(contains(&boxes[0], &Country::DE.bounding_box()));

        // Far-apart set (Germany + Cyprus): chunked into several boxes so
        // aviationweather.gov doesn't thin the stations; every country is
        // covered by some box.
        let enabled = [Country::DE, Country::CY];
        let boxes = weather_bboxes(&enabled);
        assert!(boxes.len() > 1, "far-apart countries must not share a box");
        for country in enabled {
            let bbox = country.bounding_box();
            assert!(
                boxes.iter().any(|b| contains(b, &bbox)),
                "{country} not covered"
            );
        }

        // The SIGMET union box contains everything (size is free there —
        // the upstream feed is global and client-filtered).
        let union = Country::union_bbox(&enabled).expect("non-empty set");
        for country in enabled {
            assert!(contains(&union, &country.bounding_box()));
        }

        // The empty set (every country disabled) fetches nothing: no
        // METAR/TAF boxes, no SIGMET union — honest absence, no errors.
        assert!(weather_bboxes(&[]).is_empty());
        assert_eq!(Country::union_bbox(&[]), None);
    }

    /// Stations from overlapping weather boxes merge by id — first
    /// occurrence wins, later duplicates are dropped.
    #[test]
    fn merge_by_key_drops_duplicate_stations() {
        let mut merged: Vec<(String, u32)> = Vec::new();
        // A fn item (not a closure binding) so the higher-ranked
        // `for<'a> Fn(&'a T) -> &'a K` bound is met at both call sites.
        fn key(t: &(String, u32)) -> &String {
            &t.0
        }
        merge_by_key(&mut merged, vec![("EDDF".into(), 1), ("EDDM".into(), 1)], key);
        merge_by_key(
            &mut merged,
            vec![
                ("EDDM".into(), 2), // duplicate from an overlapping box
                ("LOWW".into(), 2),
                ("LOWW".into(), 3), // duplicate within one response
            ],
            key,
        );
        assert_eq!(
            merged,
            vec![
                ("EDDF".to_owned(), 1),
                ("EDDM".to_owned(), 1),
                ("LOWW".to_owned(), 2),
            ]
        );
    }

    /// The world box must accept every feature the store can hold.
    #[test]
    fn world_bbox_covers_everything() {
        let world = world_bbox();
        assert_eq!(
            (world.west(), world.south(), world.east(), world.north()),
            (-180.0, -90.0, 180.0, 90.0)
        );
    }

    #[test]
    fn profile_scrub_updates_gate_on_planning_mode_and_change() {
        let mut slot = None;

        // Explorer mode: a scrub makes no sense and never lands.
        assert!(!update_profile_scrub(&mut slot, Some(Meters(1000.0)), false));
        assert_eq!(slot, None);

        // Planning mode: set / no-op / move / clear.
        assert!(update_profile_scrub(&mut slot, Some(Meters(1000.0)), true));
        assert_eq!(slot, Some(Meters(1000.0)));
        assert!(
            !update_profile_scrub(&mut slot, Some(Meters(1000.0)), true),
            "identical value is a no-op (mouse-move chatter)"
        );
        assert!(update_profile_scrub(&mut slot, Some(Meters(2000.0)), true));
        assert!(update_profile_scrub(&mut slot, None, true), "clearing is a change");
        assert!(!update_profile_scrub(&mut slot, None, true));

        // Leaving planning mode collapses a live scrub to None once.
        assert!(update_profile_scrub(&mut slot, Some(Meters(3000.0)), true));
        assert!(update_profile_scrub(&mut slot, Some(Meters(3000.0)), false));
        assert_eq!(slot, None);
    }

    #[test]
    fn route_highlight_updates_gate_on_planning_mode_and_change() {
        let mut slot = None;

        // Explorer mode: there is no route to highlight.
        assert!(!update_route_highlight(&mut slot, Some(1), false));
        assert_eq!(slot, None);

        // Planning mode: set / no-op / move / clear.
        assert!(update_route_highlight(&mut slot, Some(1), true));
        assert_eq!(slot, Some(1));
        assert!(
            !update_route_highlight(&mut slot, Some(1), true),
            "identical id is a no-op (hover chatter)"
        );
        assert!(update_route_highlight(&mut slot, Some(2), true));
        assert!(update_route_highlight(&mut slot, None, true), "clearing is a change");
        assert!(!update_route_highlight(&mut slot, None, true));

        // Leaving planning mode collapses a live highlight to None once.
        assert!(update_route_highlight(&mut slot, Some(3), true));
        assert!(update_route_highlight(&mut slot, Some(3), false));
        assert_eq!(slot, None);
    }

    #[test]
    fn data_dir_resolution_prefers_env_then_config_then_default() {
        let env = Some(std::ffi::OsString::from("/env/dir"));
        let config = Some(Path::new("/config/dir"));
        assert_eq!(
            resolve_data_dir_from(env.clone(), config),
            PathBuf::from("/env/dir"),
            "STRATA_DATA_DIR is the explicit per-invocation override"
        );
        assert_eq!(
            resolve_data_dir_from(None, config),
            PathBuf::from("/config/dir")
        );
        let default = resolve_data_dir_from(None, None);
        assert!(default.ends_with("strata"), "got: {}", default.display());
    }

    #[test]
    fn map_theme_resolution_follows_mode_and_survives_unknown_names() {
        use crate::config::{MapTheme, ThemeMode};

        let mut config = Config::default(); // Auto + Dark + "Oldworld"/"Pastel Light"
        assert_eq!(resolve_map_theme_id(&config), "oldworld");
        config.mode = ThemeMode::Light;
        assert_eq!(resolve_map_theme_id(&config), "pastel-light");

        config.map_theme = MapTheme::Named("high-contrast".into());
        assert_eq!(resolve_map_theme_id(&config), "high-contrast");

        // Unknown name → mode default, not a panic.
        config.map_theme = MapTheme::Named("no-such-theme".into());
        assert_eq!(resolve_map_theme_id(&config), "pastel-light");
        config.mode = ThemeMode::Dark;
        assert_eq!(resolve_map_theme_id(&config), "oldworld");
    }

    /// `Auto` follows the *active UI theme* by name: the configured slot
    /// for the current mode picks the same-named map theme; UI themes
    /// without a map sibling fall back to the mode default; an explicit
    /// `Named` selection ignores the UI theme entirely.
    #[test]
    fn auto_map_theme_follows_the_active_ui_theme_name() {
        use crate::config::{MapTheme, ThemeMode};

        let mut config = Config {
            ui_theme_dark: "Catppuccin Mocha".into(),
            ui_theme_light: "Gruvbox Light".into(),
            ..Default::default() // Auto + Dark
        };
        assert_eq!(resolve_map_theme_id(&config), "catppuccin-mocha");
        config.mode = ThemeMode::Light;
        assert_eq!(resolve_map_theme_id(&config), "gruvbox-light");

        // The inactive slot never leaks into resolution.
        config.ui_theme_dark = "Tokyo Night".into();
        assert_eq!(resolve_map_theme_id(&config), "gruvbox-light");
        config.mode = ThemeMode::Dark;
        assert_eq!(resolve_map_theme_id(&config), "tokyo-night");

        // No same-named map theme → mode default.
        config.ui_theme_dark = "Some Custom Theme".into();
        assert_eq!(resolve_map_theme_id(&config), "oldworld");
        config.mode = ThemeMode::Light;
        config.ui_theme_light = "Another Custom".into();
        assert_eq!(resolve_map_theme_id(&config), "pastel-light");

        // Explicit Named override wins over the UI theme match.
        config.ui_theme_light = "Solarized Light".into();
        config.map_theme = MapTheme::Named("matrix".into());
        assert_eq!(resolve_map_theme_id(&config), "matrix");
    }
}

//! Debounced, generation-tagged compute orchestration (plan §5.1).
//!
//! Every document edit claims a fresh generation and (re)schedules one
//! task: ~50 ms debounce on the foreground, then [`run_compute`] on the
//! background executor. Replacing the task cancels the previous debounce/
//! run; the generation check drops any result that still lands late
//! (belt and suspenders — and the same counter guards async dirty-clearing
//! in the save path).
//!
//! Sources are constructed **inside** [`run_compute`] — on the compute
//! thread — from `Send` ingredients (`Arc<Store>`, `Arc<WindsAloftFrames>`),
//! per the strata-plan contract that source traits are not `Send + Sync`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context};
use strata_data::domain::BoundingBox;
use strata_data::store::{ELEVATION_CELLS_PER_DEGREE, ELEVATION_TILE_SIDE, ElevationTileSet, Store};
use strata_plan::compute::{
    ComputeOutcome as PlanOutcome, ComputeParams, ComputedFlight, NotComputable,
};
use strata_plan::sources::Sources;
use strata_plan::{AircraftProfile, FlightDoc};

use crate::sources::{
    GriddedWindsAloftSampler, StoreAirspaceSource, StoreElevationSource, StoreObstacleSource,
    WindsAloftFrames, WmmMagvarSource, points_prefetch_bbox,
};
use crate::state::{AppState, AppStateEvent};

use super::ComputeState;

/// Debounce between an edit and its compute run — long enough to coalesce
/// a typing/drag burst, short enough to feel live (plan §5.1: ~50 ms).
pub const COMPUTE_DEBOUNCE: Duration = Duration::from_millis(50);

/// A decoded elevation tile set together with the coverage bbox it was
/// bulk-read for — cached on the open flight across compute runs, so a
/// typing burst does not re-read and re-inflate ~17 MB of tiles from
/// SQLite on every keystroke. Invalidated by coverage containment (route
/// edits that outgrow it) and by post-ingest data reloads.
pub type ElevationCache = (BoundingBox, Arc<ElevationTileSet>);

/// One full elevation tile (256 cells of 1/600°) beyond the required bbox
/// on each side of a fresh prefetch: small route edits (drags, appends)
/// stay inside the cached coverage instead of re-prefetching per edit.
const ELEVATION_CACHE_PAD_DEG: f64 =
    ELEVATION_TILE_SIDE as f64 / ELEVATION_CELLS_PER_DEGREE as f64;

/// Generation bookkeeping for debounced background compute (pure).
///
/// Edits call [`schedule`](Self::schedule) to claim a generation; a
/// finished run calls [`try_apply`](Self::try_apply) with the generation it
/// was started for and only the run matching the *latest* schedule lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComputeGeneration {
    scheduled: u64,
    applied: Option<u64>,
}

impl ComputeGeneration {
    /// Claims the next generation for a newly scheduled run.
    pub fn schedule(&mut self) -> u64 {
        self.scheduled += 1;
        self.scheduled
    }

    /// The latest scheduled generation. Production code no longer reads it
    /// (async dirty-clearing rides on [`OpenFlight::edit_epoch`], which
    /// notes-only edits bump without claiming a generation); the gate
    /// tests pin its semantics.
    ///
    /// [`OpenFlight::edit_epoch`]: super::OpenFlight::edit_epoch
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current(&self) -> u64 {
        self.scheduled
    }

    /// Records a finished run; `false` means the result is stale (a newer
    /// edit superseded it, or this generation already applied) and must be
    /// dropped. Generation 0 is never handed out by [`Self::schedule`] and
    /// never applies.
    pub fn try_apply(&mut self, generation: u64) -> bool {
        if generation == 0 || generation != self.scheduled || self.applied == Some(generation) {
            return false;
        }
        self.applied = Some(generation);
        true
    }

    /// Whether the latest scheduled run has landed — i.e. the stored
    /// computed state reflects the newest compute-relevant edit. `false`
    /// while an edit's debounce/run is still in flight (the Loading tab
    /// renders its synchronous W&B preview during exactly that window)
    /// and before anything was ever scheduled.
    pub fn is_current(&self) -> bool {
        self.applied == Some(self.scheduled)
    }
}

/// What one background run produced: [`PlanOutcome`] plus the app-level
/// failure cases (store unavailable, source errors).
#[derive(Debug)]
pub enum ComputeOutcome {
    Computed(Arc<ComputedFlight>),
    /// Benign: the document cannot be computed in its current editing
    /// state (typed reason). Normal while a route is being built — logged
    /// once per generation at `debug!`, never warned.
    NotComputable(NotComputable),
    /// The pipeline failed (store unavailable, source/profile errors).
    Failed(String),
}

impl AppState {
    /// The compute parameters the current config implies: the corridor
    /// half-width from the drawer's width select (design §3.3 "configurable
    /// ±2–5 NM"), everything else at the planning core's defaults.
    fn flight_compute_params(&self) -> ComputeParams {
        let mut params = ComputeParams::default();
        params.corridor.half_width =
            strata_plan::units::NauticalMiles(self.config.profile_drawer.corridor_half_width_nm)
                .as_meters();
        params
    }

    /// (Re)schedules the debounced compute for the open flight. Called by
    /// every document mutation and by library reloads; replacing the
    /// previous task cancels its debounce or in-flight run.
    pub(crate) fn schedule_flight_compute(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = &mut self.flight else {
            return;
        };
        let generation = flight.compute_generation.schedule();
        let doc = flight.doc.clone();
        // The decoded-elevation cache rides along (cheap: bbox + Arc);
        // run_compute reuses it while its coverage contains the route.
        let elevation_cache = flight.elevation_cache.clone();
        let aircraft = doc
            .aircraft_id
            .as_ref()
            .and_then(|id| self.aircraft_library.iter().find(|p| &p.id == id))
            .cloned();
        // Compute gets its own store connection (WAL supports several per
        // path), so its bulk reads never queue behind the UI's hit-test/
        // search queries on the shared connection — nor vice versa.
        let store = self.compute_store.clone().or_else(|| self.store.clone());
        let winds = self.flight_winds.frames_snapshot();
        let params = self.flight_compute_params();

        self.flight_compute_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COMPUTE_DEBOUNCE).await;
            let started = Instant::now();
            let (outcome, elevation) = cx
                .background_spawn(async move {
                    run_compute(&doc, aircraft.as_ref(), store, winds, &params, elevation_cache)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(flight) = &mut this.flight else {
                    return;
                };
                if !flight.compute_generation.try_apply(generation) {
                    tracing::debug!(generation, "dropping stale compute result");
                    return;
                }
                // Store the (possibly fresh) tile set back for the next
                // run — only under the generation gate, so a stale run's
                // coverage can never replace a newer one.
                if let Some(elevation) = elevation {
                    flight.elevation_cache = Some(elevation);
                }
                match outcome {
                    ComputeOutcome::Computed(computed) => {
                        tracing::debug!(
                            generation,
                            compute_ms = started.elapsed().as_millis() as u64,
                            conflicts = computed.conflicts.len(),
                            "flight computed"
                        );
                        flight.computed = Some(computed);
                        flight.compute_state = ComputeState::Computed;
                    }
                    ComputeOutcome::NotComputable(reason) => {
                        tracing::debug!(generation, %reason, "flight not computable");
                        flight.computed = None;
                        flight.compute_state = ComputeState::NotComputable(reason);
                    }
                    ComputeOutcome::Failed(error) => {
                        tracing::warn!(generation, error, "flight compute failed");
                        flight.computed = None;
                        flight.compute_state = ComputeState::Failed(error);
                    }
                }
                // The corridor (or its absence) changed — the NOTAM
                // briefing list rides on it (see `state::briefing`).
                this.refresh_briefing_relevance(cx);
                cx.emit(AppStateEvent::FlightComputed);
                cx.notify();
            })
            .ok();
        }));
    }
}

/// The elevation prefetch envelope for `doc` under `params`: route +
/// alternates padded by the corridor envelope (half-width + station
/// spacing, ×1.5 slack), so corridor sampling and endpoint elevations stay
/// within the bulk-read tiles. `None` without any points.
fn elevation_prefetch_bbox(doc: &FlightDoc, params: &ComputeParams) -> Option<BoundingBox> {
    let margin = (params.corridor.half_width.0 + params.corridor.station_spacing.0) * 1.5;
    let points = doc
        .route
        .iter()
        .map(|w| w.position())
        .chain(doc.alternates.iter().map(|p| p.position()));
    points_prefetch_bbox(points, margin)
}

/// `bbox` grown by `pad_deg` on every side, clamped to valid coordinates.
fn inflate_bbox(bbox: BoundingBox, pad_deg: f64) -> BoundingBox {
    BoundingBox::new(
        (bbox.west() - pad_deg).max(-180.0),
        (bbox.south() - pad_deg).max(-90.0),
        (bbox.east() + pad_deg).min(180.0),
        (bbox.north() + pad_deg).min(90.0),
    )
    // The clamped corners are always a valid box; keep the input on the
    // impossible error path rather than panicking.
    .unwrap_or(bbox)
}

/// One complete compute run, executed on the background executor: builds
/// the sources over the store / winds snapshot **on this thread** and runs
/// `strata_plan::compute`.
///
/// `elevation_cache` is the previous run's decoded tile set: it is reused
/// while its coverage contains the current prefetch envelope (see
/// [`elevation_prefetch_bbox`]); otherwise a fresh bulk read over the
/// envelope inflated by [`ELEVATION_CACHE_PAD_DEG`] replaces it. The
/// returned cache entry (`None` only when the run never reached the
/// elevation stage) is handed back to the caller for the next run; points
/// outside coverage still fall back per-call (see
/// [`StoreElevationSource`]).
pub fn run_compute(
    doc: &FlightDoc,
    aircraft: Option<&AircraftProfile>,
    store: Option<Arc<Store>>,
    winds: Arc<WindsAloftFrames>,
    params: &ComputeParams,
    elevation_cache: Option<ElevationCache>,
) -> (ComputeOutcome, Option<ElevationCache>) {
    let Some(aircraft) = aircraft else {
        let reason = match &doc.aircraft_id {
            Some(id) => NotComputable::UnknownAircraft { id: id.to_string() },
            None => NotComputable::NoAircraft,
        };
        return (ComputeOutcome::NotComputable(reason), None);
    };
    let Some(store) = store else {
        return (
            ComputeOutcome::Failed("data store unavailable".to_owned()),
            None,
        );
    };

    let Some(required) = elevation_prefetch_bbox(doc, params) else {
        // No points at all → nothing to prefetch over; the same gap the
        // pipeline itself would classify.
        return (ComputeOutcome::NotComputable(NotComputable::NoRoute), None);
    };

    let elevation = match elevation_cache.filter(|(coverage, _)| coverage.contains_bbox(&required))
    {
        // Cache hit: reuse the decoded tiles — no store read, no zlib.
        Some((coverage, tiles)) => {
            StoreElevationSource::with_tiles(Arc::clone(&store), coverage, tiles)
        }
        None => match StoreElevationSource::prefetch(
            Arc::clone(&store),
            inflate_bbox(required, ELEVATION_CACHE_PAD_DEG),
        ) {
            Ok(source) => source,
            Err(err) => {
                return (
                    ComputeOutcome::Failed(strata_ingest::error_chain(&err)),
                    None,
                );
            }
        },
    };
    let cache = Some(elevation.parts());
    let obstacles = StoreObstacleSource::new(Arc::clone(&store));
    let airspaces = StoreAirspaceSource::new(store);
    let sampler = GriddedWindsAloftSampler::new(winds);
    let magvar = WmmMagvarSource;
    let sources = Sources {
        elevation: &elevation,
        obstacles: &obstacles,
        airspaces: &airspaces,
        winds: &sampler,
        magvar: &magvar,
    };

    let outcome = match strata_plan::compute(doc, aircraft, &sources, params) {
        Ok(PlanOutcome::Computed(computed)) => ComputeOutcome::Computed(Arc::new(*computed)),
        Ok(PlanOutcome::NotComputable(reason)) => ComputeOutcome::NotComputable(reason),
        Err(err) => ComputeOutcome::Failed(strata_ingest::error_chain(&err)),
    };
    (outcome, cache)
}

/// Shared fixtures for tests that drive [`run_compute`] over a temp store
/// (this module's own tests and the Loading tab's W&B preview agreement
/// test).
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use strata_data::domain::{LatLon, MetersAmsl};
    use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId, Store};
    use strata_plan::FlightDoc;
    use strata_plan::flight::{FreePoint, PlannedAltitude, RoutePoint, RouteWaypoint};

    use crate::flight_io::aircraft::example_c172;

    pub(crate) fn temp_store_with_elevation() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("store.sqlite")).unwrap();
        // Flat 250 m terrain around the test route (50°N 8°E – 50°N 9°E).
        for lon in [8.0, 8.5, 9.0] {
            let id = ElevationTileId::containing(50.0, lon);
            let tile = ElevationTile::new(id, vec![250; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE])
                .unwrap();
            store.put_elevation_tile(&tile).unwrap();
        }
        (dir, Arc::new(store))
    }

    pub(crate) fn two_leg_doc() -> FlightDoc {
        let mut doc = FlightDoc::new("test");
        let wp = |name: &str, lat: f64, lon: f64| {
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: Some(name.to_owned()),
                position: LatLon::new(lat, lon).unwrap(),
            }))
        };
        doc.route = vec![wp("A", 50.0, 8.0), wp("B", 50.0, 9.0)];
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)));
        doc.aircraft_id = Some(example_c172().id);
        doc.loading.fuel = strata_plan::units::Liters(150.0);
        doc
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::{LatLon, MetersAmsl};
    use strata_plan::flight::{FreePoint, RoutePoint, RouteWaypoint};

    use crate::flight_io::aircraft::example_c172;

    use super::test_support::{temp_store_with_elevation, two_leg_doc};
    use super::*;

    // --- generation gate (pure) -------------------------------------------

    #[test]
    fn generations_are_monotonic_and_stale_results_drop() {
        let mut generations = ComputeGeneration::default();
        let first = generations.schedule();
        let second = generations.schedule();
        assert!(second > first);

        // The superseded run's result is stale.
        assert!(!generations.try_apply(first));
        // The latest run lands exactly once.
        assert!(generations.try_apply(second));
        assert!(!generations.try_apply(second), "double apply rejected");

        // A new edit after an applied result starts a fresh cycle.
        let third = generations.schedule();
        assert_eq!(generations.current(), third);
        assert!(generations.try_apply(third));
    }

    #[test]
    fn unknown_generations_never_apply() {
        let mut generations = ComputeGeneration::default();
        assert!(!generations.try_apply(0));
        assert!(!generations.try_apply(7));
        let g = generations.schedule();
        assert!(!generations.try_apply(g + 1));
        assert!(generations.try_apply(g));
    }

    #[test]
    fn is_current_flips_with_the_schedule_apply_cycle() {
        let mut generations = ComputeGeneration::default();
        assert!(!generations.is_current(), "nothing scheduled yet");

        let first = generations.schedule();
        assert!(!generations.is_current(), "debounce/run still in flight");
        assert!(generations.try_apply(first));
        assert!(generations.is_current(), "the latest run landed");

        // The next edit immediately marks the stored result stale.
        generations.schedule();
        assert!(!generations.is_current());
    }

    // --- run_compute over a temp store --------------------------------------

    #[test]
    fn computes_a_simple_flight_over_a_temp_store() {
        let (_dir, store) = temp_store_with_elevation();
        let aircraft = example_c172();
        let (outcome, elevation) = run_compute(
            &two_leg_doc(),
            Some(&aircraft),
            Some(store),
            Arc::new(WindsAloftFrames::default()),
            &ComputeParams::default(),
            None,
        );
        assert!(
            elevation.is_some(),
            "a successful run hands back its decoded tile set"
        );
        let ComputeOutcome::Computed(computed) = outcome else {
            panic!("expected Computed, got {outcome:?}");
        };
        assert_eq!(computed.legs.len(), 1);
        // ~39 NM leg at 50°N.
        let nm = computed.legs[0].distance.0 / 1852.0;
        assert!((nm - 38.6).abs() < 1.0, "distance {nm} NM");
        // The corridor saw the synthetic terrain.
        assert!(
            computed
                .corridor
                .samples
                .iter()
                .any(|s| s.max_terrain == Some(MetersAmsl(250.0)))
        );
        // No winds frames + no departure time → calm fallback solved legs.
        assert_eq!(computed.winds.len(), 1);
        assert_eq!(computed.winds[0].wind.speed.0, 0.0);
    }

    #[test]
    fn structural_gaps_are_benign_and_typed() {
        let (_dir, store) = temp_store_with_elevation();
        let winds = Arc::new(WindsAloftFrames::default());
        let params = ComputeParams::default();

        // No aircraft selected.
        let mut doc = two_leg_doc();
        doc.aircraft_id = None;
        let (outcome, _) =
            run_compute(&doc, None, Some(Arc::clone(&store)), winds.clone(), &params, None);
        assert!(
            matches!(&outcome, ComputeOutcome::NotComputable(NotComputable::NoAircraft)),
            "{outcome:?}"
        );

        // Aircraft referenced but not in the library.
        let doc = two_leg_doc();
        let (outcome, _) =
            run_compute(&doc, None, Some(Arc::clone(&store)), winds.clone(), &params, None);
        assert!(
            matches!(
                &outcome,
                ComputeOutcome::NotComputable(NotComputable::UnknownAircraft { id })
                    if id == "example-c172"
            ),
            "{outcome:?}"
        );

        let aircraft = example_c172();

        // One-waypoint route.
        let mut doc = two_leg_doc();
        doc.route.truncate(1);
        let (outcome, _) = run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::clone(&store)),
            winds.clone(),
            &params,
            None,
        );
        assert!(
            matches!(
                &outcome,
                ComputeOutcome::NotComputable(NotComputable::RouteTooShort)
            ),
            "{outcome:?}"
        );

        // No waypoints at all.
        let mut doc = two_leg_doc();
        doc.route.clear();
        let (outcome, _) = run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::clone(&store)),
            winds.clone(),
            &params,
            None,
        );
        assert!(
            matches!(&outcome, ComputeOutcome::NotComputable(NotComputable::NoRoute)),
            "{outcome:?}"
        );

        // No planned altitude anywhere (a pre-defaults document like the
        // gate-4 test flight): benign, with the offending leg — the old
        // behaviour was a warned hard failure.
        let mut doc = two_leg_doc();
        doc.cruise_altitude = None;
        let (outcome, _) = run_compute(&doc, Some(&aircraft), Some(store), winds, &params, None);
        assert!(
            matches!(
                &outcome,
                ComputeOutcome::NotComputable(NotComputable::MissingAltitude { leg: 0 })
            ),
            "{outcome:?}"
        );
    }

    /// The per-keystroke hot path: a second run over the same route must
    /// reuse the first run's decoded tile set (same `Arc`, no SQLite read
    /// or zlib inflate), and a route that outgrows the coverage prefetches
    /// a fresh, larger set.
    #[test]
    fn elevation_cache_is_reused_until_the_route_outgrows_it() {
        let (_dir, store) = temp_store_with_elevation();
        let aircraft = example_c172();
        let winds = Arc::new(WindsAloftFrames::default());
        let params = ComputeParams::default();
        let doc = two_leg_doc();

        let (outcome, first) = run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::clone(&store)),
            winds.clone(),
            &params,
            None,
        );
        assert!(matches!(outcome, ComputeOutcome::Computed(_)));
        let (coverage, tiles) = first.expect("first run produces the cache");
        assert!(
            coverage.contains_bbox(&elevation_prefetch_bbox(&doc, &params).unwrap()),
            "coverage spans the route envelope (plus the pad)"
        );

        // Same document again (the typing-burst shape): the exact tile
        // set comes back.
        let (outcome, second) = run_compute(
            &doc,
            Some(&aircraft),
            Some(Arc::clone(&store)),
            winds.clone(),
            &params,
            Some((coverage, Arc::clone(&tiles))),
        );
        assert!(matches!(outcome, ComputeOutcome::Computed(_)));
        let (second_coverage, second_tiles) = second.expect("cache flows through");
        assert!(Arc::ptr_eq(&tiles, &second_tiles), "decoded tiles reused");
        assert_eq!(second_coverage, coverage);

        // Extending the route far beyond the coverage forces a fresh
        // prefetch over a larger envelope (stale coverage must not serve).
        let mut extended = two_leg_doc();
        extended.route.push(RouteWaypoint::new(RoutePoint::Free(FreePoint {
            name: Some("C".to_owned()),
            position: LatLon::new(52.5, 12.0).unwrap(),
        })));
        let (_, third) = run_compute(
            &extended,
            Some(&aircraft),
            Some(store),
            winds,
            &params,
            Some((coverage, Arc::clone(&tiles))),
        );
        let (third_coverage, third_tiles) = third.expect("fresh cache after the miss");
        assert!(!Arc::ptr_eq(&tiles, &third_tiles), "stale set replaced");
        assert!(
            third_coverage
                .contains_bbox(&elevation_prefetch_bbox(&extended, &params).unwrap()),
            "new coverage spans the extended route"
        );
    }

    /// The phase-4 gate's exact complaint: a brand-new flight plus two
    /// route clicks must compute — the seeded cruise default closes the
    /// "leg 0 has no planned altitude" gap.
    #[test]
    fn a_new_flight_doc_with_two_waypoints_computes_immediately() {
        let (_dir, store) = temp_store_with_elevation();
        let aircraft = example_c172();

        let mut doc = crate::state::flight::new_flight_doc("");
        doc.aircraft_id = Some(aircraft.id.clone());
        doc.loading.fuel = strata_plan::units::Liters(150.0);
        let wp = |name: &str, lat: f64, lon: f64| {
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: Some(name.to_owned()),
                position: LatLon::new(lat, lon).unwrap(),
            }))
        };
        super::super::ops::append_waypoint(&mut doc, wp("A", 50.0, 8.0).point);
        super::super::ops::append_waypoint(&mut doc, wp("B", 50.0, 9.0).point);

        let (outcome, _) = run_compute(
            &doc,
            Some(&aircraft),
            Some(store),
            Arc::new(WindsAloftFrames::default()),
            &ComputeParams::default(),
            None,
        );
        let ComputeOutcome::Computed(computed) = outcome else {
            panic!("expected Computed, got {outcome:?}");
        };
        // The whole profile flies at the seeded 3000 ft default.
        let cruise = computed
            .phases
            .segments
            .iter()
            .find(|s| s.kind == strata_plan::perf::PhaseKind::Cruise)
            .expect("reaches cruise");
        assert!((cruise.start_altitude.as_feet() - 3000.0).abs() < 1e-6);
    }

    #[test]
    fn missing_store_fails_loudly() {
        let aircraft = example_c172();
        let (outcome, elevation) = run_compute(
            &two_leg_doc(),
            Some(&aircraft),
            None,
            Arc::new(WindsAloftFrames::default()),
            &ComputeParams::default(),
            None,
        );
        assert!(
            matches!(&outcome, ComputeOutcome::Failed(e) if e.contains("store")),
            "{outcome:?}"
        );
        assert!(elevation.is_none(), "no cache from a run that never read tiles");
    }
}

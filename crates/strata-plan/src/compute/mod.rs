//! The compute façade (plan §3 `compute/`): one pure entry point the app
//! calls on every edit — `compute(doc, aircraft, sources, params)` →
//! [`ComputedFlight`]. Target: <10 ms for typical routes so it can run
//! per keystroke on a background thread, generation-tagged app-side.
//!
//! Orchestration only — every number is produced by the owning module
//! (corridor, wind, perf, wb, fuel, conflict, navlog); this module wires
//! their inputs together consistently (one cruise-setting resolution, one
//! taxi/trip fuel figure shared between the ladder and W&B) and assembles
//! the per-leg summaries.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use strata_data::domain::{LatLon, Meters, MetersAmsl};
use thiserror::Error;

use crate::aircraft::AircraftProfile;
use crate::conflict::{self, Conflict, ConflictError, ConflictThresholds};
use crate::corridor::{self, Corridor, CorridorError, CorridorParams, CorridorSample};
use crate::flight::{FlightDoc, RoutePoint, RouteWaypoint};
use crate::fuel::{self, FuelError, FuelLadder};
use crate::navlog::{self, NavLog, NavLogError};
use crate::perf::{self, PerfError, PhasePlan};
use crate::route::{self, RouteError};
use crate::sources::{ElevationSource, SourceError, Sources};
use crate::units::{DegreesMagnetic, DegreesTrue};
use crate::wb::{self, WbError, WbReport};
use crate::wind::{self, LegWind, WindError};

#[cfg(test)]
mod tests;

/// Tuning knobs for one compute run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComputeParams {
    pub corridor: CorridorParams,
    pub thresholds: ConflictThresholds,
}

/// Resolved geometry of one leg (the map/list-facing summary; the nav log
/// carries the full per-leg numbers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputedLeg {
    pub index: usize,
    /// Labels of the bounding waypoints (`RoutePoint::label`).
    pub from: String,
    pub to: String,
    pub distance: Meters,
    pub true_track: DegreesTrue,
    /// True track corrected by the variation at the leg midpoint.
    pub magnetic_track: DegreesMagnetic,
    pub midpoint: LatLon,
}

/// Everything the planning surfaces render — the complete computed state
/// for one document generation. Serializable so `strata-brief` can take it
/// as the PDF context (plan §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputedFlight {
    pub legs: Vec<ComputedLeg>,
    /// Profile series: per-station terrain/obstacles + airspace crossings.
    pub corridor: Corridor,
    pub winds: Vec<LegWind>,
    pub phases: PhasePlan,
    pub weight_balance: WbReport,
    pub fuel: FuelLadder,
    pub conflicts: Vec<Conflict>,
    pub navlog: NavLog,
}

/// What one [`compute`] run produced: the computed state, or the typed
/// reason the document honestly cannot be computed yet. `Err` on the
/// [`compute`] result is reserved for real failures ([`ComputeError`]).
///
/// The flight is boxed: the outcome value travels through channels and
/// generation gates while [`ComputedFlight`] is hundreds of bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComputeOutcome {
    Computed(Box<ComputedFlight>),
    NotComputable(NotComputable),
}

impl ComputeOutcome {
    /// The computed flight; `None` when the document was not computable.
    pub fn computed(self) -> Option<ComputedFlight> {
        match self {
            Self::Computed(flight) => Some(*flight),
            Self::NotComputable(_) => None,
        }
    }
}

/// A structural gap that leaves the document *not computable* in its
/// current editing state — the normal mid-edit condition, deliberately
/// distinct from [`ComputeError`] (real failures). Typed so callers can
/// store and render it without string matching; the `Display` text is the
/// user-facing phrasing.
///
/// The aircraft variants are produced by the *caller*: the compute façade
/// takes an already-resolved profile, so the app maps an unset or
/// unresolvable [`FlightDoc::aircraft_id`] onto them — one shared
/// vocabulary for every planning surface.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotComputable {
    /// The route has no waypoints yet.
    #[error("the route is empty")]
    NoRoute,
    /// Fewer than two distinct waypoints (a single point, or only
    /// coincident ones) — there is no leg to plan.
    #[error("the route needs at least two waypoints")]
    RouteTooShort,
    /// `leg` has neither its own planned altitude nor a flight cruise
    /// altitude to fall back to — the vertical profile is undefined.
    #[error(
        "leg {leg} has no planned altitude (no leg altitude and no flight cruise altitude)"
    )]
    MissingAltitude { leg: usize },
    /// No aircraft profile is selected on the document (caller-produced).
    #[error("no aircraft selected")]
    NoAircraft,
    /// The document references an aircraft profile the caller could not
    /// resolve (caller-produced).
    #[error("aircraft profile \"{id}\" is not in the library")]
    UnknownAircraft { id: String },
}

/// Errors from the compute pipeline — real failures (source IO,
/// inconsistent profiles), never the benign editing states
/// ([`NotComputable`]).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ComputeError {
    #[error(transparent)]
    Route(#[from] RouteError),
    #[error(transparent)]
    Corridor(#[from] CorridorError),
    #[error(transparent)]
    Wind(#[from] WindError),
    #[error(transparent)]
    Perf(#[from] PerfError),
    #[error(transparent)]
    WeightBalance(#[from] WbError),
    #[error(transparent)]
    Fuel(#[from] FuelError),
    #[error(transparent)]
    Conflict(#[from] ConflictError),
    #[error(transparent)]
    NavLog(#[from] NavLogError),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Routes whose total great-circle length is at or below this (meters)
/// consist only of coincident points — there is no corridor or vertical
/// profile to compute. Matches the perf module's distance epsilon.
const MIN_ROUTE_LENGTH_METERS: f64 = 1e-6;

/// Computes everything for `doc` in one deterministic pass: cruise-setting
/// validation → corridor sampling → per-leg winds → vertical phases (plus
/// the alternate's diversion plan) → fuel ladder → weight & balance →
/// conflicts → nav log → leg summaries. Pure (no IO beyond the source
/// traits), so callers own threading and cancellation.
///
/// Wiring choices, documented:
///
/// - **Endpoint elevations:** *named* endpoints (airports, navaids,
///   reporting points) use the elevation source at the waypoint position
///   (the field-elevation convention); a **free LatLon endpoint** has no
///   published elevation — the aircraft simply stands on the ground there
///   — so it uses the corridor's worst-case terrain at the endpoint
///   station instead, which is exactly the reference the conflict engine
///   compares against (a free-point departure can therefore never start
///   "below terrain"). Outside elevation coverage both fall back to sea
///   level (0 m AMSL). Alternates keep the point query — their one-leg
///   diversion only feeds the fuel ladder, never the conflict engine.
/// - **Alternate fuel** uses the *first* alternate (design §3.4 "alternate
///   leg (if set)"): a one-leg diversion plan destination → alternate at
///   the final route leg's planned altitude (else the flight's cruise
///   altitude — one of the two exists whenever the main plan succeeded).
/// - **W&B and the fuel ladder share one taxi/trip figure** — the ladder's
///   own rungs — so the reports can never disagree about what burns when.
/// - **Frequency suggestions** stay `None`: the [`Sources`] bundle carries
///   no airport data. The app calls [`navlog::build_navlog`] directly with
///   its prefetched route airports when it wants them.
/// - **NOTAM and runway-distance conflicts** are not produced here — their
///   inputs (document snapshot, runway choice) are not part of [`Sources`];
///   the app folds in [`conflict::detect_notam_conflicts`] and
///   [`conflict::runway_margin_conflict`] itself.
///
/// Structural gaps — empty/too-short route, a leg without a resolvable
/// planned altitude — are not errors: they return
/// [`ComputeOutcome::NotComputable`] with the typed reason (the normal
/// state while a route is being built). Every real failure propagates from
/// the owning module as [`ComputeError`].
pub fn compute(
    doc: &FlightDoc,
    aircraft: &AircraftProfile,
    sources: &Sources<'_>,
    params: &ComputeParams,
) -> Result<ComputeOutcome, ComputeError> {
    if let Some(gap) = structural_gap(doc) {
        return Ok(ComputeOutcome::NotComputable(gap));
    }
    let route = doc.route.as_slice();

    // Fail fast on an empty cruise table / unknown power setting; the
    // resolved TAS also feeds the wind sampler's passage-time estimates.
    let cruise = perf::resolve_cruise(aircraft, doc.power_setting.as_deref())?;

    let corridor = corridor::sample_corridor(
        route,
        &params.corridor,
        sources.elevation,
        sources.obstacles,
        sources.airspaces,
    )?;

    let winds = wind::leg_winds(
        route,
        doc.cruise_altitude,
        doc.departure_time,
        cruise.tas,
        sources.winds,
    )?;

    let departure_elevation = endpoint_profile_elevation(
        &route[0],
        corridor.samples.first(),
        sources.elevation,
    )?;
    let destination_elevation = endpoint_profile_elevation(
        &route[route.len() - 1],
        corridor.samples.last(),
        sources.elevation,
    )?;
    let phases = perf::plan_phases(
        route,
        aircraft,
        doc.power_setting.as_deref(),
        doc.cruise_altitude,
        departure_elevation,
        destination_elevation,
    )?;

    let alternate_plan = alternate_phases(doc, aircraft, sources, destination_elevation)?;
    let fuel = fuel::compute_fuel_ladder(
        &doc.fuel_policy,
        aircraft,
        &phases,
        alternate_plan.as_ref(),
        doc.loading.fuel,
    )?;

    // The ladder's taxi/trip rungs are exactly the policy/phase figures —
    // feeding them back keeps W&B and fuel consistent by construction.
    let weight_balance = wb::compute_weight_balance(aircraft, &doc.loading, fuel.taxi, fuel.trip)?;

    let conflicts = conflict::detect_conflicts(
        &corridor,
        &phases,
        &weight_balance,
        &fuel,
        &params.thresholds,
    )?;

    let navlog = navlog::build_navlog(doc, aircraft, &winds, &phases, sources.magvar, &[])?;

    let legs = computed_legs(route, doc, sources)?;

    Ok(ComputeOutcome::Computed(Box::new(ComputedFlight {
        legs,
        corridor,
        winds,
        phases,
        weight_balance,
        fuel,
        conflicts,
        navlog,
    })))
}

/// The first structural gap preventing computation, if any: route shape
/// (empty, then too short/degenerate), then per-leg altitude resolvability
/// — a leg flies at its own [`RouteWaypoint::leg_altitude`], else at the
/// flight's cruise altitude, the exact resolution [`perf::plan_phases`]
/// applies. Pre-validating here keeps the pipeline's own errors honest:
/// once this returns `None`, every leg has an altitude and the alternate
/// diversion (final leg's altitude, else cruise) cannot fail on altitudes
/// either.
fn structural_gap(doc: &FlightDoc) -> Option<NotComputable> {
    let route = doc.route.as_slice();
    if route.is_empty() {
        return Some(NotComputable::NoRoute);
    }
    if route.len() < 2 || route::total_distance(route).0 <= MIN_ROUTE_LENGTH_METERS {
        return Some(NotComputable::RouteTooShort);
    }
    if doc.cruise_altitude.is_none()
        && let Some(leg) = route[..route.len() - 1]
            .iter()
            .position(|w| w.leg_altitude.is_none())
    {
        return Some(NotComputable::MissingAltitude { leg });
    }
    None
}

/// Field elevation at a route endpoint: the max-pooled cell value at the
/// point, sea level outside elevation coverage (the documented fallback —
/// honest for the German coverage area, and conflict detection rides on
/// the corridor's own terrain regardless).
fn endpoint_elevation(
    elevation: &dyn ElevationSource,
    position: LatLon,
) -> Result<MetersAmsl, SourceError> {
    Ok(elevation
        .max_elevation_at(position)?
        .unwrap_or(MetersAmsl(0.0)))
}

/// The elevation the vertical profile starts/ends at, by endpoint kind
/// (see [`compute`]'s wiring notes): named points use the field-elevation
/// point query; a free point starts *on* the corridor's ground, so it uses
/// the corridor's worst-case terrain at the endpoint station — the same
/// reference the clearance checks use, keeping departure/arrival
/// consistent by construction. Falls back to the point query (then sea
/// level) when the endpoint station carries no terrain.
fn endpoint_profile_elevation(
    waypoint: &RouteWaypoint,
    endpoint_sample: Option<&CorridorSample>,
    elevation: &dyn ElevationSource,
) -> Result<MetersAmsl, SourceError> {
    if matches!(waypoint.point, RoutePoint::Free(_))
        && let Some(terrain) = endpoint_sample.and_then(|sample| sample.max_terrain)
    {
        return Ok(terrain);
    }
    endpoint_elevation(elevation, waypoint.position())
}

/// The one-leg diversion plan destination → first alternate backing the
/// fuel ladder's alternate rung (design §3.4 "alternate leg (if set)").
///
/// The diversion flies at the final route leg's planned altitude, falling
/// back to the flight's cruise altitude — one of the two exists whenever
/// the main plan succeeded, so this cannot newly fail on altitudes. A
/// diversion too short for the modeled transitions caps at the
/// climb/descent apex like any other plan ([`perf::plan_phases`]).
fn alternate_phases(
    doc: &FlightDoc,
    aircraft: &AircraftProfile,
    sources: &Sources<'_>,
    destination_elevation: MetersAmsl,
) -> Result<Option<PhasePlan>, ComputeError> {
    let Some(alternate) = doc.alternates.first() else {
        return Ok(None);
    };
    let destination = &doc.route[doc.route.len() - 1];
    let diversion = [
        RouteWaypoint {
            point: destination.point.clone(),
            leg_altitude: doc.route[doc.route.len() - 2].leg_altitude,
            leg_wind: None,
            notes: String::new(),
        },
        RouteWaypoint::new(alternate.clone()),
    ];
    let alternate_elevation = endpoint_elevation(sources.elevation, alternate.position())?;
    Ok(Some(perf::plan_phases(
        &diversion,
        aircraft,
        doc.power_setting.as_deref(),
        doc.cruise_altitude,
        destination_elevation,
        alternate_elevation,
    )?))
}

/// Leg summaries with magnetic tracks: the variation is evaluated at each
/// leg's great-circle midpoint at the departure date — the nav log's
/// convention, including the 2026-01-01 substitute when no departure time
/// is set (variation drifts well under a tenth of a degree per year over
/// Germany, far below display resolution).
fn computed_legs(
    route: &[RouteWaypoint],
    doc: &FlightDoc,
    sources: &Sources<'_>,
) -> Result<Vec<ComputedLeg>, SourceError> {
    let date = doc.departure_time.map_or_else(
        || NaiveDate::from_ymd_opt(2026, 1, 1).expect("constant date is valid"),
        |t| t.date_naive(),
    );
    let mut legs = Vec::with_capacity(route.len().saturating_sub(1));
    for leg in route::legs(route) {
        let geometry = leg.geometry();
        let variation = sources.magvar.magvar(geometry.midpoint, date)?;
        legs.push(ComputedLeg {
            index: leg.index,
            from: leg.from.point.label(),
            to: leg.to.point.label(),
            distance: geometry.distance,
            true_track: geometry.initial_true_track,
            magnetic_track: geometry.initial_true_track.to_magnetic(variation),
            midpoint: geometry.midpoint,
        });
    }
    Ok(legs)
}

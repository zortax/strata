//! NOTAM relevance filtering and ranking for the briefing list (design
//! §3.4 "Briefing", plan §3): pure geometry/time logic over a decoded
//! NOTAM snapshot — no IO, no provider knowledge.
//!
//! Input: the snapshot's NOTAMs, the flight's route + alternates, the
//! sampled [`Corridor`], two time windows and the planned altitude band.
//! Output: the ordered, filtered briefing list ([`RelevantNotam`]).
//!
//! # Filtering
//!
//! 1. **Briefability:** `NOTAMC` cancellations and checklist NOTAMs are
//!    administrative and never listed; a NOTAM that another snapshot entry
//!    cancels (`NOTAMC`) or replaces (`NOTAMR`) is superseded and dropped
//!    (the replacement itself stays — it carries the current text).
//! 2. **Time:** the validity window (items B/C) must intersect the
//!    *briefing window* — the period the briefing covers, typically the
//!    snapshot's fetch window around the flight. `EST` ends count as the
//!    working end (the domain convention, [`NotamEnd::Estimated`]); `PERM`
//!    never expires. Item D schedules are *not* interpreted
//!    (conservative: a NOTAM active only mornings still briefs an
//!    afternoon flight).
//! 3. **Altitude:** the Q-line band must overlap the flight's
//!    [`AltitudeBand`], normalized through the same datum discipline as
//!    the conflict engine (FL × 100 ft as AMSL; `GND`/`UNL` unbounded;
//!    overlap inclusive at the edges). `None` disables the check
//!    (conservative: everything passes).
//! 4. **Geography** — the relevance classes of [`NotamRelevance`], tried
//!    in order; a NOTAM matching none is dropped.
//!
//! # Ordering
//!
//! Aerodrome NOTAMs first, grouped by the aerodrome's position along the
//! flight (departure → en-route → destination → alternates); then corridor
//! NOTAMs by the along-track distance where their circle first comes into
//! reach; then FIR-wide NOTAMs. Ties (same aerodrome / same entry station)
//! break on validity start, then id — fully deterministic.
//!
//! The separate *flight window* (off-blocks → ETA) drives
//! [`RelevantNotam::active_during_flight`]: a briefed NOTAM whose validity
//! misses the actual flight (e.g. starting hours after landing) stays on
//! the list for context but is marked inactive — the badge logic counts
//! only active ones (design §3.1).

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strata_data::domain::{
    IcaoCode, MetersAmsl, Notam, NotamId, NotamKind, QCondition, QSubject,
};

use crate::conflict::{Bound, limit_to_amsl};
use crate::corridor::Corridor;
use crate::flight::{NamedPointKind, RoutePoint, RouteWaypoint};
use crate::perf::PhasePlan;
use crate::route::great_circle_distance;
use crate::units::{METERS_PER_NAUTICAL_MILE, NauticalMiles};

/// Q-line radius that conventionally means "the whole FIR" — non-geometric,
/// so such NOTAMs never classify by corridor distance (they fall through to
/// [`NotamRelevance::Fir`] when filed against the FIR). Same convention as
/// the conflict engine.
const FIR_WIDE_RADIUS_NM: u32 = 999;

/// Half-open UTC time window `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl TimeWindow {
    pub fn new(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Self { from, to }
    }
}

/// The vertical band the flight occupies, AMSL-normalized: ground at the
/// lower edge (the aircraft starts and ends on it), the highest planned
/// altitude at the upper.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AltitudeBand {
    pub floor: MetersAmsl,
    pub ceiling: MetersAmsl,
}

impl AltitudeBand {
    /// The band a phase plan spans: min to max over every segment
    /// endpoint (the climb starts at field elevation, so the floor is the
    /// lower of the two field elevations). `None` for an empty plan.
    pub fn from_phases(phases: &PhasePlan) -> Option<Self> {
        let altitudes = phases
            .segments
            .iter()
            .flat_map(|s| [s.start_altitude.0, s.end_altitude.0]);
        let mut floor = f64::INFINITY;
        let mut ceiling = f64::NEG_INFINITY;
        for altitude in altitudes {
            floor = floor.min(altitude);
            ceiling = ceiling.max(altitude);
        }
        (floor <= ceiling).then_some(Self {
            floor: MetersAmsl(floor),
            ceiling: MetersAmsl(ceiling),
        })
    }
}

/// Why a NOTAM made the briefing list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotamRelevance {
    /// Filed (item A) against an aerodrome of the flight: departure,
    /// destination, an en-route named airport, or an alternate.
    Aerodrome(IcaoCode),
    /// The Q-line circle comes within the corridor half-width of the
    /// route centerline. `distance_nm` is the lateral distance from the
    /// centerline to the circle *edge* (0 when the centerline crosses the
    /// circle).
    RouteCorridor { distance_nm: NauticalMiles },
    /// Filed against the FIR itself with the conventional "whole FIR"
    /// radius (999) — genuinely area-wide, no circle to judge.
    Fir,
}

/// One briefing-list entry: the NOTAM, why it is listed, and whether its
/// validity intersects the actual flight window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelevantNotam {
    pub notam: Notam,
    pub relevance: NotamRelevance,
    /// Items B/C overlap the flight window (off-blocks → ETA). Inactive
    /// entries stay listed for context (e.g. tomorrow's outage at the
    /// destination) but never raise the badge.
    pub active_during_flight: bool,
}

/// Everything [`relevant_notams`] judges against. Borrowed: the caller
/// (the app's briefing state) owns the snapshot and the computed corridor.
#[derive(Debug, Clone, Copy)]
pub struct RelevanceInput<'a> {
    /// The decoded snapshot.
    pub notams: &'a [Notam],
    /// The flight route, departure first (aerodrome matching + order).
    pub route: &'a [RouteWaypoint],
    /// Alternates (aerodrome matching, ordered after the route).
    pub alternates: &'a [RoutePoint],
    /// The sampled corridor of the computed flight.
    pub corridor: &'a Corridor,
    /// What the briefing covers — NOTAMs not valid anywhere inside are
    /// dropped (the "expired / not yet relevant" filter).
    pub briefing_window: TimeWindow,
    /// The flight itself (off-blocks → ETA) — drives
    /// [`RelevantNotam::active_during_flight`].
    pub flight_window: TimeWindow,
    /// Planned altitude band; `None` skips the vertical filter.
    pub altitude_band: Option<AltitudeBand>,
}

/// Filters and ranks `input.notams` into the briefing list. Pure and
/// infallible; see the module docs for the exact filter and order
/// semantics.
pub fn relevant_notams(input: &RelevanceInput<'_>) -> Vec<RelevantNotam> {
    let superseded = superseded_ids(input.notams);
    let aerodromes = route_aerodromes(input.route, input.alternates);

    let mut entries: Vec<(OrderKey, RelevantNotam)> = Vec::new();
    for notam in input.notams {
        if !is_briefable(notam, &superseded) {
            continue;
        }
        if !notam
            .validity
            .overlaps(input.briefing_window.from, input.briefing_window.to)
        {
            continue;
        }
        if !band_overlaps(notam, input.altitude_band) {
            continue;
        }
        let Some((relevance, key)) = classify(notam, &aerodromes, input.corridor) else {
            continue;
        };
        let active_during_flight = notam
            .validity
            .overlaps(input.flight_window.from, input.flight_window.to);
        entries.push((
            key,
            RelevantNotam {
                notam: notam.clone(),
                relevance,
                active_during_flight,
            },
        ));
    }

    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// Whether `notam` is an *activation of an airspace restriction* — Q-code
/// subject group `R` (ED-R/D/P, reservations, TRA/TSA) with an
/// activation-class condition. The class behind the red NOTAM badge
/// (design §3.1: "an active ED-R-activation-class NOTAM intersecting the
/// corridor").
pub fn is_restriction_activation(notam: &Notam) -> bool {
    let subject_group = notam.q.code.subject.code().chars().next();
    subject_group == Some('R')
        && matches!(
            notam.q.code.condition,
            QCondition::Activated | QCondition::WillTakePlace
        )
}

// --- filtering ---------------------------------------------------------------

/// Ids cancelled or replaced by another NOTAM in the snapshot.
fn superseded_ids(notams: &[Notam]) -> HashSet<NotamId> {
    notams
        .iter()
        .filter_map(|notam| match notam.kind {
            NotamKind::Replacement { replaces } => Some(replaces),
            NotamKind::Cancellation { cancels } => Some(cancels),
            NotamKind::New => None,
        })
        .collect()
}

/// Briefing-list material: not a cancellation, not superseded, not a
/// checklist NOTAM (trigger NOTAMs stay — they announce AIP changes and
/// belong in a PIB).
fn is_briefable(notam: &Notam, superseded: &HashSet<NotamId>) -> bool {
    if matches!(notam.kind, NotamKind::Cancellation { .. }) {
        return false;
    }
    if superseded.contains(&notam.id) {
        return false;
    }
    !(matches!(notam.q.code.subject, QSubject::Checklist)
        || matches!(notam.q.code.condition, QCondition::Checklist))
}

/// Q-line band vs flight band, inclusive at the edges (conservative). The
/// Q-line carries no AGL datum, so terrain is not involved — the same
/// normalization the NOTAM conflict check uses.
fn band_overlaps(notam: &Notam, band: Option<AltitudeBand>) -> bool {
    let Some(band) = band else {
        return true;
    };
    let floor = limit_to_amsl(&notam.q.lower, None, Bound::Floor);
    let ceiling = limit_to_amsl(&notam.q.upper, None, Bound::Ceiling);
    floor <= band.ceiling.0 && ceiling >= band.floor.0
}

// --- classification + ordering ------------------------------------------------

/// Composite sort key: relevance group, then the group-specific position,
/// then validity start + id for determinism. `Ord` via the derived
/// lexicographic tuple order; the along-track meters are stored as an
/// integer to keep `Eq`/`Ord` derivable (millimeter resolution — far below
/// station spacing).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OrderKey {
    /// 0 = aerodrome, 1 = corridor, 2 = FIR.
    group: u8,
    /// Aerodrome position along the flight (group 0; 0 otherwise).
    aerodrome_index: usize,
    /// Entry along-track in millimeters (group 1; 0 otherwise).
    along_track_mm: u64,
    from: DateTime<Utc>,
    id: String,
}

impl OrderKey {
    fn new(notam: &Notam, group: u8, aerodrome_index: usize, along_track_m: f64) -> Self {
        Self {
            group,
            aerodrome_index,
            // Corridor tracks are bounded (a VFR route is « 4·10⁹ km),
            // and negative along-tracks cannot occur for stations.
            along_track_mm: (along_track_m.max(0.0) * 1000.0) as u64,
            from: notam.validity.from,
            id: notam.id.to_string(),
        }
    }
}

/// The flight's aerodromes in briefing order: named airport waypoints in
/// route order, then alternates; ids that are not 4-character ICAO
/// indicators (e.g. an airfield referenced by a non-ICAO id) cannot match
/// item A and are skipped, duplicates keep their first position.
///
/// Public because the NOTAM *fetch* scopes its location query with exactly
/// this list — fetching and ranking must agree on what "the flight's
/// aerodromes" means.
pub fn route_aerodromes(route: &[RouteWaypoint], alternates: &[RoutePoint]) -> Vec<IcaoCode> {
    let mut seen = HashSet::new();
    route
        .iter()
        .map(|waypoint| &waypoint.point)
        .chain(alternates.iter())
        .filter_map(|point| match point {
            RoutePoint::Named(named) if named.kind == NamedPointKind::Airport => {
                IcaoCode::new(&named.id).ok()
            }
            _ => None,
        })
        .filter(|code| seen.insert(code.clone()))
        .collect()
}

/// Classifies one NOTAM, in precedence order: aerodrome (item A matches a
/// flight aerodrome) → corridor (Q-line circle within reach of the
/// centerline) → FIR (filed against the FIR itself **and** carrying the
/// non-geometric whole-FIR radius). `None` = not relevant to this flight
/// — in particular a circle that misses the corridor, even when filed
/// against the FIR (design §3.4: relevance-filtered by along-track
/// distance; a parachute area 20 km abeam is noise, GPS jamming over half
/// the FIR is not).
fn classify(
    notam: &Notam,
    aerodromes: &[IcaoCode],
    corridor: &Corridor,
) -> Option<(NotamRelevance, OrderKey)> {
    if let Some(index) = aerodromes
        .iter()
        .position(|aerodrome| notam.locations.contains(aerodrome))
    {
        return Some((
            NotamRelevance::Aerodrome(aerodromes[index].clone()),
            OrderKey::new(notam, 0, index, 0.0),
        ));
    }
    if let Some((distance_nm, entry_along_m)) = corridor_intersection(notam, corridor) {
        return Some((
            NotamRelevance::RouteCorridor { distance_nm },
            OrderKey::new(notam, 1, 0, entry_along_m),
        ));
    }
    if notam.q.radius_nm >= FIR_WIDE_RADIUS_NM && notam.locations.contains(notam.fir()) {
        return Some((NotamRelevance::Fir, OrderKey::new(notam, 2, 0, 0.0)));
    }
    None
}

/// Whether the Q-line circle comes within the corridor half-width of any
/// station's centerline position. Returns the centerline-to-circle-edge
/// distance (0 when a station lies inside the circle) and the along-track
/// of the first station in reach (the briefing order anchor).
///
/// The conventional radius `999` ("whole FIR") is non-geometric and never
/// matches here.
fn corridor_intersection(notam: &Notam, corridor: &Corridor) -> Option<(NauticalMiles, f64)> {
    if notam.q.radius_nm >= FIR_WIDE_RADIUS_NM {
        return None;
    }
    let radius_m = f64::from(notam.q.radius_nm) * METERS_PER_NAUTICAL_MILE;
    let reach = radius_m + corridor.params.half_width.0;

    let mut min_distance = f64::INFINITY;
    let mut entry_along: Option<f64> = None;
    for sample in &corridor.samples {
        let distance = great_circle_distance(sample.station.position, notam.q.centre).0;
        min_distance = min_distance.min(distance);
        if distance <= reach && entry_along.is_none() {
            entry_along = Some(sample.station.along_track.0);
        }
    }
    let entry_along = entry_along?;
    let edge_distance = (min_distance - radius_m).max(0.0);
    Some((
        NauticalMiles(edge_distance / METERS_PER_NAUTICAL_MILE),
        entry_along,
    ))
}

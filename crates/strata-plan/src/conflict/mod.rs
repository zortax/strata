//! The conflict engine (plan §3 `conflict/`): corridor + planned profile +
//! airspace semantics → conflicts. Drives every status badge (design §4);
//! each conflict carries a location so badges can navigate to their cause.
//!
//! Entry points:
//!
//! - [`detect_conflicts`] — terrain/obstacle clearance per station,
//!   airspace penetrations, and the folded-in W&B/fuel states.
//! - [`detect_notam_conflicts`] — Q-line area NOTAMs intersecting the
//!   corridor within the flight window (separate because NOTAMs arrive via
//!   the document snapshot, not the [`Sources`](crate::sources::Sources)
//!   bundle).
//! - [`runway_margin_conflict`] — folds one per-runway distance assessment
//!   (computed via [`crate::perf`]) into a conflict.
//!
//! Datum discipline lives in the `airspace` submodule: AGL limits are
//! evaluated against the per-station corridor terrain, FL limits via the
//! documented standard-atmosphere conversion — never raw value
//! comparisons (plan §7 "datum traps").

mod airspace;
mod clearance;
mod notam;
// `crate::profile` (the drawer-facing series) delegates altitude sampling
// to this module's planned-profile queries.
pub(crate) mod profile;
mod states;
#[cfg(test)]
pub(crate) mod tests;

use serde::{Deserialize, Serialize};
use strata_data::domain::{LatLon, Meters, MetersAgl};
use thiserror::Error;

use crate::corridor::Corridor;
use crate::fuel::FuelLadder;
use crate::perf::PhasePlan;
use crate::wb::WbReport;

pub use notam::detect_notam_conflicts;
pub use states::runway_margin_conflict;

// The datum-normalization helpers are shared with `crate::notam_relevance`
// (Q-line bands vs the flight's altitude band use the same discipline).
pub(crate) use airspace::{Bound, limit_to_amsl};

/// What kind of problem a conflict reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Planned altitude under the terrain clearance buffer.
    Terrain,
    /// Planned altitude under the obstacle clearance buffer.
    Obstacle,
    /// Airspace penetration requiring attention (ED-R/D/P always
    /// [`ConflictSeverity::Warning`]; TMZ/RMZ entries
    /// [`ConflictSeverity::Caution`], design §4).
    Airspace,
    /// A NOTAM area (Q-line circle) intersecting the corridor within the
    /// flight window — drives the NOTAM badge (design §3.1).
    Notam,
    /// A W&B state outside the envelope or over a mass limit.
    WeightBalance,
    /// Loaded fuel below the minimum-required ladder.
    Fuel,
    /// Required runway distance margin below threshold.
    RunwayDistance,
}

/// Severity, ordered: `Info < Caution < Warning` (amber/red badge logic
/// takes the max per kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSeverity {
    Info,
    Caution,
    Warning,
}

/// Where a conflict is anchored — every badge navigates here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictLocation {
    /// A corridor station (terrain/obstacle/airspace conflicts).
    Station {
        along_track: Meters,
        position: LatLon,
    },
    /// A whole leg.
    Leg { index: usize },
    /// Document-level (W&B, fuel, runway distances).
    Flight,
}

/// One conflict, human-readable and navigable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub severity: ConflictSeverity,
    pub location: ConflictLocation,
    /// One-line explanation, e.g. *"enters EDDF CTR (CTR D) at 1.2 NM at
    /// 2300 ft — floor GND, ceiling 3500 ft MSL"*.
    pub message: String,
}

/// Configurable thresholds. Defaults: 1000 ft terrain/obstacle clearance
/// (the design's MSA buffer), runway margin ratio 1.0 (available must
/// cover the factored required distance), 1 NM endpoint grace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConflictThresholds {
    /// Minimum height over corridor worst-case terrain.
    pub terrain_clearance: MetersAgl,
    /// Minimum height over corridor obstacles.
    pub obstacle_clearance: MetersAgl,
    /// Minimum `available / required` runway distance ratio.
    pub min_runway_margin_ratio: f64,
    /// Along-track grace at the route ends: terrain/obstacle clearance is
    /// not evaluated within this distance of the departure while the
    /// **initial climb** is still underway, nor within it of the
    /// destination during the **final descent** (clearance checks make no
    /// sense over the field the aircraft is lifting off from or letting
    /// down onto — the corridor's worst-case terrain there is the
    /// departure environment itself). A genuine obstruction beyond the
    /// grace still conflicts at the ramped buffer; airspace checks are
    /// never exempted. `0` disables the grace.
    pub endpoint_grace_distance: Meters,
}

impl Default for ConflictThresholds {
    fn default() -> Self {
        Self {
            terrain_clearance: MetersAgl::from_feet(1000.0),
            obstacle_clearance: MetersAgl::from_feet(1000.0),
            min_runway_margin_ratio: 1.0,
            endpoint_grace_distance: Meters(crate::units::METERS_PER_NAUTICAL_MILE),
        }
    }
}

/// Errors from conflict evaluation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConflictError {
    #[error("conflict evaluation input incomplete: {0}")]
    IncompleteInput(&'static str),
}

/// Evaluates every conflict class against the computed inputs:
///
/// 1. **Terrain/obstacle clearance** per corridor station against the
///    planned altitude profile, with the configured buffer ramping through
///    the initial climb and final descent (`clearance` submodule);
///    contiguous violating stations merge into one conflict anchored at
///    the worst.
/// 2. **Airspace penetrations** per corridor crossing where the planned
///    altitude lies inside the datum-normalized vertical band (`airspace`
///    submodule: the AGL/FL discipline and the severity table).
/// 3. **W&B / fuel** states folded in from their reports.
///
/// NOTAM and runway-distance conflicts have their own entry points (see
/// the module docs); a corridor with samples but an empty phase plan is
/// rejected as inconsistent (no altitude to judge stations against).
///
/// Output order: terrain, obstacles, airspace (each in along-track
/// order), then document-level conflicts.
pub fn detect_conflicts(
    corridor: &Corridor,
    phases: &PhasePlan,
    wb: &WbReport,
    fuel: &FuelLadder,
    thresholds: &ConflictThresholds,
) -> Result<Vec<Conflict>, ConflictError> {
    if !corridor.samples.is_empty() && phases.segments.is_empty() {
        return Err(ConflictError::IncompleteInput(
            "corridor has stations but the phase plan has no segments",
        ));
    }

    let mut conflicts = Vec::new();
    if !phases.segments.is_empty() {
        conflicts.extend(clearance::terrain_conflicts(corridor, phases, thresholds));
        conflicts.extend(clearance::obstacle_conflicts(corridor, phases, thresholds));
        conflicts.extend(airspace::airspace_conflicts(corridor, phases));
    }
    conflicts.extend(states::wb_conflicts(wb));
    conflicts.extend(states::fuel_conflicts(fuel));
    Ok(conflicts)
}

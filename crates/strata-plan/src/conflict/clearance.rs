//! Terrain and obstacle clearance checks per corridor station.
//!
//! A station violates when `planned altitude − reference elevation <
//! required buffer`, where the reference is the corridor's worst-case
//! terrain (or tallest obstacle top) at that station and the required
//! buffer is the configured clearance scaled by the climb/descent ramp
//! (`profile::clearance_ramp`). The comparison is strict: a clearance of
//! exactly the buffer is *not* a conflict.
//!
//! **Endpoint grace** (`ConflictThresholds::endpoint_grace_distance`,
//! default 1 NM): stations within the grace of the departure are exempt
//! while the initial climb is still underway, mirrored at the destination
//! during the final descent — the corridor's worst-case terrain right at
//! the route ends is the departure/arrival environment itself (the
//! corridor is several NM wide), not an obstruction the climb-out must
//! out-clear. Beyond the grace the ramped buffer applies as usual, so a
//! genuine ridge a few NM out still conflicts.
//!
//! Consecutive violating stations merge into **one** conflict anchored at
//! the worst station (largest buffer deficit), so a 10 km ridge yields one
//! navigable conflict, not twenty per-station duplicates.

use strata_data::domain::{FEET_PER_METER, MetersAmsl, ObstacleKind};

use crate::corridor::{Corridor, CorridorSample};
use crate::perf::PhasePlan;
use crate::units::METERS_PER_NAUTICAL_MILE;

use super::profile;
use super::{Conflict, ConflictKind, ConflictLocation, ConflictSeverity, ConflictThresholds};

/// Float guard so that "clearance exactly equals the buffer" never trips a
/// conflict through rounding noise.
const MARGIN_EPSILON: f64 = 1e-9;

/// One violating station inside a run.
struct Violation {
    sample_index: usize,
    /// `required buffer − actual clearance` (> 0 by construction).
    deficit: f64,
    /// Planned altitude − reference elevation (negative = below it).
    clearance: f64,
    reference: MetersAmsl,
}

pub(crate) fn terrain_conflicts(
    corridor: &Corridor,
    phases: &PhasePlan,
    thresholds: &ConflictThresholds,
) -> Vec<Conflict> {
    check_stations(
        corridor,
        phases,
        thresholds,
        thresholds.terrain_clearance.0,
        |sample| sample.max_terrain,
        terrain_message,
        ConflictKind::Terrain,
    )
}

pub(crate) fn obstacle_conflicts(
    corridor: &Corridor,
    phases: &PhasePlan,
    thresholds: &ConflictThresholds,
) -> Vec<Conflict> {
    check_stations(
        corridor,
        phases,
        thresholds,
        thresholds.obstacle_clearance.0,
        |sample| sample.tallest_obstacle.as_ref().map(|o| o.elevation_top),
        obstacle_message,
        ConflictKind::Obstacle,
    )
}

/// Whether the station at `along_track` sits inside the endpoint grace
/// (see the module docs): within the grace distance of the profile start
/// *and* still inside the initial climb, or the mirrored condition at the
/// destination during the final descent.
fn in_endpoint_grace(phases: &PhasePlan, along_track: f64, grace_m: f64) -> bool {
    if grace_m <= 0.0 {
        return false;
    }
    if let Some((climb_start, climb_end)) = profile::initial_climb(phases)
        && along_track < climb_end.0
        && along_track - climb_start.0 < grace_m
    {
        return true;
    }
    if let Some((descent_start, descent_end)) = profile::final_descent(phases)
        && along_track > descent_start.0
        && descent_end.0 - along_track < grace_m
    {
        return true;
    }
    false
}

fn check_stations(
    corridor: &Corridor,
    phases: &PhasePlan,
    thresholds: &ConflictThresholds,
    buffer_m: f64,
    reference: impl Fn(&CorridorSample) -> Option<MetersAmsl>,
    message: impl Fn(&[Violation], &Corridor) -> String,
    kind: ConflictKind,
) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    let mut run: Vec<Violation> = Vec::new();
    let mut last_index: Option<usize> = None;

    let flush = |run: &mut Vec<Violation>, conflicts: &mut Vec<Conflict>| {
        if run.is_empty() {
            return;
        }
        let worst = worst_of(run);
        let station = corridor.samples[worst.sample_index].station;
        conflicts.push(Conflict {
            kind,
            severity: ConflictSeverity::Warning,
            location: ConflictLocation::Station {
                along_track: station.along_track,
                position: station.position,
            },
            message: message(run, corridor),
        });
        run.clear();
    };

    let grace_m = thresholds.endpoint_grace_distance.0;
    for (index, sample) in corridor.samples.iter().enumerate() {
        let violation = reference(sample).and_then(|reference| {
            let x = sample.station.along_track;
            if in_endpoint_grace(phases, x.0, grace_m) {
                return None;
            }
            let planned = profile::altitude_at(phases, x)?;
            let required = buffer_m * profile::clearance_ramp(phases, x);
            let clearance = planned.0 - reference.0;
            (clearance < required - MARGIN_EPSILON).then_some(Violation {
                sample_index: index,
                deficit: required - clearance,
                clearance,
                reference,
            })
        });

        match violation {
            Some(v) => {
                // Merge only directly consecutive stations.
                if last_index.is_some_and(|last| index != last + 1) {
                    flush(&mut run, &mut conflicts);
                }
                last_index = Some(index);
                run.push(v);
            }
            None => {
                flush(&mut run, &mut conflicts);
                last_index = None;
            }
        }
    }
    flush(&mut run, &mut conflicts);
    conflicts
}

fn run_extent(run: &[Violation], corridor: &Corridor) -> (f64, f64) {
    let first = corridor.samples[run[0].sample_index].station.along_track.0;
    let last = corridor.samples[run[run.len() - 1].sample_index]
        .station
        .along_track
        .0;
    (
        first / METERS_PER_NAUTICAL_MILE,
        last / METERS_PER_NAUTICAL_MILE,
    )
}

fn worst_of(run: &[Violation]) -> &Violation {
    run.iter()
        .max_by(|a, b| a.deficit.total_cmp(&b.deficit))
        .expect("run is non-empty")
}

fn terrain_message(run: &[Violation], corridor: &Corridor) -> String {
    let worst = worst_of(run);
    let (from_nm, to_nm) = run_extent(run, corridor);
    let at_nm = corridor.samples[worst.sample_index]
        .station
        .along_track
        .0
        / METERS_PER_NAUTICAL_MILE;
    let terrain_ft = worst.reference.as_feet().round();
    if worst.clearance < 0.0 {
        format!(
            "terrain up to {terrain_ft:.0} ft is above the planned altitude at {at_nm:.1} NM \
             (affected {from_nm:.1}–{to_nm:.1} NM)"
        )
    } else {
        let clearance_ft = (worst.clearance * FEET_PER_METER).round();
        format!(
            "terrain clearance {clearance_ft:.0} ft over {terrain_ft:.0} ft terrain at \
             {at_nm:.1} NM (affected {from_nm:.1}–{to_nm:.1} NM)"
        )
    }
}

fn obstacle_message(run: &[Violation], corridor: &Corridor) -> String {
    let worst = worst_of(run);
    let sample = &corridor.samples[worst.sample_index];
    let at_nm = sample.station.along_track.0 / METERS_PER_NAUTICAL_MILE;
    let label = sample
        .tallest_obstacle
        .as_ref()
        .map(|o| {
            let kind = obstacle_label(o.kind);
            match o.name.as_deref() {
                Some(name) => format!("{kind} {name:?}"),
                None => kind.to_owned(),
            }
        })
        .unwrap_or_else(|| "obstacle".to_owned());
    let top_ft = worst.reference.as_feet().round();
    if worst.clearance < 0.0 {
        format!("{label} (top {top_ft:.0} ft) is above the planned altitude at {at_nm:.1} NM")
    } else {
        let clearance_ft = (worst.clearance * FEET_PER_METER).round();
        format!("{label} (top {top_ft:.0} ft) only {clearance_ft:.0} ft below at {at_nm:.1} NM")
    }
}

fn obstacle_label(kind: ObstacleKind) -> &'static str {
    match kind {
        ObstacleKind::WindTurbine => "wind turbine",
        ObstacleKind::Antenna => "antenna",
        ObstacleKind::Mast => "mast",
        ObstacleKind::Tower => "tower",
        ObstacleKind::Chimney => "chimney",
        ObstacleKind::Building => "building",
        ObstacleKind::PowerLine => "power line",
        ObstacleKind::Crane => "crane",
        ObstacleKind::Bridge => "bridge",
        ObstacleKind::Other(_) => "obstacle",
    }
}

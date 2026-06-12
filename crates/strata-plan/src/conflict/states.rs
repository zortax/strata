//! Document-level conflicts folded in from already-computed states:
//! weight & balance, fuel ladder, runway distance margins.

use strata_data::domain::Meters;

use crate::fuel::FuelLadder;
use crate::wb::{WbReport, WbStateKind};

use super::{Conflict, ConflictKind, ConflictLocation, ConflictSeverity, ConflictThresholds};

/// One Warning per W&B state outside its envelope/limits.
pub(crate) fn wb_conflicts(wb: &WbReport) -> Vec<Conflict> {
    wb.states
        .iter()
        .filter(|state| !state.within_envelope)
        .map(|state| Conflict {
            kind: ConflictKind::WeightBalance,
            severity: ConflictSeverity::Warning,
            location: ConflictLocation::Flight,
            message: format!(
                "{} mass {:.0} kg at arm {:.3} m is outside the CG envelope or over a limit",
                state_label(state.kind),
                state.mass.0,
                state.arm.0
            ),
        })
        .collect()
}

fn state_label(kind: WbStateKind) -> &'static str {
    match kind {
        WbStateKind::Ramp => "ramp",
        WbStateKind::Takeoff => "takeoff",
        WbStateKind::ZeroFuel => "zero-fuel",
        WbStateKind::Landing => "landing",
    }
}

/// A Warning when loaded fuel is under the ladder minimum (strictly
/// negative margin; exactly meeting the minimum is not a conflict).
pub(crate) fn fuel_conflicts(fuel: &FuelLadder) -> Vec<Conflict> {
    if fuel.margin.0 < -1e-9 {
        vec![Conflict {
            kind: ConflictKind::Fuel,
            severity: ConflictSeverity::Warning,
            location: ConflictLocation::Flight,
            message: format!(
                "loaded fuel {:.1} L is {:.1} L under the minimum required {:.1} L",
                fuel.loaded.0,
                -fuel.margin.0,
                fuel.minimum_required.0
            ),
        }]
    } else {
        Vec::new()
    }
}

/// Folds one runway-distance assessment into a conflict when the
/// available/required ratio is below
/// [`ConflictThresholds::min_runway_margin_ratio`].
///
/// Lives here (not in [`detect_conflicts`](super::detect_conflicts))
/// because runway selection and the corrected distances are inputs the
/// caller computes per runway via [`crate::perf::takeoff_distance`] /
/// [`crate::perf::landing_distance`]; `required` is the *factored*
/// distance, `available` the declared length. `required ≤ 0` (no data)
/// yields no conflict.
pub fn runway_margin_conflict(
    runway: &str,
    required: Meters,
    available: Meters,
    thresholds: &ConflictThresholds,
) -> Option<Conflict> {
    if required.0 <= 0.0 {
        return None;
    }
    let ratio = available.0 / required.0;
    if ratio >= thresholds.min_runway_margin_ratio - 1e-12 {
        return None;
    }
    Some(Conflict {
        kind: ConflictKind::RunwayDistance,
        severity: ConflictSeverity::Warning,
        location: ConflictLocation::Flight,
        message: format!(
            "runway {runway}: {:.0} m available vs {:.0} m required (ratio {ratio:.2}, minimum {:.2})",
            available.0, required.0, thresholds.min_runway_margin_ratio
        ),
    })
}

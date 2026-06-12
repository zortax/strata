//! Planned-altitude profile queries shared by the conflict checks.
//!
//! The [`PhasePlan`] is the single source of truth for "how high are we at
//! along-track distance *x*": gap-free segments with linear altitude change
//! within each segment.

use strata_data::domain::{Meters, MetersAmsl};

use crate::perf::{PhaseKind, PhasePlan};

/// Planned altitude at `along_track`, linearly interpolated within the
/// containing phase segment. Positions before the first / after the last
/// segment clamp to the profile's end altitudes (tolerates float spillover
/// at the route ends). `None` only for an empty phase plan.
pub(crate) fn altitude_at(phases: &PhasePlan, along_track: Meters) -> Option<MetersAmsl> {
    let segments = &phases.segments;
    let first = segments.first()?;
    let x = along_track.0;
    if x <= first.start_along_track.0 {
        return Some(first.start_altitude);
    }
    for seg in segments {
        if x <= seg.end_along_track.0 {
            let span = seg.end_along_track.0 - seg.start_along_track.0;
            if span <= 0.0 {
                return Some(seg.end_altitude);
            }
            let fraction = (x - seg.start_along_track.0) / span;
            let alt = seg.start_altitude.0 + fraction * (seg.end_altitude.0 - seg.start_altitude.0);
            return Some(MetersAmsl(alt));
        }
    }
    segments.last().map(|seg| seg.end_altitude)
}

/// The initial climb's along-track interval — the contiguous run of climb
/// segments starting the profile. `None` when the profile doesn't start
/// with a climb.
pub(crate) fn initial_climb(phases: &PhasePlan) -> Option<(Meters, Meters)> {
    let segments = &phases.segments;
    let first = segments.first()?;
    if first.kind != PhaseKind::Climb {
        return None;
    }
    let start = first.start_along_track;
    let mut end = start;
    for seg in segments {
        if seg.kind != PhaseKind::Climb {
            break;
        }
        end = seg.end_along_track;
    }
    Some((start, end))
}

/// The final descent's along-track interval — the contiguous run of
/// descent segments ending the profile. `None` when the profile doesn't
/// end with a descent.
pub(crate) fn final_descent(phases: &PhasePlan) -> Option<(Meters, Meters)> {
    let segments = &phases.segments;
    let last = segments.last()?;
    if last.kind != PhaseKind::Descent {
        return None;
    }
    let end = last.end_along_track;
    let mut start = end;
    for seg in segments.iter().rev() {
        if seg.kind != PhaseKind::Descent {
            break;
        }
        start = seg.start_along_track;
    }
    Some((start, end))
}

/// Clearance-buffer ramp factor in `[0, 1]` at `along_track`.
///
/// The full terrain/obstacle clearance buffer cannot apply at the runway:
/// the planned profile starts and ends *on* the field, so a flat buffer
/// would flag every departure and arrival. Documented heuristic: within
/// the **initial climb** (the contiguous run of climb segments starting the
/// profile) the required buffer ramps linearly from 0 at the departure to
/// the full value at top of climb; the **final descent** (contiguous run of
/// descent segments ending the profile) mirrors that towards the
/// destination. Everywhere else — cruise, en-route step climbs/descents —
/// the full buffer applies, so a ridge near TOC/TOD is still caught at a
/// proportionally reduced (but non-zero) buffer.
pub(crate) fn clearance_ramp(phases: &PhasePlan, along_track: Meters) -> f64 {
    let x = along_track.0;
    let mut factor: f64 = 1.0;

    if let Some((climb_start, climb_end)) = initial_climb(phases) {
        let span = climb_end.0 - climb_start.0;
        if span > 0.0 && x < climb_end.0 {
            factor = factor.min(((x - climb_start.0) / span).clamp(0.0, 1.0));
        }
    }

    if let Some((descent_start, descent_end)) = final_descent(phases) {
        let span = descent_end.0 - descent_start.0;
        if span > 0.0 && x > descent_start.0 {
            factor = factor.min(((descent_end.0 - x) / span).clamp(0.0, 1.0));
        }
    }

    factor
}

#[cfg(test)]
mod tests {
    use strata_data::domain::Meters;

    use super::*;
    use crate::conflict::tests::{climb_cruise_descent_plan, cruise_only_plan};

    #[test]
    fn altitude_interpolates_within_segments() {
        // Plan: climb 0→10 km from 0 m to 1000 m, cruise 10→30 km at
        // 1000 m, descent 30→40 km back to 0 m.
        let plan = climb_cruise_descent_plan(40_000.0, 10_000.0, 10_000.0, 1000.0);
        // Mid-climb at 5 km: 0 + 5/10 × 1000 = 500 m.
        assert_eq!(altitude_at(&plan, Meters(5_000.0)), Some(MetersAmsl(500.0)));
        // Cruise at 20 km: 1000 m.
        assert_eq!(
            altitude_at(&plan, Meters(20_000.0)),
            Some(MetersAmsl(1000.0))
        );
        // Mid-descent at 35 km: 1000 − 5/10 × 1000 = 500 m.
        assert_eq!(
            altitude_at(&plan, Meters(35_000.0)),
            Some(MetersAmsl(500.0))
        );
        // Clamped beyond the ends.
        assert_eq!(altitude_at(&plan, Meters(-1.0)), Some(MetersAmsl(0.0)));
        assert_eq!(altitude_at(&plan, Meters(40_001.0)), Some(MetersAmsl(0.0)));
    }

    #[test]
    fn empty_plan_has_no_altitude() {
        let plan = crate::perf::PhasePlan {
            segments: Vec::new(),
            toc: None,
            tod: None,
            total_duration: crate::units::Minutes(0.0),
            total_fuel: crate::units::Liters(0.0),
        };
        assert_eq!(altitude_at(&plan, Meters(0.0)), None);
    }

    #[test]
    fn ramp_is_linear_in_initial_climb_and_final_descent() {
        let plan = climb_cruise_descent_plan(40_000.0, 10_000.0, 10_000.0, 1000.0);
        assert!((clearance_ramp(&plan, Meters(0.0)) - 0.0).abs() < 1e-12);
        assert!((clearance_ramp(&plan, Meters(5_000.0)) - 0.5).abs() < 1e-12);
        assert!((clearance_ramp(&plan, Meters(10_000.0)) - 1.0).abs() < 1e-12);
        assert!((clearance_ramp(&plan, Meters(20_000.0)) - 1.0).abs() < 1e-12);
        // Descent 30→40 km: at 35 km half the buffer remains.
        assert!((clearance_ramp(&plan, Meters(35_000.0)) - 0.5).abs() < 1e-12);
        assert!((clearance_ramp(&plan, Meters(40_000.0)) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn cruise_only_plan_never_ramps() {
        let plan = cruise_only_plan(20_000.0, 700.0);
        for x in [0.0, 1.0, 10_000.0, 20_000.0] {
            assert_eq!(clearance_ramp(&plan, Meters(x)), 1.0);
        }
    }
}

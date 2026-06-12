//! NOTAM-area conflicts: Q-line circles intersecting the corridor during
//! the flight window.
//!
//! A NOTAM becomes a corridor conflict when **all** of the following hold:
//!
//! 1. **Relevance:** its Q-code subject group is `R` (airspace
//!    restrictions — ED-R/D/P activations, reservations) or `W`
//!    (navigation warnings — jumping, firing, UAS, balloons …), or any
//!    other group whose condition is *activated* / *will take place*
//!    (e.g. a CTR activation). Aerodrome/facility NOTAMs (runway closures,
//!    lighting, ILS …) are briefing-list material, not corridor geometry,
//!    and are skipped here. `NOTAMC` cancellations and checklist/trigger
//!    administrative NOTAMs never conflict.
//! 2. **Time:** its validity overlaps the flight window (half-open,
//!    `NotamValidity::overlaps` semantics). Item D daily schedules are
//!    *not* interpreted (conservative: a NOTAM active only 0800–1200
//!    daily still flags an afternoon flight).
//! 3. **Lateral:** the Q-line circle (centre + radius) comes within the
//!    corridor half-width of any station's centerline position. The
//!    conventional "whole FIR" radius `999` is treated as non-geometric
//!    and skipped — FIR-wide NOTAMs belong in the briefing list.
//! 4. **Vertical:** the planned altitude at an intersecting station lies
//!    within the Q-line band (FL × 100 ft as AMSL, standard-atmosphere
//!    assumption; `GND`/`UNL` unbounded — the same normalization as the
//!    airspace checks). With an empty phase plan the vertical check is
//!    skipped conservatively (lateral hit ⇒ conflict).
//!
//! Severity: subject group `R` ⇒ [`ConflictSeverity::Warning`] (ED-R/D/P
//! always red, design §4); everything else ⇒ [`ConflictSeverity::Caution`].

use chrono::{DateTime, Utc};
use strata_data::domain::{Notam, NotamKind, QCondition, QSubject};

use crate::corridor::Corridor;
use crate::perf::PhasePlan;
use crate::route::great_circle_distance;
use crate::units::METERS_PER_NAUTICAL_MILE;

use super::airspace::{Bound, limit_to_amsl};
use super::profile;
use super::{Conflict, ConflictKind, ConflictLocation, ConflictSeverity};

/// Radius value that conventionally means "the whole FIR".
const FIR_WIDE_RADIUS_NM: u32 = 999;

/// Detects NOTAM-area conflicts for the flight window
/// `[window_from, window_to)` (typically departure time to ETA). Pure and
/// infallible: NOTAMs come pre-parsed (e.g. from the document's NOTAM
/// snapshot), the corridor and phases from the compute pipeline.
pub fn detect_notam_conflicts(
    corridor: &Corridor,
    phases: &PhasePlan,
    notams: &[Notam],
    window_from: DateTime<Utc>,
    window_to: DateTime<Utc>,
) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    for notam in notams {
        if !is_area_relevant(notam) {
            continue;
        }
        if !notam.validity.overlaps(window_from, window_to) {
            continue;
        }
        if notam.q.radius_nm >= FIR_WIDE_RADIUS_NM {
            continue;
        }
        let Some(sample_index) = first_intersecting_station(corridor, phases, notam) else {
            continue;
        };
        let station = corridor.samples[sample_index].station;
        let at_nm = station.along_track.0 / METERS_PER_NAUTICAL_MILE;
        conflicts.push(Conflict {
            kind: ConflictKind::Notam,
            severity: severity(notam),
            location: ConflictLocation::Station {
                along_track: station.along_track,
                position: station.position,
            },
            message: format!(
                "NOTAM {} ({}) within corridor at {at_nm:.1} NM — {}",
                notam.id,
                q_label(notam),
                summary(notam),
            ),
        });
    }
    conflicts
}

/// Subject groups that describe *areas* a route can hit, plus activations.
fn is_area_relevant(notam: &Notam) -> bool {
    if matches!(notam.kind, NotamKind::Cancellation { .. }) {
        return false;
    }
    let subject = &notam.q.code.subject;
    let condition = &notam.q.code.condition;
    if matches!(subject, QSubject::Checklist)
        || matches!(condition, QCondition::Checklist | QCondition::Trigger)
    {
        return false;
    }
    match subject_group(subject) {
        Some('R') | Some('W') => true,
        _ => matches!(
            condition,
            QCondition::Activated | QCondition::WillTakePlace
        ),
    }
}

fn subject_group(subject: &QSubject) -> Option<char> {
    subject.code().chars().next()
}

fn severity(notam: &Notam) -> ConflictSeverity {
    match subject_group(&notam.q.code.subject) {
        Some('R') => ConflictSeverity::Warning,
        _ => ConflictSeverity::Caution,
    }
}

/// First station whose centerline position lies within
/// `radius + corridor half-width` of the Q-line centre *and* where the
/// planned altitude is inside the Q-line vertical band.
fn first_intersecting_station(
    corridor: &Corridor,
    phases: &PhasePlan,
    notam: &Notam,
) -> Option<usize> {
    let reach = f64::from(notam.q.radius_nm) * METERS_PER_NAUTICAL_MILE + corridor.params.half_width.0;
    corridor.samples.iter().position(|sample| {
        let distance = great_circle_distance(sample.station.position, notam.q.centre);
        if distance.0 > reach {
            return false;
        }
        // Vertical: skipped (conservative) when there is no profile. The
        // Q-line band has no AGL datum, so no terrain is involved.
        match profile::altitude_at(phases, sample.station.along_track) {
            None => true,
            Some(planned) => {
                let floor = limit_to_amsl(&notam.q.lower, None, Bound::Floor);
                let ceiling = limit_to_amsl(&notam.q.upper, None, Bound::Ceiling);
                floor <= planned.0 && planned.0 <= ceiling
            }
        }
    })
}

/// `"restricted area activated"` — decoded Q-code, falling back to the raw
/// codes for unknown subjects/conditions.
fn q_label(notam: &Notam) -> String {
    let subject = notam
        .q
        .code
        .subject
        .description()
        .map_or_else(|| notam.q.code.subject.code().to_owned(), str::to_owned);
    let condition = notam
        .q
        .code
        .condition
        .description()
        .map_or_else(|| notam.q.code.condition.code().to_owned(), str::to_owned);
    format!("{subject} {condition}")
}

/// First line of item E, truncated on a char boundary.
fn summary(notam: &Notam) -> String {
    const MAX: usize = 80;
    let line = notam.text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= MAX {
        line.to_owned()
    } else {
        let truncated: String = line.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

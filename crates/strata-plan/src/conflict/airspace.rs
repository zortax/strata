//! Airspace penetration: corridor crossing intervals × the planned
//! altitude profile against **datum-normalized** floor/ceiling.
//!
//! Datum discipline (the dangerous-bug area, plan §7 "datum traps"):
//!
//! - **AGL limits are evaluated against the per-station corridor terrain**,
//!   never as raw numbers — an AGL floor over sloping terrain is a sloped
//!   band edge, so the same cruise altitude can be inside the volume over a
//!   valley and below it over a ridge.
//! - **FL limits** are converted at `FL × 100 ft` treating pressure
//!   altitude as AMSL (standard-atmosphere assumption; QNH deviation is a
//!   couple of hundred feet at most and the published caveat lives in the
//!   UI, design §3.3).
//! - `GND` floors and `UNL` ceilings are unbounded.
//!
//! Penetration is **inclusive** at both limits: flying exactly at a floor
//! or ceiling counts as inside (the conservative reading for a planning
//! aid).

use strata_data::domain::{
    Airspace, AirspaceClass, AirspaceKind, MetersAmsl, VerticalLimit, VerticalReference,
};

use crate::corridor::{AirspaceCrossing, Corridor};
use crate::perf::PhasePlan;
use crate::units::METERS_PER_NAUTICAL_MILE;

use super::profile;
use super::{Conflict, ConflictKind, ConflictLocation, ConflictSeverity};

/// Which bound of a volume a limit describes — controls the conservative
/// fallback when terrain is unknown for an AGL limit (flag more, not less:
/// unknown terrain pins an AGL *floor* to ground level and an AGL
/// *ceiling* to unlimited).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bound {
    Floor,
    Ceiling,
}

/// Normalizes a vertical limit to meters AMSL for comparison against the
/// planned altitude. `terrain` is the bound-appropriate corridor terrain
/// statistic at the station being evaluated (AGL limits ride on it):
/// **lowest** across the corridor width for floors, **highest** for
/// ceilings — each the choice that flags more, not less.
pub(crate) fn limit_to_amsl(
    limit: &VerticalLimit,
    terrain: Option<MetersAmsl>,
    bound: Bound,
) -> f64 {
    match limit.reference {
        VerticalReference::Fl(level) => MetersAmsl::from_feet(f64::from(level) * 100.0).0,
        VerticalReference::Amsl(m) => m.0,
        VerticalReference::Agl(h) => match (terrain, bound) {
            (Some(t), _) => t.0 + h.0,
            (None, Bound::Floor) => h.0, // terrain unknown: assume sea level
            (None, Bound::Ceiling) => f64::INFINITY,
        },
        VerticalReference::Gnd => match bound {
            Bound::Floor => f64::NEG_INFINITY,
            Bound::Ceiling => terrain.map_or(0.0, |t| t.0),
        },
        VerticalReference::Unl => f64::INFINITY,
    }
}

/// Severity of penetrating `airspace`, or `None` when entry is not a
/// conflict for a VFR flight (class E/F/G and pure information sectors).
///
/// Documented mapping (design §4):
/// - **Warning (red):** ED-P/R/D — always — plus class A (VFR prohibited).
/// - **Caution (amber):** clearance- or equipment-bound airspace — CTR/
///   MCTR, TMZ/RMZ ("amber-informational"), TRA/TSA/MTA, ADIZ, overflight
///   restrictions, and class B/C/D volumes generally.
/// - **Info:** traffic-pattern/activity areas worth a look (ATZ, MATZ,
///   HTZ, TIZ/TIA, glider/parachute/recreational areas, alert/warning/
///   protected areas).
/// - **None:** FIR/UIR, FIS/VFR/ACC sectors, airways, routes and anything
///   class E/F/G/unclassified without a listed kind — legal to enter VFR,
///   no badge noise.
pub(crate) fn airspace_severity(airspace: &Airspace) -> Option<ConflictSeverity> {
    use AirspaceKind as K;
    match airspace.kind {
        K::Prohibited | K::Restricted | K::Danger => Some(ConflictSeverity::Warning),
        K::Ctr
        | K::Mctr
        | K::Tmz
        | K::Rmz
        | K::Tra
        | K::Tsa
        | K::MilitaryTrainingArea
        | K::Adiz
        | K::OverflightRestriction => Some(ConflictSeverity::Caution),
        K::Atz
        | K::Matz
        | K::Htz
        | K::Tiz
        | K::Tia
        | K::GliderSector
        | K::ParachuteJumpArea
        | K::RecreationalActivity
        | K::AlertArea
        | K::WarningArea
        | K::ProtectedArea => Some(ConflictSeverity::Info),
        K::Fir
        | K::Uir
        | K::FisSector
        | K::VfrSector
        | K::AccSector
        | K::Airway
        | K::MilitaryTrainingRoute
        | K::MilitaryRoute
        | K::TsaTraFeedingRoute
        | K::TransponderSetting
        | K::LowerTrafficArea
        | K::UpperTrafficArea => None,
        K::Area | K::Tma | K::Cta | K::Other(_) => match airspace.class {
            AirspaceClass::A => Some(ConflictSeverity::Warning),
            AirspaceClass::B | AirspaceClass::C | AirspaceClass::D => {
                Some(ConflictSeverity::Caution)
            }
            AirspaceClass::E
            | AirspaceClass::F
            | AirspaceClass::G
            | AirspaceClass::Unclassified => None,
        },
    }
}

/// Indices into `corridor.samples` of the stations (within the crossing's
/// along-track interval) where the planned altitude lies inside the
/// volume's vertical band. AGL limits use each station's own terrain, so
/// the band edge follows the slope.
pub(crate) fn penetrating_stations(
    crossing: &AirspaceCrossing,
    corridor: &Corridor,
    phases: &PhasePlan,
) -> Vec<usize> {
    corridor
        .samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| {
            let x = sample.station.along_track;
            if x.0 < crossing.entry_along_track.0 || x.0 > crossing.exit_along_track.0 {
                return false;
            }
            let Some(planned) = profile::altitude_at(phases, x) else {
                return false;
            };
            let floor = limit_to_amsl(&crossing.airspace.lower, sample.min_terrain, Bound::Floor);
            let ceiling =
                limit_to_amsl(&crossing.airspace.upper, sample.max_terrain, Bound::Ceiling);
            floor <= planned.0 && planned.0 <= ceiling
        })
        .map(|(index, _)| index)
        .collect()
}

pub(crate) fn airspace_conflicts(corridor: &Corridor, phases: &PhasePlan) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    for crossing in &corridor.crossings {
        let Some(severity) = airspace_severity(&crossing.airspace) else {
            continue;
        };
        let stations = penetrating_stations(crossing, corridor, phases);
        let Some(&first) = stations.first() else {
            continue;
        };
        let station = corridor.samples[first].station;
        let planned = profile::altitude_at(phases, station.along_track)
            .map_or(0.0, |alt| alt.as_feet().round());
        let at_nm = station.along_track.0 / METERS_PER_NAUTICAL_MILE;
        let airspace = &crossing.airspace;
        let label = airspace_label(airspace);
        conflicts.push(Conflict {
            kind: ConflictKind::Airspace,
            severity,
            location: ConflictLocation::Station {
                along_track: station.along_track,
                position: station.position,
            },
            message: format!(
                "enters {} ({label}) at {at_nm:.1} NM at {planned:.0} ft — floor {}, ceiling {}",
                airspace.name, airspace.lower, airspace.upper
            ),
        });
    }
    conflicts
}

/// `"CTR D"` for classed volumes, the kind alone for unclassified ones.
fn airspace_label(airspace: &Airspace) -> String {
    if airspace.class == AirspaceClass::Unclassified {
        airspace.kind.to_string()
    } else {
        format!("{} {}", airspace.kind, airspace.class)
    }
}

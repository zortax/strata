//! Profile-drawer series (design §3.3, plan §5.2): pure, view-facing
//! helpers deriving what the custom-painted profile view needs from a
//! [`ComputedFlight`](crate::compute::ComputedFlight)'s parts — the view
//! does layout/paint only.
//!
//! Already-shaped series come straight off the computed flight: the
//! per-station along-track distance, worst-case terrain and tallest
//! obstacle from [`Corridor::samples`]; the planned-altitude polyline with
//! TOC/TOD from [`PhasePlan::segments`]; the crossings' along-track
//! intervals from [`Corridor::crossings`]. This module adds what is *not*
//! stored:
//!
//! - [`crossing_bands`] / [`station_band`] — a crossing's vertical band
//!   **datum-resolved at each station**, so AGL/GND edges follow the
//!   terrain silhouette (the design's sloped band edges) and FL edges use
//!   the standard-atmosphere convention (the QNH caveat is the view's
//!   annotation to draw).
//! - [`planned_altitude_at`] — the planned polyline sampled at an
//!   arbitrary along-track distance (scrub readout, drag feedback).
//! - [`msa_per_leg`] — the minimum-safe-altitude reference line: corridor
//!   worst case (terrain *and* obstacle tops) per leg plus a buffer.
//! - [`freezing_level_estimate`] / [`freezing_levels`] — the ISA-lapse
//!   freezing-level series from the legs' sampled temperatures.
//!
//! **Drawing vs conflict semantics, documented:** the conflict engine
//! resolves unknowns conservatively (unknown terrain pins an AGL floor to
//! sea level but an AGL ceiling to *unlimited*, GND floors to −∞ — flag
//! more, not less). A drawn band needs finite, honest edges instead: GND
//! floors sit **on** the station's worst-case terrain (the drawn
//! silhouette), AGL *floors* ride the corridor's **lowest** terrain `+ h`
//! (matching the conflict engine, so a flagged penetration never sits
//! visually below the drawn band floor), AGL *ceilings* ride the highest
//! terrain `+ h` (sea level where terrain is unknown, both bounds), UNL
//! ceilings are `None` for the view to cap at the chart top. Penetration
//! judgement stays with [`crate::conflict`] — these bands are for drawing
//! and the hover stack only.

#[cfg(test)]
mod tests;

use strata_data::domain::{Meters, MetersAgl, MetersAmsl, VerticalLimit, VerticalReference};

use crate::corridor::{AirspaceCrossing, Corridor, CorridorSample};
use crate::flight::{PlannedAltitude, RouteWaypoint};
use crate::perf::{ISA_LAPSE_CELSIUS_PER_METER, PhasePlan, planned_altitude_amsl};
use crate::units::Celsius;
use crate::wind::LegWind;

/// One airspace crossing's vertical band at one corridor station, resolved
/// to AMSL for drawing (see the module docs for the resolution rules).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationBand {
    /// The station's along-track distance from departure.
    pub along_track: Meters,
    /// Band floor at this station. GND sits on the station's worst-case
    /// terrain, AGL rides the corridor's *lowest* terrain `+ h` (sea level
    /// where terrain is unknown), FL uses `FL × 100 ft` on the standard
    /// atmosphere. A (malformed) UNL floor resolves to `+∞`, keeping the
    /// band honestly empty.
    pub floor: MetersAmsl,
    /// Band ceiling, resolved like the floor; `None` = unlimited (UNL) —
    /// the view caps it at the chart top.
    pub ceiling: Option<MetersAmsl>,
}

/// The crossing's band at one station — the hover-stack building block.
pub fn station_band(crossing: &AirspaceCrossing, sample: &CorridorSample) -> StationBand {
    StationBand {
        along_track: sample.station.along_track,
        floor: floor_amsl(&crossing.airspace.lower, sample),
        ceiling: ceiling_amsl(&crossing.airspace.upper, sample.max_terrain),
    }
}

/// The crossing's band sampled at every corridor station inside its
/// along-track interval (inclusive at both ends, matching the conflict
/// engine's station filter), in along-track order — the polygon the view
/// fills, with AGL/GND edges following the per-station terrain.
pub fn crossing_bands(corridor: &Corridor, crossing: &AirspaceCrossing) -> Vec<StationBand> {
    corridor
        .samples
        .iter()
        .filter(|sample| {
            let x = sample.station.along_track.0;
            crossing.entry_along_track.0 <= x && x <= crossing.exit_along_track.0
        })
        .map(|sample| station_band(crossing, sample))
        .collect()
}

/// Planned altitude at `along_track`, linearly interpolated within the
/// containing phase segment; positions beyond the profile clamp to its end
/// altitudes. `None` only for an empty phase plan. (The same query the
/// conflict engine judges stations with.)
pub fn planned_altitude_at(phases: &PhasePlan, along_track: Meters) -> Option<MetersAmsl> {
    crate::conflict::profile::altitude_at(phases, along_track)
}

/// Minimum safe altitude per route leg: the worst case over the leg's
/// stations — max terrain *and* obstacle tops, corridor-wide by
/// construction — plus `buffer` (design §3.3; the default buffer is the
/// conflict engine's 1000 ft clearance). Indexed by leg; `None` for legs
/// without any terrain/obstacle data (outside elevation coverage, or legs
/// shorter than the station spacing that received no station). A station
/// on a leg boundary counts toward the following leg, the corridor's
/// station convention.
pub fn msa_per_leg(corridor: &Corridor, buffer: MetersAgl) -> Vec<Option<MetersAmsl>> {
    let legs = corridor
        .samples
        .iter()
        .map(|sample| sample.station.leg_index + 1)
        .max()
        .unwrap_or(0);
    let mut msa: Vec<Option<MetersAmsl>> = vec![None; legs];
    for sample in &corridor.samples {
        let terrain = sample.max_terrain.map(|t| t.0);
        let obstacle = sample.tallest_obstacle.as_ref().map(|o| o.elevation_top.0);
        let Some(reference) = terrain.into_iter().chain(obstacle).reduce(f64::max) else {
            continue;
        };
        let candidate = reference + buffer.0;
        let slot = &mut msa[sample.station.leg_index];
        *slot = Some(MetersAmsl(
            slot.map_or(candidate, |current| current.0.max(candidate)),
        ));
    }
    msa
}

/// Freezing-level estimate from one sampled temperature at altitude,
/// extrapolated along the ISA lapse rate (6.5 °C/km): the altitude where
/// 0 °C is reached. A below-zero sample yields a level *below* its
/// altitude (possibly below ground or sea level in winter — honest; the
/// view clamps to its chart). A planning hint, never an icing forecast.
pub fn freezing_level_estimate(temperature: Celsius, altitude: MetersAmsl) -> MetersAmsl {
    MetersAmsl(altitude.0 + temperature.0 / ISA_LAPSE_CELSIUS_PER_METER)
}

/// Per-leg freezing-level estimates from the computed leg winds: each
/// leg's sampled temperature (manual overrides carry an ISA estimate, so
/// they yield the ISA freezing level) extrapolated from the leg's planned
/// altitude. `None` where the leg has no resolvable altitude or no wind
/// entry. Indexed by leg.
pub fn freezing_levels(
    route: &[RouteWaypoint],
    cruise_altitude: Option<PlannedAltitude>,
    winds: &[LegWind],
) -> Vec<Option<MetersAmsl>> {
    (0..route.len().saturating_sub(1))
        .map(|index| {
            let altitude = route[index]
                .leg_altitude
                .or(cruise_altitude)
                .map(planned_altitude_amsl)?;
            let wind = winds.iter().find(|w| w.leg_index == index)?;
            Some(freezing_level_estimate(wind.wind.temperature, altitude))
        })
        .collect()
}

/// Floor of `limit` at a station (drawing semantics — see the module
/// docs): AGL floors ride the corridor's lowest terrain, matching the
/// conflict engine; GND floors sit on the drawn silhouette (worst case).
fn floor_amsl(limit: &VerticalLimit, sample: &CorridorSample) -> MetersAmsl {
    match limit.reference {
        VerticalReference::Fl(level) => MetersAmsl::from_feet(f64::from(level) * 100.0),
        VerticalReference::Amsl(m) => m,
        VerticalReference::Agl(h) => {
            MetersAmsl(sample.min_terrain.unwrap_or(MetersAmsl(0.0)).0 + h.0)
        }
        VerticalReference::Gnd => sample.max_terrain.unwrap_or(MetersAmsl(0.0)),
        // A floor of UNL is malformed data; +∞ keeps the band empty
        // instead of inventing a volume.
        VerticalReference::Unl => MetersAmsl(f64::INFINITY),
    }
}

/// Ceiling of `limit` at a station with worst-case `terrain`; `None` =
/// unlimited.
fn ceiling_amsl(limit: &VerticalLimit, terrain: Option<MetersAmsl>) -> Option<MetersAmsl> {
    let ground = terrain.unwrap_or(MetersAmsl(0.0));
    Some(match limit.reference {
        VerticalReference::Fl(level) => MetersAmsl::from_feet(f64::from(level) * 100.0),
        VerticalReference::Amsl(m) => m,
        VerticalReference::Agl(h) => MetersAmsl(ground.0 + h.0),
        VerticalReference::Gnd => ground,
        VerticalReference::Unl => return None,
    })
}

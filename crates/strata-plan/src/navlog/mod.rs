//! Nav log (PLOG) assembly (plan §3 `navlog/`): per-leg rows from
//! route + wind + performance + magnetic variation, cumulative times/fuel,
//! frequency suggestions, totals.
//!
//! Row semantics are documented on the `rows` submodule (interval walk,
//! ETE/fuel sources, TOC/TOD splitting); the frequency-suggestion
//! heuristic on the `frequency` submodule.

mod frequency;
mod rows;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strata_data::domain::{Airport, Frequency};
use thiserror::Error;

use crate::aircraft::AircraftProfile;
use crate::flight::{FlightDoc, PlannedAltitude};
use crate::perf::PhasePlan;
use crate::sources::{MagvarSource, SourceError, WindsAloft};
use crate::units::{DegreesMagnetic, DegreesTrue, Knots, Liters, Minutes, NauticalMiles};
use crate::wind::LegWind;

/// What a nav-log row represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavLogRowKind {
    Waypoint,
    TopOfClimb,
    TopOfDescent,
}

/// One PLOG row: the waypoint (or TOC/TOD) plus the values of the **leg
/// arriving at it**. The departure row carries `None` leg values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavLogRow {
    pub kind: NavLogRowKind,
    /// Waypoint label (ident/name) or "TOC"/"TOD".
    pub label: String,
    /// Planned altitude *at* this point.
    pub altitude: Option<PlannedAltitude>,
    pub true_track: Option<DegreesTrue>,
    pub magnetic_track: Option<DegreesMagnetic>,
    /// The leg's wind (direction/speed/temperature).
    pub wind: Option<WindsAloft>,
    /// Wind correction angle, degrees, positive = right.
    pub wind_correction_angle_deg: Option<f64>,
    pub magnetic_heading: Option<DegreesMagnetic>,
    pub tas: Option<Knots>,
    pub ground_speed: Option<Knots>,
    pub distance: Option<NauticalMiles>,
    pub ete: Option<Minutes>,
    pub eta: Option<DateTime<Utc>>,
    pub leg_fuel: Option<Liters>,
    pub cumulative_fuel: Option<Liters>,
    /// Fuel remaining on board at this point.
    pub remaining_fuel: Option<Liters>,
    /// Suggested frequency (nearest relevant FIS/TWR from airport data).
    pub frequency: Option<Frequency>,
    /// Pilot-editable notes, seeded from the waypoint's stored
    /// [`RouteWaypoint::notes`] (empty for TOC/TOD rows). The app edits
    /// the document field; the row carries the copy for display/export.
    ///
    /// [`RouteWaypoint::notes`]: crate::flight::RouteWaypoint::notes
    pub notes: String,
}

/// Totals row of the PLOG.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NavLogTotals {
    pub distance: NauticalMiles,
    pub ete: Minutes,
    pub fuel: Liters,
}

/// The assembled nav log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavLog {
    /// Departure row first, then one row per waypoint/TOC/TOD in
    /// along-track order.
    pub rows: Vec<NavLogRow>,
    pub totals: NavLogTotals,
}

/// Errors from nav-log assembly.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NavLogError {
    #[error("navlog inputs disagree: {0}")]
    InconsistentInput(&'static str),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Assembles the PLOG. Magnetic values use the variation at each leg's
/// great-circle midpoint at the flight date (design §4) — without a
/// departure time, 2026-01-01 substitutes (variation drifts well under a
/// tenth of a degree per year over Germany, far below PLOG resolution).
/// `airports` are the route-relevant airports (app-prefetched) for
/// frequency suggestions.
///
/// TAS comes from the document's cruise power setting (`None` = the
/// profile's first); a named setting that does not exist in the profile is
/// an [`NavLogError::InconsistentInput`], as are a route with fewer than
/// two waypoints and a phase plan that does not span the route's length
/// (±1 %).
pub fn build_navlog(
    doc: &FlightDoc,
    aircraft: &AircraftProfile,
    winds: &[LegWind],
    phases: &PhasePlan,
    magvar: &dyn MagvarSource,
    airports: &[Airport],
) -> Result<NavLog, NavLogError> {
    let setting = match doc.power_setting.as_deref() {
        Some(name) => Some(
            aircraft
                .performance
                .cruise_settings
                .iter()
                .find(|s| s.name == name)
                .ok_or(NavLogError::InconsistentInput(
                    "flight references a power setting the aircraft profile does not have",
                ))?,
        ),
        None => aircraft.performance.cruise_settings.first(),
    };
    let taxi_fuel = doc.fuel_policy.taxi.as_hours() * aircraft.performance.taxi_fuel_flow.0;
    rows::assemble(&rows::Inputs {
        doc,
        winds,
        phases,
        magvar,
        airports,
        tas: setting.map(|s| s.tas),
        taxi_fuel,
    })
}

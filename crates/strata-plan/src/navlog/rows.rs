//! PLOG row assembly: the checkpoint walk.
//!
//! The route's waypoints plus the TOC/TOD markers form a sorted list of
//! **checkpoints** along the track; every checkpoint becomes one row whose
//! values describe the interval *arriving* at it from the previous one.
//! Splitting at TOC/TOD keeps row distances summing exactly to the route
//! total while the markers appear in sequence (design §3.3 "Nav Log").
//!
//! Documented semantics (see also [`build_navlog`](super::build_navlog)):
//!
//! - **ETE** uses the arriving leg's wind-triangle ground speed over the
//!   interval distance (wind-corrected). Where a leg has no resolved wind
//!   the phase plan's time share over the interval substitutes (no-wind),
//!   and failing that the cruise TAS.
//! - **Fuel** always comes from the phase plan, distributed over the
//!   interval proportionally to along-track overlap with each segment —
//!   climb fuel lands before TOC, descent fuel after TOD. (Phase times
//!   are no-wind by construction; the row ETE is wind-corrected. The two
//!   columns answer different questions and may disagree slightly.)
//! - **TAS** is the planned cruise TAS (the wind agent solves every leg
//!   at cruise TAS; climb/descent speeds live in the profile view).
//! - **Remaining fuel** = loaded fuel − taxi fuel − cumulative burn,
//!   shown only when the loading scenario carries fuel.

use chrono::Duration;
use strata_data::domain::{Airport, LatLon};

use crate::flight::{FlightDoc, PlannedAltitude, RoutePoint};
use crate::perf::{PhasePlan, ProfileMarker};
use crate::route::{LegGeometry, leg_geometry};
use crate::sources::MagvarSource;
use crate::units::{Knots, Liters, MagneticVariation, Minutes, NauticalMiles};
use crate::wind::LegWind;

use super::{NavLog, NavLogError, NavLogRow, NavLogRowKind, NavLogTotals, frequency};

pub(crate) struct Inputs<'a> {
    pub doc: &'a FlightDoc,
    pub winds: &'a [LegWind],
    pub phases: &'a PhasePlan,
    pub magvar: &'a dyn MagvarSource,
    pub airports: &'a [Airport],
    /// Cruise TAS from the resolved power setting, if any.
    pub tas: Option<Knots>,
    /// Taxi fuel in liters (policy time × profile taxi flow).
    pub taxi_fuel: f64,
}

struct Checkpoint {
    kind: NavLogRowKind,
    label: String,
    along: f64,
    position: LatLon,
    /// Index of the leg arriving at this checkpoint.
    leg_index: usize,
    /// Route index for waypoint checkpoints (`None` for TOC/TOD) — the
    /// row's notes come from that waypoint's stored notes.
    route_index: Option<usize>,
    /// ICAO id when the checkpoint is a named airport waypoint.
    airport_ident: Option<String>,
    /// Last waypoint of the route?
    is_destination: bool,
}

pub(crate) fn assemble(inputs: &Inputs<'_>) -> Result<NavLog, NavLogError> {
    let route = &inputs.doc.route;
    if route.len() < 2 {
        return Err(NavLogError::InconsistentInput(
            "route has fewer than two waypoints",
        ));
    }

    // Leg geometry + cumulative along-track distances.
    let geometries: Vec<LegGeometry> = route
        .windows(2)
        .map(|pair| leg_geometry(pair[0].position(), pair[1].position()))
        .collect();
    let mut cumulative = vec![0.0; route.len()];
    for (i, geometry) in geometries.iter().enumerate() {
        cumulative[i + 1] = cumulative[i] + geometry.distance.0;
    }
    let total = cumulative[route.len() - 1];

    // The phase plan, when present, must span this route.
    if let Some(last) = inputs.phases.segments.last() {
        let span = last.end_along_track.0;
        let tolerance = (total * 0.01).max(1.0);
        if (span - total).abs() > tolerance {
            return Err(NavLogError::InconsistentInput(
                "phase plan does not span the route",
            ));
        }
    }

    // Checkpoints: waypoints 1.. plus TOC/TOD, sorted along track (stable:
    // a marker coinciding with a waypoint sorts after it).
    let mut checkpoints: Vec<Checkpoint> = Vec::new();
    for (i, waypoint) in route.iter().enumerate().skip(1) {
        checkpoints.push(Checkpoint {
            kind: NavLogRowKind::Waypoint,
            label: waypoint.point.label(),
            along: cumulative[i],
            position: waypoint.position(),
            leg_index: i - 1,
            route_index: Some(i),
            airport_ident: airport_ident(&waypoint.point),
            is_destination: i == route.len() - 1,
        });
    }
    let marker_checkpoint = |kind: NavLogRowKind, label: &str, marker: &ProfileMarker| {
        let leg_index = leg_at(&cumulative, marker.along_track.0);
        Checkpoint {
            kind,
            label: label.to_owned(),
            along: marker.along_track.0,
            position: marker.position,
            leg_index,
            route_index: None,
            airport_ident: None,
            is_destination: false,
        }
    };
    if let Some(toc) = &inputs.phases.toc {
        checkpoints.push(marker_checkpoint(NavLogRowKind::TopOfClimb, "TOC", toc));
    }
    if let Some(tod) = &inputs.phases.tod {
        checkpoints.push(marker_checkpoint(NavLogRowKind::TopOfDescent, "TOD", tod));
    }
    checkpoints.sort_by(|a, b| a.along.total_cmp(&b.along));

    // Per-leg wind and (lazily memoized) magnetic variation.
    let wind_for = |leg: usize| inputs.winds.iter().find(|w| w.leg_index == leg);
    let date = inputs
        .doc
        .departure_time
        .map(|t| t.date_naive())
        .unwrap_or_else(|| {
            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("constant date is valid")
        });
    let mut variations: Vec<Option<MagneticVariation>> = vec![None; geometries.len()];
    let mut variation_for = |leg: usize| -> Result<MagneticVariation, NavLogError> {
        if let Some(v) = variations[leg] {
            return Ok(v);
        }
        let v = inputs.magvar.magvar(geometries[leg].midpoint, date)?;
        variations[leg] = Some(v);
        Ok(v)
    };

    let has_fuel_load = inputs.doc.loading.fuel.0 > 0.0;
    let mut rows = Vec::with_capacity(checkpoints.len() + 1);
    rows.push(departure_row(
        route[0].point.label(),
        route[0].notes.clone(),
    ));

    let mut prev_along = 0.0;
    let mut cumulative_ete: Option<f64> = Some(0.0);
    let mut cumulative_fuel = 0.0;
    let mut total_distance_nm = 0.0;

    for checkpoint in &checkpoints {
        let distance_m = (checkpoint.along - prev_along).max(0.0);
        let distance_nm = distance_m / crate::units::METERS_PER_NAUTICAL_MILE;
        total_distance_nm += distance_nm;

        let leg = checkpoint.leg_index;
        let geometry = &geometries[leg];
        let variation = variation_for(leg)?;
        let true_track = geometry.initial_true_track;
        let magnetic_track = true_track.to_magnetic(variation);
        let wind = wind_for(leg);

        let (wind_aloft, wca, magnetic_heading, ground_speed) = match wind {
            Some(w) => (
                Some(w.wind),
                Some(w.triangle.wind_correction_angle_deg),
                Some(w.triangle.true_heading.to_magnetic(variation)),
                Some(w.triangle.ground_speed),
            ),
            None => (None, None, None, None),
        };

        // ETE: wind-corrected GS, else phase time share, else cruise TAS.
        let ete_min = match ground_speed {
            Some(gs) if gs.0 > 0.0 => Some(distance_nm / gs.0 * 60.0),
            _ if !inputs.phases.segments.is_empty() => Some(share_over(
                inputs.phases,
                prev_along,
                checkpoint.along,
                |seg| seg.duration.0,
            )),
            _ => match inputs.tas {
                Some(tas) if tas.0 > 0.0 => Some(distance_nm / tas.0 * 60.0),
                _ => None,
            },
        };
        cumulative_ete = match (cumulative_ete, ete_min) {
            (Some(acc), Some(ete)) => Some(acc + ete),
            _ => None,
        };
        let eta = match (inputs.doc.departure_time, cumulative_ete) {
            (Some(dep), Some(minutes)) => {
                Some(dep + Duration::milliseconds((minutes * 60_000.0).round() as i64))
            }
            _ => None,
        };

        // Fuel from the phase plan's along-track distribution.
        let leg_fuel = (!inputs.phases.segments.is_empty()).then(|| {
            share_over(inputs.phases, prev_along, checkpoint.along, |seg| {
                seg.fuel.0
            })
        });
        if let Some(fuel) = leg_fuel {
            cumulative_fuel += fuel;
        }
        let remaining = (has_fuel_load && leg_fuel.is_some()).then_some(Liters(
            inputs.doc.loading.fuel.0 - inputs.taxi_fuel - cumulative_fuel,
        ));

        let altitude = row_altitude(inputs, checkpoint, route);

        rows.push(NavLogRow {
            kind: checkpoint.kind,
            label: checkpoint.label.clone(),
            altitude,
            notes: checkpoint
                .route_index
                .map(|i| route[i].notes.clone())
                .unwrap_or_default(),
            true_track: Some(true_track),
            magnetic_track: Some(magnetic_track),
            wind: wind_aloft,
            wind_correction_angle_deg: wca,
            magnetic_heading,
            tas: inputs.tas,
            ground_speed,
            distance: Some(NauticalMiles(distance_nm)),
            ete: ete_min.map(Minutes),
            eta,
            leg_fuel: leg_fuel.map(Liters),
            cumulative_fuel: leg_fuel.map(|_| Liters(cumulative_fuel)),
            remaining_fuel: remaining,
            frequency: frequency::suggest(
                inputs.airports,
                checkpoint.position,
                checkpoint.airport_ident.as_deref(),
            ),
        });

        prev_along = checkpoint.along;
    }

    let totals = NavLogTotals {
        distance: NauticalMiles(total_distance_nm),
        ete: Minutes(rows.iter().filter_map(|r| r.ete).map(|e| e.0).sum()),
        fuel: inputs.phases.total_fuel,
    };
    Ok(NavLog { rows, totals })
}

/// The departure row: label and the departure waypoint's notes only —
/// every leg value is `None` by contract (the frozen [`NavLogRow`]
/// semantics).
fn departure_row(label: String, notes: String) -> NavLogRow {
    NavLogRow {
        kind: NavLogRowKind::Waypoint,
        label,
        notes,
        altitude: None,
        true_track: None,
        magnetic_track: None,
        wind: None,
        wind_correction_angle_deg: None,
        magnetic_heading: None,
        tas: None,
        ground_speed: None,
        distance: None,
        ete: None,
        eta: None,
        leg_fuel: None,
        cumulative_fuel: None,
        remaining_fuel: None,
        frequency: None,
    }
}

/// Planned altitude *at* a checkpoint: TOC/TOD carry their marker
/// altitude; the destination lands at the profile's end altitude; other
/// waypoints show the arriving leg's planned altitude (leg override or
/// the flight's cruise default).
fn row_altitude(
    inputs: &Inputs<'_>,
    checkpoint: &Checkpoint,
    route: &[crate::flight::RouteWaypoint],
) -> Option<PlannedAltitude> {
    match checkpoint.kind {
        NavLogRowKind::TopOfClimb => inputs
            .phases
            .toc
            .map(|m| PlannedAltitude::Amsl(m.altitude)),
        NavLogRowKind::TopOfDescent => inputs
            .phases
            .tod
            .map(|m| PlannedAltitude::Amsl(m.altitude)),
        NavLogRowKind::Waypoint => {
            if checkpoint.is_destination
                && let Some(last) = inputs.phases.segments.last()
            {
                return Some(PlannedAltitude::Amsl(last.end_altitude));
            }
            route[checkpoint.leg_index]
                .leg_altitude
                .or(inputs.doc.cruise_altitude)
        }
    }
}

/// Index of the leg containing along-track position `x`.
fn leg_at(cumulative: &[f64], x: f64) -> usize {
    debug_assert!(cumulative.len() >= 2);
    let legs = cumulative.len() - 1;
    for i in 0..legs {
        if x <= cumulative[i + 1] {
            return i;
        }
    }
    legs - 1
}

/// Sums `value(segment) × overlap fraction` over the interval `(a, b]` —
/// distributes per-segment quantities (time, fuel) proportionally to
/// along-track overlap.
fn share_over(
    phases: &PhasePlan,
    a: f64,
    b: f64,
    value: impl Fn(&crate::perf::PhaseSegment) -> f64,
) -> f64 {
    phases
        .segments
        .iter()
        .map(|seg| {
            let span = seg.end_along_track.0 - seg.start_along_track.0;
            if span <= 0.0 {
                return 0.0;
            }
            let overlap = (seg.end_along_track.0.min(b) - seg.start_along_track.0.max(a)).max(0.0);
            value(seg) * overlap / span
        })
        .sum()
}

fn airport_ident(point: &RoutePoint) -> Option<String> {
    match point {
        RoutePoint::Named(named)
            if named.kind == crate::flight::NamedPointKind::Airport =>
        {
            Some(named.id.clone())
        }
        _ => None,
    }
}

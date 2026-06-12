//! Vertical profile construction: climb/cruise/descent splitting with
//! TOC/TOD (plan §3 `perf/`, design §3.3 "planned altitude line").
//!
//! **Model, documented:**
//!
//! - Each leg has a planned altitude (its own override, else the flight's
//!   cruise altitude). Altitude changes *start at the waypoint* where the
//!   plan changes (climb/descend after passing it); only the **final
//!   descent** (or climb) to the destination elevation is positioned
//!   backwards from the route end — that intersection is the TOD.
//! - Climb and descent are single-segment linear models from the profile
//!   (`rate`, `ias`, `fuel_flow`); the climb/descent gradient along track
//!   is `rate / ias`. **IAS is used as TAS** (planning approximation: at
//!   VFR climb altitudes TAS exceeds IAS by a few percent, so the profile
//!   reaches altitude slightly *earlier* than computed — and wind is not
//!   applied here; per-leg wind affects times in the nav log, not the
//!   geometric profile).
//! - Where the route is too short to contain a transition, the climb is
//!   capped where it meets the final descent line (TOC/TOD are `None`:
//!   no cruise altitude is reached). Where even an immediate full-route
//!   transition cannot reach the destination elevation at the modeled
//!   gradient, a single direct (steeper-than-modeled) segment is emitted
//!   with a `tracing::warn` — the conflict engine sees the profile either
//!   way.

use strata_data::domain::{LatLon, Meters, MetersAmsl};

use crate::aircraft::{AircraftProfile, PowerSetting};
use crate::flight::{PlannedAltitude, RouteWaypoint};
use crate::route;
use crate::units::{
    FeetPerMinute, Knots, Liters, LitersPerHour, METERS_PER_NAUTICAL_MILE, Minutes,
};
use crate::wind::LegWind;

use super::isa::planned_altitude_amsl;
use super::{PerfError, PhaseKind, PhasePlan, PhaseSegment, ProfileMarker};

/// Altitudes closer than this (meters) are the same level.
const ALT_EPS: f64 = 0.01;

/// Along-track positions closer than this (meters) coincide.
const DIST_EPS: f64 = 1e-6;

/// Splits the route into climb/cruise/descent segments with TOC/TOD and
/// per-segment time/fuel, from departure elevation over the per-leg
/// planned altitudes to destination elevation. See the module docs for the
/// model. Fewer than two waypoints (or a zero-length route) yield an empty
/// plan.
pub fn plan_phases(
    route: &[RouteWaypoint],
    aircraft: &AircraftProfile,
    power_setting: Option<&str>,
    cruise_altitude: Option<PlannedAltitude>,
    departure_elevation: MetersAmsl,
    destination_elevation: MetersAmsl,
) -> Result<PhasePlan, PerfError> {
    if route.len() < 2 {
        return Ok(empty_plan());
    }

    let cruise = resolve_cruise(aircraft, power_setting)?;

    // Per-leg planned altitudes (meters AMSL) and cumulative leg
    // boundaries (meters along track).
    let mut targets = Vec::with_capacity(route.len() - 1);
    for (index, waypoint) in route[..route.len() - 1].iter().enumerate() {
        let planned = waypoint
            .leg_altitude
            .or(cruise_altitude)
            .ok_or(PerfError::NoPlannedAltitude(index))?;
        targets.push(planned_altitude_amsl(planned).0);
    }

    let mut boundaries = Vec::with_capacity(route.len());
    boundaries.push(0.0_f64);
    let mut accumulated = 0.0_f64;
    for leg in route::legs(route) {
        accumulated += leg.geometry().distance.0;
        boundaries.push(accumulated);
    }
    let total = accumulated;
    if total <= DIST_EPS {
        return Ok(empty_plan());
    }

    let h_dep = departure_elevation.0;
    let h_dest = destination_elevation.0;
    let performance = &aircraft.performance;

    // ----- Forward pass: follow the per-leg targets from the departure
    // elevation; transitions start at the waypoint where the plan changes.
    let mut points: Vec<(f64, f64)> = vec![(0.0, h_dep)];
    let mut altitude = h_dep;
    for (index, &target) in targets.iter().enumerate() {
        let leg_start = boundaries[index];
        let leg_end = boundaries[index + 1];
        if leg_end - leg_start <= DIST_EPS {
            continue;
        }
        if target > altitude + ALT_EPS {
            let model = climb_model(performance)?;
            let reach = leg_start + (target - altitude) / model.gradient;
            if reach <= leg_end {
                push_point(&mut points, reach, target);
                altitude = target;
            } else {
                altitude += model.gradient * (leg_end - leg_start);
            }
        } else if target < altitude - ALT_EPS {
            let model = descent_model(performance)?;
            let reach = leg_start + (altitude - target) / model.gradient;
            if reach <= leg_end {
                push_point(&mut points, reach, target);
                altitude = target;
            } else {
                altitude -= model.gradient * (leg_end - leg_start);
            }
        }
        push_point(&mut points, leg_end, altitude);
    }

    // ----- Final transition to the destination elevation, positioned
    // backwards from the route end.
    let end_altitude = points.last().map(|&(_, h)| h).unwrap_or(h_dep);
    if end_altitude > h_dest + ALT_EPS {
        let model = descent_model(performance)?;
        let line = |x: f64| h_dest + (total - x) * model.gradient;
        if line(0.0) < h_dep - ALT_EPS {
            // Even an immediate descent at the modeled gradient cannot get
            // down in time.
            tracing::warn!(
                total_m = total,
                "route too short for the modeled descent; using a direct steeper segment"
            );
            return Ok(direct_plan(
                total,
                h_dep,
                h_dest,
                &model,
                PhaseKind::Descent,
            ));
        }
        points = clip_with_line(&points, line, true);
    } else if end_altitude < h_dest - ALT_EPS {
        let model = climb_model(performance)?;
        let line = |x: f64| h_dest - (total - x) * model.gradient;
        if line(0.0) > h_dep + ALT_EPS {
            tracing::warn!(
                total_m = total,
                "route too short for the modeled climb; using a direct steeper segment"
            );
            return Ok(direct_plan(total, h_dep, h_dest, &model, PhaseKind::Climb));
        }
        points = clip_with_line(&points, line, false);
    } else if let Some(last) = points.last_mut() {
        // Within tolerance — snap exactly.
        last.1 = h_dest;
    }

    let points = merge_collinear(points);

    // ----- Segments.
    let mut segments = Vec::with_capacity(points.len().saturating_sub(1));
    for pair in points.windows(2) {
        let (x0, h0) = pair[0];
        let (x1, h1) = pair[1];
        let dx = x1 - x0;
        let dh = h1 - h0;
        if dx <= DIST_EPS && dh.abs() <= ALT_EPS {
            continue;
        }
        let segment = if dh > ALT_EPS {
            let model = climb_model(performance)?;
            vertical_segment(PhaseKind::Climb, x0, x1, h0, h1, &model)
        } else if dh < -ALT_EPS {
            let model = descent_model(performance)?;
            vertical_segment(PhaseKind::Descent, x0, x1, h0, h1, &model)
        } else {
            let duration = Minutes::from_hours(dx / METERS_PER_NAUTICAL_MILE / cruise.tas.0);
            PhaseSegment {
                kind: PhaseKind::Cruise,
                start_along_track: Meters(x0),
                end_along_track: Meters(x1),
                start_altitude: MetersAmsl(h0),
                end_altitude: MetersAmsl(h0),
                tas: cruise.tas,
                duration,
                fuel: Liters(cruise.fuel_flow.0 * duration.as_hours()),
            }
        };
        segments.push(segment);
    }

    // ----- TOC/TOD markers (initial climb into cruise / final descent out
    // of cruise; an apex profile that never levels has neither).
    let toc = match (segments.first(), segments.get(1)) {
        (Some(first), Some(second))
            if first.kind == PhaseKind::Climb && second.kind == PhaseKind::Cruise =>
        {
            Some(marker_at(
                route,
                &boundaries,
                first.end_along_track.0,
                first.end_altitude,
            ))
        }
        _ => None,
    };
    let tod = match segments.len().checked_sub(2) {
        Some(before_last)
            if segments[before_last + 1].kind == PhaseKind::Descent
                && segments[before_last].kind == PhaseKind::Cruise =>
        {
            let last = &segments[before_last + 1];
            Some(marker_at(
                route,
                &boundaries,
                last.start_along_track.0,
                last.start_altitude,
            ))
        }
        _ => None,
    };

    Ok(assemble(segments, toc, tod))
}

/// Returns a copy of `phases` whose segment durations and fuel burns follow
/// the solved per-leg ground speeds, while keeping the vertical geometry
/// (TOC/TOD positions and altitudes) unchanged.
///
/// The profile geometry is still the deterministic no-wind climb/descent
/// model; this pass only makes the temporal/fuel totals agree with the
/// wind-corrected PLOG timing. Missing leg winds fall back to the original
/// phase time share for that overlap.
pub fn wind_adjusted_phases(
    route: &[RouteWaypoint],
    phases: &PhasePlan,
    winds: &[LegWind],
) -> PhasePlan {
    if phases.segments.is_empty() || route.len() < 2 {
        return phases.clone();
    }

    let mut boundaries = Vec::with_capacity(route.len());
    boundaries.push(0.0_f64);
    let mut accumulated = 0.0_f64;
    for leg in route::legs(route) {
        accumulated += leg.geometry().distance.0;
        boundaries.push(accumulated);
    }

    let mut adjusted = phases.clone();
    for segment in &mut adjusted.segments {
        let original_minutes = segment.duration.0.max(0.0);
        let original_flow = if original_minutes > 0.0 {
            segment.fuel.0 / Minutes(original_minutes).as_hours()
        } else {
            0.0
        };
        let span = segment.end_along_track.0 - segment.start_along_track.0;
        if span <= DIST_EPS {
            continue;
        }

        let mut minutes = 0.0_f64;
        for leg in route::legs(route) {
            let leg_start = boundaries[leg.index];
            let leg_end = boundaries[leg.index + 1];
            let overlap = (segment.end_along_track.0.min(leg_end)
                - segment.start_along_track.0.max(leg_start))
            .max(0.0);
            if overlap <= DIST_EPS {
                continue;
            }
            let wind = winds.iter().find(|wind| wind.leg_index == leg.index);
            if let Some(wind) = wind
                && wind.triangle.ground_speed.0 > 0.0
            {
                minutes += overlap / METERS_PER_NAUTICAL_MILE / wind.triangle.ground_speed.0 * 60.0;
            } else {
                minutes += original_minutes * overlap / span;
            }
        }

        segment.duration = Minutes(minutes);
        segment.fuel = Liters(original_flow * segment.duration.as_hours());
    }

    assemble(adjusted.segments, adjusted.toc, adjusted.tod)
}

/// One cruise/climb/descent model resolved and validated.
struct VerticalRates {
    /// Meters of altitude per meter along track (`rate / ias`).
    gradient: f64,
    rate_mps: f64,
    ias: Knots,
    fuel_flow: LitersPerHour,
}

fn climb_model(performance: &crate::aircraft::Performance) -> Result<VerticalRates, PerfError> {
    vertical_model(
        "climb",
        performance.climb.ias,
        performance.climb.rate,
        performance.climb.fuel_flow,
    )
}

fn descent_model(performance: &crate::aircraft::Performance) -> Result<VerticalRates, PerfError> {
    vertical_model(
        "descent",
        performance.descent.ias,
        performance.descent.rate,
        performance.descent.fuel_flow,
    )
}

fn vertical_model(
    kind: &str,
    ias: Knots,
    rate: FeetPerMinute,
    fuel_flow: LitersPerHour,
) -> Result<VerticalRates, PerfError> {
    if !rate.0.is_finite() || rate.0 <= 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "{kind} rate must be > 0 ft/min (got {})",
            rate.0
        )));
    }
    if !ias.0.is_finite() || ias.0 <= 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "{kind} speed must be > 0 kt (got {})",
            ias.0
        )));
    }
    if !fuel_flow.0.is_finite() || fuel_flow.0 < 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "{kind} fuel flow must be ≥ 0 L/h (got {})",
            fuel_flow.0
        )));
    }
    let rate_mps = rate.as_meters_per_second();
    Ok(VerticalRates {
        gradient: rate_mps / ias.as_meters_per_second(),
        rate_mps,
        ias,
        fuel_flow,
    })
}

/// Resolves and validates the cruise power setting (`None` = the profile's
/// first; also the compute façade's fail-fast validation and TAS source).
pub(crate) fn resolve_cruise<'a>(
    aircraft: &'a AircraftProfile,
    power_setting: Option<&str>,
) -> Result<&'a PowerSetting, PerfError> {
    let settings = &aircraft.performance.cruise_settings;
    if settings.is_empty() {
        return Err(PerfError::NoCruiseSetting);
    }
    let setting = match power_setting {
        Some(name) => settings
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| PerfError::UnknownPowerSetting(name.to_owned()))?,
        None => &settings[0],
    };
    if !setting.tas.0.is_finite() || setting.tas.0 <= 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "cruise setting {:?} must have TAS > 0 kt (got {})",
            setting.name, setting.tas.0
        )));
    }
    if !setting.fuel_flow.0.is_finite() || setting.fuel_flow.0 < 0.0 {
        return Err(PerfError::InvalidProfile(format!(
            "cruise setting {:?} must have fuel flow ≥ 0 L/h (got {})",
            setting.name, setting.fuel_flow.0
        )));
    }
    Ok(setting)
}

/// A climb/descent segment; duration from the height change and the
/// modeled rate (consistent with `Δx / ias` by construction, and the
/// honest number for the direct steeper-than-modeled fallback).
fn vertical_segment(
    kind: PhaseKind,
    x0: f64,
    x1: f64,
    h0: f64,
    h1: f64,
    model: &VerticalRates,
) -> PhaseSegment {
    let duration = Minutes((h1 - h0).abs() / model.rate_mps / 60.0);
    PhaseSegment {
        kind,
        start_along_track: Meters(x0),
        end_along_track: Meters(x1),
        start_altitude: MetersAmsl(h0),
        end_altitude: MetersAmsl(h1),
        tas: model.ias,
        duration,
        fuel: Liters(model.fuel_flow.0 * duration.as_hours()),
    }
}

fn empty_plan() -> PhasePlan {
    PhasePlan {
        segments: Vec::new(),
        toc: None,
        tod: None,
        total_duration: Minutes(0.0),
        total_fuel: Liters(0.0),
    }
}

/// The degenerate single-segment plan for routes too short to contain the
/// modeled transition (see module docs).
fn direct_plan(
    total: f64,
    h_dep: f64,
    h_dest: f64,
    model: &VerticalRates,
    kind: PhaseKind,
) -> PhasePlan {
    let segment = vertical_segment(kind, 0.0, total, h_dep, h_dest, model);
    assemble(vec![segment], None, None)
}

fn assemble(
    segments: Vec<PhaseSegment>,
    toc: Option<ProfileMarker>,
    tod: Option<ProfileMarker>,
) -> PhasePlan {
    let total_duration = Minutes(segments.iter().map(|s| s.duration.0).sum());
    let total_fuel = Liters(segments.iter().map(|s| s.fuel.0).sum());
    PhasePlan {
        segments,
        toc,
        tod,
        total_duration,
        total_fuel,
    }
}

/// Appends a point, skipping exact-ish duplicates of the last one.
fn push_point(points: &mut Vec<(f64, f64)>, x: f64, h: f64) {
    if let Some(&(last_x, last_h)) = points.last()
        && (x - last_x).abs() < 1e-9
        && (h - last_h).abs() < 1e-9
    {
        return;
    }
    points.push((x, h));
}

/// Clips the profile polyline against a straight line: keeps
/// `min(profile, line)` when `keep_lower`, else `max(profile, line)`,
/// inserting the crossing vertices.
fn clip_with_line(
    points: &[(f64, f64)],
    line: impl Fn(f64) -> f64,
    keep_lower: bool,
) -> Vec<(f64, f64)> {
    let sign = if keep_lower { 1.0 } else { -1.0 };
    let excess = |x: f64, h: f64| sign * (h - line(x));

    let mut out: Vec<(f64, f64)> = Vec::with_capacity(points.len() + 2);
    for (i, &(x, h)) in points.iter().enumerate() {
        let d = excess(x, h);
        let clipped = if d > 0.0 { line(x) } else { h };
        push_point(&mut out, x, clipped);
        if let Some(&(x2, h2)) = points.get(i + 1) {
            let d2 = excess(x2, h2);
            if (d > 0.0 && d2 < 0.0) || (d < 0.0 && d2 > 0.0) {
                let t = d / (d - d2);
                let cx = x + t * (x2 - x);
                push_point(&mut out, cx, line(cx));
            }
        }
    }
    out
}

/// Removes interior vertices of collinear runs (e.g. boundary points of
/// legs sharing one cruise altitude, or successive points on the final
/// descent line).
fn merge_collinear(points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for point in points {
        while out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let slope_ab = (b.1 - a.1) / (b.0 - a.0).max(1e-12);
            let slope_bp = (point.1 - b.1) / (point.0 - b.0).max(1e-12);
            if (slope_ab - slope_bp).abs() < 1e-9 {
                out.pop();
            } else {
                break;
            }
        }
        push_point(&mut out, point.0, point.1);
    }
    out
}

/// Marker (TOC/TOD) at an along-track position: locates the containing leg
/// and interpolates on its great circle.
fn marker_at(
    route: &[RouteWaypoint],
    boundaries: &[f64],
    along_track: f64,
    altitude: MetersAmsl,
) -> ProfileMarker {
    ProfileMarker {
        along_track: Meters(along_track),
        position: position_at(route, boundaries, along_track),
        altitude,
    }
}

fn position_at(route: &[RouteWaypoint], boundaries: &[f64], along_track: f64) -> LatLon {
    let mut result = route[route.len() - 1].position();
    for leg in route::legs(route) {
        let start = boundaries[leg.index];
        let end = boundaries[leg.index + 1];
        if along_track <= end + DIST_EPS || leg.index == route.len() - 2 {
            let length = end - start;
            let fraction = if length > DIST_EPS {
                (along_track - start) / length
            } else {
                0.0
            };
            result = route::intermediate_point(leg.from.position(), leg.to.position(), fraction);
            break;
        }
    }
    result
}

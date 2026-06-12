//! Per-leg wind resolution over a route.

use chrono::{DateTime, Duration, Utc};
use strata_data::domain::MetersAmsl;

use crate::flight::{PlannedAltitude, RouteWaypoint};
use crate::perf::{isa_temperature, planned_altitude_amsl};
use crate::route;
use crate::sources::{Provenance, WindsAloft, WindsAloftSampler};
use crate::units::{DegreesTrue, Knots, METERS_PER_NAUTICAL_MILE};

use super::{LegWind, LegWindOrigin, WindError, solve_wind_triangle};

/// Resolves the wind for every leg and solves its triangle.
///
/// Per leg, in order:
///
/// 1. **Manual override wins:** a [`leg_wind`](RouteWaypoint::leg_wind) on
///    the leg's *from* waypoint is used as-is, with an ISA-estimated
///    temperature at the leg altitude ([`LegWindOrigin::Manual`]).
/// 2. Otherwise the sampler is queried at the **leg midpoint**, the leg's
///    planned altitude (leg override, else `cruise_altitude`) and the
///    estimated **mid-leg passage time**: `departure_time` + the ETE of all
///    previous legs (from their solved ground speeds) + half this leg's ETE
///    estimated at `tas` (the wind isn't known yet — a second-order
///    approximation, documented).
/// 3. **Calm-ISA fallback:** where sampling is impossible (no departure
///    time, no planned altitude) or the model has no data (`Ok(None)`), a
///    calm 0 kt wind with ISA temperature is used and the leg still solves
///    (GS = TAS). Reported as [`LegWindOrigin::IsaFallback`] with
///    [`Provenance::Isa`] temperature, so every surface can label the
///    assumption honestly.
///
/// Errors propagate from the sampler ([`WindError::Source`]) and from
/// unsolvable triangles ([`WindError::Unsolvable`]).
pub fn leg_winds(
    route: &[RouteWaypoint],
    cruise_altitude: Option<PlannedAltitude>,
    departure_time: Option<DateTime<Utc>>,
    tas: Knots,
    sampler: &dyn WindsAloftSampler,
) -> Result<Vec<LegWind>, WindError> {
    let mut result = Vec::with_capacity(route.len().saturating_sub(1));
    let mut elapsed_minutes = 0.0_f64;

    for leg in route::legs(route) {
        let geometry = leg.geometry();
        let altitude = leg
            .from
            .leg_altitude
            .or(cruise_altitude)
            .map(planned_altitude_amsl);

        let (wind, origin) = match leg.from.leg_wind {
            Some(manual) => {
                let temperature = isa_temperature(altitude.unwrap_or(MetersAmsl(0.0)));
                (
                    WindsAloft {
                        direction: DegreesTrue::new(manual.direction.0),
                        speed: manual.speed,
                        temperature,
                        temperature_provenance: Provenance::Isa,
                    },
                    LegWindOrigin::Manual,
                )
            }
            None => {
                let sampled = match (altitude, departure_time) {
                    (Some(altitude), Some(departure)) => {
                        // Mid-leg passage estimate; the current leg's half
                        // ETE uses TAS since its wind isn't known yet.
                        let half_leg_minutes = half_leg_ete_minutes(geometry.distance.0, tas);
                        let valid_time =
                            departure + minutes_duration(elapsed_minutes + half_leg_minutes);
                        sampler.sample(geometry.midpoint, altitude, valid_time)?
                    }
                    _ => None,
                };
                match sampled {
                    Some(wind) => (wind, LegWindOrigin::Sampled),
                    None => {
                        tracing::debug!(
                            leg = leg.index,
                            "no winds-aloft data for leg; using calm-ISA fallback"
                        );
                        let temperature = isa_temperature(altitude.unwrap_or(MetersAmsl(0.0)));
                        (
                            WindsAloft {
                                direction: DegreesTrue::new(0.0),
                                speed: Knots(0.0),
                                temperature,
                                temperature_provenance: Provenance::Isa,
                            },
                            LegWindOrigin::IsaFallback,
                        )
                    }
                }
            }
        };

        let triangle =
            solve_wind_triangle(geometry.initial_true_track, tas, wind.direction, wind.speed)?;

        // Accumulate this leg's ETE from the solved ground speed for the
        // next leg's passage-time estimate.
        let distance_nm = geometry.distance.0 / METERS_PER_NAUTICAL_MILE;
        elapsed_minutes += distance_nm / triangle.ground_speed.0 * 60.0;

        result.push(LegWind {
            leg_index: leg.index,
            wind,
            origin,
            triangle,
        });
    }

    Ok(result)
}

/// Half the leg's ETE in minutes, estimated at `tas` (zero for degenerate
/// TAS — the triangle solver rejects that case afterwards anyway).
fn half_leg_ete_minutes(distance_meters: f64, tas: Knots) -> f64 {
    if tas.0 > 0.0 {
        distance_meters / METERS_PER_NAUTICAL_MILE / tas.0 * 60.0 / 2.0
    } else {
        0.0
    }
}

/// Fractional minutes → chrono duration (millisecond resolution; plenty
/// for forecast-hour sampling).
fn minutes_duration(minutes: f64) -> Duration {
    Duration::milliseconds((minutes * 60_000.0).round() as i64)
}

//! Fuel ladder and endurance (plan §3 `fuel/`): policy → ladder against
//! loaded fuel, endurance/range readouts.
//!
//! The ladder (design §3.4 "Fuel"):
//! taxi + trip + contingency + alternate + final reserve + extra =
//! **minimum required**, judged against the loaded fuel of the W&B scenario.
//! Policy knobs (times, percentages, extra) are defensively clamped at zero
//! so malformed input can never *reduce* the minimum.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
mod tests;

use crate::aircraft::AircraftProfile;
use crate::flight::{Contingency, FuelPolicy};
use crate::perf::{PhaseKind, PhasePlan};
use crate::units::{Liters, LitersPerHour, Minutes};

/// The computed fuel ladder (design §3.4 "Fuel"): every rung in liters,
/// plus the derived minimum and margin against the loaded fuel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FuelLadder {
    /// Policy taxi time × profile taxi flow.
    pub taxi: Liters,
    /// Trip fuel from the phase plan.
    pub trip: Liters,
    pub contingency: Liters,
    /// Fuel for the alternate leg; zero without an alternate.
    pub alternate: Liters,
    /// Policy final-reserve time × cruise flow.
    pub final_reserve: Liters,
    pub extra: Liters,
    /// taxi + trip + contingency + alternate + final reserve + extra.
    pub minimum_required: Liters,
    /// Fuel on board from the loading scenario.
    pub loaded: Liters,
    /// `loaded − minimum_required` (negative = under-fueled).
    pub margin: Liters,
}

/// Errors from fuel computation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FuelError {
    #[error("aircraft fuel data missing: {0}")]
    MissingData(&'static str),
    #[error("aircraft profile has no power setting named {0:?}")]
    UnknownPowerSetting(String),
}

/// Builds the fuel ladder from policy, profile, the trip phase plan and an
/// optional alternate-leg phase plan, judged against `loaded` fuel.
///
/// Rungs:
/// - **taxi** — policy taxi minutes × the profile's taxi fuel flow.
/// - **trip** — [`PhasePlan::total_fuel`] of `trip`.
/// - **contingency** — per [`Contingency`]: a percentage of trip fuel or a
///   fixed amount.
/// - **alternate** — [`PhasePlan::total_fuel`] of `alternate`, zero if none.
/// - **final reserve** — policy minutes × cruise flow. The flow is derived
///   from the trip plan's cruise segments (fuel ÷ duration); when the plan
///   never cruises it falls back to the selected profile cruise setting
///   (`None` = first). No resolvable positive flow with a positive reserve
///   time is [`FuelError::MissingData`].
/// - **extra** — straight from the policy.
pub fn compute_fuel_ladder(
    policy: &FuelPolicy,
    aircraft: &AircraftProfile,
    power_setting: Option<&str>,
    trip: &PhasePlan,
    alternate: Option<&PhasePlan>,
    loaded: Liters,
) -> Result<FuelLadder, FuelError> {
    let taxi =
        Liters(Minutes(policy.taxi.0.max(0.0)).as_hours() * aircraft.performance.taxi_fuel_flow.0);
    let trip_fuel = trip.total_fuel;
    let contingency = Liters(match policy.contingency {
        Contingency::PercentOfTrip(percent) => trip_fuel.0 * percent.max(0.0) / 100.0,
        Contingency::Fixed(liters) => liters.0.max(0.0),
    });
    let alternate_fuel = alternate.map_or(Liters(0.0), |plan| plan.total_fuel);

    let reserve_minutes = policy.final_reserve.0.max(0.0);
    let final_reserve = if reserve_minutes > 0.0 {
        let flow = final_reserve_flow(aircraft, power_setting, trip).ok_or(
            FuelError::MissingData("cruise fuel flow for the final reserve"),
        )?;
        Liters(Minutes(reserve_minutes).as_hours() * flow.0)
    } else {
        Liters(0.0)
    };

    let extra = Liters(policy.extra.0.max(0.0));
    let minimum_required =
        Liters(taxi.0 + trip_fuel.0 + contingency.0 + alternate_fuel.0 + final_reserve.0 + extra.0);

    Ok(FuelLadder {
        taxi,
        trip: trip_fuel,
        contingency,
        alternate: alternate_fuel,
        final_reserve,
        extra,
        minimum_required,
        loaded,
        margin: Liters(loaded.0 - minimum_required.0),
    })
}

/// The cruise fuel flow backing the final reserve: derived from the trip
/// plan's cruise segments when it has any (Σ fuel ÷ Σ duration), otherwise
/// the selected profile cruise setting. `None` when neither yields a
/// positive flow or the selected setting does not exist.
fn final_reserve_flow(
    aircraft: &AircraftProfile,
    power_setting: Option<&str>,
    trip: &PhasePlan,
) -> Option<LitersPerHour> {
    let (fuel, minutes) = trip
        .segments
        .iter()
        .filter(|segment| segment.kind == PhaseKind::Cruise)
        .fold((0.0, 0.0), |(fuel, minutes), segment| {
            (fuel + segment.fuel.0, minutes + segment.duration.0)
        });
    if minutes > 0.0 {
        let flow = fuel / Minutes(minutes).as_hours();
        if flow > 0.0 {
            return Some(LitersPerHour(flow));
        }
    }
    let setting = match power_setting {
        Some(name) => aircraft
            .performance
            .cruise_settings
            .iter()
            .find(|setting| setting.name == name)?,
        None => aircraft.performance.cruise_settings.first()?,
    };
    (setting.fuel_flow.0 > 0.0).then_some(setting.fuel_flow)
}

/// Endurance of `fuel` at the named cruise power setting (`None` = the
/// profile's first setting): fuel ÷ cruise flow.
///
/// Negative fuel clamps to zero endurance. A missing setting name is
/// [`FuelError::UnknownPowerSetting`]; an empty cruise table or a
/// non-positive flow (placeholder profile) is [`FuelError::MissingData`] —
/// never a division by zero.
pub fn endurance(
    aircraft: &AircraftProfile,
    power_setting: Option<&str>,
    fuel: Liters,
) -> Result<Minutes, FuelError> {
    let settings = &aircraft.performance.cruise_settings;
    let setting = match power_setting {
        Some(name) => settings
            .iter()
            .find(|setting| setting.name == name)
            .ok_or_else(|| FuelError::UnknownPowerSetting(name.to_owned()))?,
        None => settings
            .first()
            .ok_or(FuelError::MissingData("cruise power settings"))?,
    };
    if setting.fuel_flow.0 <= 0.0 {
        return Err(FuelError::MissingData("positive cruise fuel flow"));
    }
    Ok(Minutes::from_hours(fuel.0.max(0.0) / setting.fuel_flow.0))
}

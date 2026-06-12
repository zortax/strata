//! Fuel policy: how the minimum-required fuel ladder is built.
//!
//! Defaults follow the **EASA Part-NCO day-VFR template** (30 min final
//! reserve, 5 % contingency). They are templates, clearly labelled in the
//! UI — not regulatory guidance; the pilot verifies current regulation.

use serde::{Deserialize, Serialize};

use crate::units::{Liters, Minutes};

/// Contingency fuel rule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contingency {
    /// Percentage of trip fuel (e.g. `5.0` = 5 %).
    PercentOfTrip(f64),
    /// Fixed amount.
    Fixed(Liters),
}

/// The fuel-ladder policy (design §3.4 "Fuel"): taxi → trip → contingency →
/// alternate → final reserve → extra. Trip and alternate fuel come from the
/// performance phases; this struct holds the policy knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FuelPolicy {
    /// Taxi/run-up allowance at the aircraft's taxi fuel flow.
    /// Template default: 10 min.
    pub taxi: Minutes,
    /// Template default: 5 % of trip fuel.
    pub contingency: Contingency,
    /// Final reserve at cruise fuel flow. EASA Part-NCO day-VFR template:
    /// 30 min.
    pub final_reserve: Minutes,
    /// Discretionary extra fuel on top of the minimum.
    pub extra: Liters,
}

impl Default for FuelPolicy {
    fn default() -> Self {
        Self {
            taxi: Minutes(10.0),
            contingency: Contingency::PercentOfTrip(5.0),
            final_reserve: Minutes(30.0),
            extra: Liters(0.0),
        }
    }
}

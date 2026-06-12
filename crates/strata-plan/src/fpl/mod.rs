//! ICAO flight plan (FPL) generation, items 7–19, with local format-level
//! validation (plan §3 `fpl/`). Output is the plain-text message for
//! manual filing — no online filing in this milestone.
//!
//! [`generate`] builds the parenthesized ATS message line by line; every
//! produced item is checked by [`validate_item`] (which also serves as a
//! standalone pre-flight check for user-edited fields). Pilot-side data
//! (item 19 persons/colour/PIC) comes from [`PilotInfo`], app settings
//! material that is not part of the flight document.

mod format;
mod items;
#[cfg(test)]
mod tests;
mod validate;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::aircraft::AircraftProfile;
use crate::compute::ComputedFlight;
use crate::flight::FlightDoc;

pub use validate::validate_item;

/// Errors from FPL generation/validation. `item` is the ICAO item number
/// (7–19).
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum FplError {
    #[error("FPL item {item}: {reason}")]
    InvalidItem { item: u8, reason: String },
    #[error("FPL item {item} requires {what}")]
    MissingData { item: u8, what: &'static str },
}

/// Pilot/operator data for item 19 — lives in the app settings (design
/// §4 "pilot data from settings"), not in the flight document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PilotInfo {
    /// Item 19 `C/` — pilot in command. Required.
    pub pilot_in_command: String,
    /// Item 19 `P/` — persons on board; `None` files as `TBN`.
    pub persons_on_board: Option<u16>,
    /// Item 19 `A/` — aircraft colour and markings; omitted when `None`.
    pub aircraft_color: Option<String>,
}

/// Generates the FPL message text (items 7–19) from the document, the
/// aircraft's identity/equipment, the computed EET and the pilot data.
///
/// Field conventions (documented per builder in the `items` submodule):
/// registration without hyphens (7), `V` + general aviation (8), wake
/// category from MTOW (9), profile equipment strings (10), departure/
/// destination as ICAO indicators or `ZZZZ` + item 18 coordinates
/// (13/16), `N0xxx` TAS + `A0xx`/`F0xx`/`VFR` level + DCT-joined route
/// (15), EET from the computed nav log (16), `DOF/` (18), endurance from
/// the fuel plan + persons + PIC (19).
pub fn generate(
    doc: &FlightDoc,
    aircraft: &AircraftProfile,
    computed: &ComputedFlight,
    pilot: &PilotInfo,
) -> Result<String, FplError> {
    let item7 = items::item7(aircraft)?;
    let item8 = items::item8(doc);
    let item9 = items::item9(aircraft)?;
    let item10 = items::item10(aircraft)?;
    let (item13, dep) = items::item13(doc)?;
    let item15 = items::item15(doc, aircraft)?;
    let (item16, dest, altn) = items::item16(doc, computed)?;
    let item18 = items::item18(doc, dep, dest, altn);
    let item19 = items::item19(doc, aircraft, pilot)?;

    for (item, value) in [
        (7u8, item7.as_str()),
        (8, item8.as_str()),
        (9, item9.as_str()),
        (10, item10.as_str()),
        (13, item13.as_str()),
        (15, item15.as_str()),
        (16, item16.as_str()),
        (18, item18.as_str()),
        (19, item19.as_str()),
    ] {
        validate_item(item, value)?;
    }

    Ok(format!(
        "(FPL-{item7}-{item8}\n-{item9}-{item10}\n-{item13}\n-{item15}\n-{item16}\n-{item18}\n-{item19})"
    ))
}

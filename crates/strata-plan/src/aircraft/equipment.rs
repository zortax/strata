//! ICAO FPL item 10 equipment defaults carried by the aircraft profile.

use serde::{Deserialize, Serialize};

/// Equipment strings for FPL item 10 (`10a/10b`), e.g. `"SDFGLO"/"S"`.
/// Free-form here; format validation lives in [`crate::fpl`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FplEquipment {
    /// Item 10a — COM/NAV/approach equipment. Template default `"V"`
    /// (VHF RTF only); edit per aircraft.
    pub com_nav_approach: String,
    /// Item 10b — surveillance equipment. Template default `"S"` (Mode S
    /// elementary, the common German GA fit); edit per aircraft.
    pub surveillance: String,
}

impl Default for FplEquipment {
    fn default() -> Self {
        Self {
            com_nav_approach: "V".to_owned(),
            surveillance: "S".to_owned(),
        }
    }
}

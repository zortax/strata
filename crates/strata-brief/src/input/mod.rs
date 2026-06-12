//! The briefing input contract: a tree of plain, serializable data.
//!
//! Deliberately **decoupled from `strata-plan`** — no planner types appear
//! here, only strings, numbers and timestamps. The app converts its
//! `ComputedFlight`/snapshot state into this shape; any other frontend can
//! do the same, which keeps the crate reusable (plan §1).
//!
//! Conventions:
//!
//! - Quantities are `f64` in the unit named by the field suffix
//!   (`_nm`, `_minutes`, `_liters`, `_kg`, `_m`, `_kt`, `_deg`, `_c`).
//! - Altitudes are **pre-formatted strings** (`"5500 ft AMSL"`, `"FL 75"`):
//!   altitudes carry their datum (CLAUDE.md) and the datum-aware formatting
//!   belongs to the caller, not a PDF layout crate.
//! - Timestamps are `chrono::DateTime<Utc>`; the template formats them.
//!   [`BriefingInput::generated_at`] is provided by the caller so rendering
//!   is deterministic — the crate never reads a clock.
//! - Sections are `Option`: `None` renders the section with an honest
//!   "not available" line, never silently absent.

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Everything the briefing document renders. See the module docs for the
/// unit and formatting conventions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BriefingInput {
    pub flight: FlightSummary,
    /// Generation timestamp shown on the cover/footer and stamped into the
    /// PDF metadata. Caller-provided: same input ⇒ same PDF.
    pub generated_at: DateTime<Utc>,
    pub navlog: Option<NavLogSection>,
    pub fuel: Option<FuelSection>,
    pub weight_balance: Option<WbSection>,
    pub weather: Option<WeatherSection>,
    pub notams: Option<NotamSection>,
}

/// Cover-block facts about the flight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightSummary {
    /// Flight name, e.g. `"Bavaria Test Hop"`.
    pub name: String,
    /// Ordered waypoint labels, departure first, destination last.
    pub route: Vec<String>,
    /// Alternate aerodrome label, if planned.
    pub alternate: Option<String>,
    /// Aircraft type designator, e.g. `"C172"`.
    pub aircraft_type: Option<String>,
    /// Aircraft registration, e.g. `"D-EABC"`.
    pub registration: Option<String>,
    pub callsign: Option<String>,
    /// Planned departure time (UTC).
    pub departure_time: Option<DateTime<Utc>>,
    /// Pre-formatted, datum-carrying cruise altitude (`"5500 ft AMSL"`).
    pub cruise_altitude: Option<String>,
    pub total_distance_nm: Option<f64>,
    pub total_ete_minutes: Option<f64>,
    pub total_fuel_liters: Option<f64>,
    /// Free-form remarks shown on the cover.
    pub remarks: Option<String>,
}

/// What a nav-log row marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavLogRowKind {
    Waypoint,
    TopOfClimb,
    TopOfDescent,
}

/// Leg wind (direction the wind blows *from*, true).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LegWind {
    pub direction_deg: f64,
    pub speed_kt: f64,
    pub temperature_c: Option<f64>,
}

/// One PLOG row: the checkpoint plus the values of the leg arriving at it.
/// The departure row carries `None` leg values by convention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavLogRow {
    pub kind: NavLogRowKind,
    /// Waypoint label, or `"TOC"`/`"TOD"`.
    pub label: String,
    /// Pre-formatted planned altitude at this point.
    pub altitude: Option<String>,
    pub true_track_deg: Option<f64>,
    pub magnetic_track_deg: Option<f64>,
    pub magnetic_heading_deg: Option<f64>,
    pub wind: Option<LegWind>,
    /// Wind correction angle, positive = right.
    pub wind_correction_angle_deg: Option<f64>,
    pub tas_kt: Option<f64>,
    pub ground_speed_kt: Option<f64>,
    pub distance_nm: Option<f64>,
    pub ete_minutes: Option<f64>,
    pub eta: Option<DateTime<Utc>>,
    pub leg_fuel_liters: Option<f64>,
    pub remaining_fuel_liters: Option<f64>,
    /// Pre-formatted frequency suggestion, e.g. `"Langen Info 128.950"`.
    pub frequency: Option<String>,
    pub notes: String,
}

/// The nav-log table plus its totals row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavLogSection {
    pub rows: Vec<NavLogRow>,
    pub total_distance_nm: f64,
    pub total_ete_minutes: f64,
    pub total_fuel_liters: f64,
}

/// The fuel ladder (taxi + trip + contingency + alternate + final reserve
/// + extra = minimum required) judged against the loaded fuel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuelSection {
    pub taxi_liters: f64,
    pub trip_liters: f64,
    pub contingency_liters: f64,
    pub alternate_liters: f64,
    pub final_reserve_liters: f64,
    pub extra_liters: f64,
    pub minimum_required_liters: f64,
    pub loaded_liters: f64,
    /// `loaded − minimum required` (negative = under-fueled).
    pub margin_liters: f64,
    pub endurance_minutes: Option<f64>,
    /// Policy provenance line, e.g. the "EASA Part-NCO template — verify
    /// current regulation" label the design doc requires.
    pub policy_note: Option<String>,
}

/// One row of the loading table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbLoadingRow {
    /// Station label (`"Empty aircraft"`, `"Pilot & front pax"`, `"Fuel"`…).
    pub station: String,
    pub mass_kg: f64,
    /// Arm aft of datum; the template derives the moment (mass × arm).
    pub arm_m: f64,
}

/// One computed mass-and-CG state (ramp / takeoff / zero fuel / landing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbStateRow {
    /// Display label, e.g. `"Takeoff"`.
    pub label: String,
    pub mass_kg: f64,
    pub cg_arm_m: f64,
    /// Inside the CG envelope *and* under the applicable mass limit.
    pub within_limits: bool,
}

/// A point in (arm, mass) space, for the envelope figure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CgPoint {
    pub arm_m: f64,
    pub mass_kg: f64,
}

/// Weight & balance: loading table, per-state CG verdicts and the envelope
/// data backing the figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbSection {
    pub loading: Vec<WbLoadingRow>,
    pub states: Vec<WbStateRow>,
    /// CG envelope polygon vertices (unclosed ring). Fewer than three
    /// points renders the numeric tables without the figure.
    pub envelope: Vec<CgPoint>,
    /// Fuel-burn CG track from takeoff to zero fuel.
    pub burn_track: Vec<CgPoint>,
    pub notes: Option<String>,
}

/// METAR/TAF for one aerodrome relevant to the route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AerodromeWeather {
    pub icao: String,
    pub name: Option<String>,
    /// Role on this flight: `"Departure"`, `"Destination"`, `"Alternate"`,
    /// `"En-route"`.
    pub role: String,
    /// Flight category badge (`"VFR"`, `"MVFR"`, `"IFR"`, `"LIFR"`).
    pub flight_category: Option<String>,
    pub metar_raw: Option<String>,
    /// Decoded METAR summary (multi-line plain text).
    pub metar_decoded: Option<String>,
    pub taf_raw: Option<String>,
    pub taf_decoded: Option<String>,
}

/// Winds aloft for one leg at its planned altitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindsAloftRow {
    /// Leg label, e.g. `"EDMA → WLD"`.
    pub leg: String,
    /// Pre-formatted planned altitude for the leg.
    pub altitude: String,
    pub direction_deg: f64,
    pub speed_kt: f64,
    pub temperature_c: Option<f64>,
}

/// The weather section: per-aerodrome METAR/TAF, per-leg winds aloft and
/// the freezing level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSection {
    /// When the weather snapshot was taken (the flight stores the snapshot
    /// it was planned with).
    pub snapshot_time: Option<DateTime<Utc>>,
    pub aerodromes: Vec<AerodromeWeather>,
    pub winds_aloft: Vec<WindsAloftRow>,
    /// Pre-formatted freezing level, e.g. `"9 800 ft AMSL"`.
    pub freezing_level: Option<String>,
    /// Provenance caveat for the winds-aloft data, rendered visibly with
    /// the table — e.g. `"ISA estimate — no forecast data"` when the plan
    /// fell back to the standard atmosphere. `None` hides the line.
    #[serde(default)]
    pub winds_source_note: Option<String>,
}

/// One NOTAM card, relevance-ordered by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotamCard {
    /// NOTAM id, e.g. `"A1234/26"`.
    pub id: String,
    /// Affected location(s), e.g. `"EDDF"` or `"EDGG (FIR)"`.
    pub location: String,
    /// Why this NOTAM made the briefing, e.g. `"Departure"` or
    /// `"Route corridor, 3 NM off track"`.
    pub relevance: Option<String>,
    /// Pre-formatted validity window.
    pub validity: String,
    /// Item D activity schedule, verbatim, when present.
    pub schedule: Option<String>,
    /// Pre-formatted vertical limits (items F/G), when present.
    pub limits: Option<String>,
    /// Decoded one-or-two-line summary.
    pub summary: String,
    /// The complete NOTAM as received.
    pub raw: String,
}

/// The NOTAM section. An empty `notams` list renders as "no relevant
/// NOTAMs" — distinct from the whole section being unavailable (`None` on
/// [`BriefingInput::notams`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotamSection {
    pub snapshot_time: Option<DateTime<Utc>>,
    pub notams: Vec<NotamCard>,
    /// Provenance caveat for the NOTAM data, rendered visibly at the top
    /// of the section — e.g. `"Built-in sample NOTAMs — not a real
    /// briefing"` while no live source is configured. `None` hides the
    /// line.
    #[serde(default)]
    pub source_note: Option<String>,
}

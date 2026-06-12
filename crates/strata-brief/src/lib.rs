//! Strata briefing PDF generation (plan §1 `strata-brief`).
//!
//! Renders a VFR pre-flight briefing document — cover, nav log, fuel plan,
//! weight & balance, weather, NOTAMs — to PDF using
//! [typst](https://typst.app) as a library.
//!
//! Self-contained and deterministic: the template and fonts are embedded,
//! the typst `World` answers no filesystem or network requests, and the
//! generation timestamp is part of the input. The input contract
//! ([`BriefingInput`]) is plain serializable data with **no planner types**
//! — the app converts, so the crate stays reusable by other frontends.
//!
//! ```no_run
//! # fn demo(input: &strata_brief::BriefingInput) -> Result<(), strata_brief::BriefError> {
//! let pdf: Vec<u8> = strata_brief::render_briefing(input)?;
//! # Ok(()) }
//! ```
//!
//! typst is a heavy dependency (large transitive graph, slow cold build) —
//! that is why this crate exists at all: only `strata-app` links it, and
//! only for the one export feature (plan §1 rationale).

#[cfg(test)]
pub(crate) mod fixtures;
mod input;
mod render;
mod world;

pub use input::{
    AerodromeWeather, BriefingInput, CgPoint, FlightSummary, FuelSection, LegWind, NavLogRow,
    NavLogRowKind, NavLogSection, NotamCard, NotamSection, WbLoadingRow, WbSection, WbStateRow,
    WeatherSection, WindsAloftRow,
};
pub use render::{BriefError, render_briefing};

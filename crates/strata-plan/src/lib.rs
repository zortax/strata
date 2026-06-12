//! UI-free VFR flight-planning core of Strata.
//!
//! Owns the flight document model ([`flight::FlightDoc`]), aircraft profiles
//! ([`aircraft::AircraftProfile`]), route geometry ([`route`]), and the pure
//! computation pipeline (corridor sampling, wind triangle, performance
//! phases, weight & balance, fuel, conflicts, nav log, ICAO FPL) behind the
//! single façade [`compute::compute()`].
//!
//! Storage-agnostic by design: terrain, obstacles, airspaces, winds aloft
//! and magnetic variation are consumed through the [`sources`] traits, which
//! the application implements over its store / gridded caches and tests
//! implement synthetically.
//!
//! Planning aid only — **not for navigation**. Units follow CLAUDE.md
//! discipline: WGS84 degrees + meters internally, datum-carrying vertical
//! types from `strata-data`; aviation-facing units (NM, kt, ft, L) are
//! explicit newtypes in [`units`], converted at the edges.

pub mod aircraft;
pub mod compute;
pub mod conflict;
pub mod corridor;
pub mod flight;
pub mod fpl;
pub mod fuel;
pub mod navlog;
pub mod notam_relevance;
pub mod perf;
pub mod profile;
pub mod route;
pub mod sources;
pub mod units;
pub mod versioned;
pub mod wb;
pub mod wind;

pub use aircraft::AircraftProfile;
pub use compute::{ComputeOutcome, ComputedFlight, NotComputable, compute};
pub use flight::FlightDoc;
pub use sources::{
    AirspaceSource, ElevationSource, MagvarSource, ObstacleSource, Provenance, Sources,
    WindsAloftSampler,
};

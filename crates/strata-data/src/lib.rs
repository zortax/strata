//! UI-free core of Strata: domain model, provider traits + implementations
//! (openAIP, aviationweather.gov, Copernicus DEM, Protomaps, DWD ICON-D2),
//! SQLite store, METAR/TAF decoders.
//!
//! Read-only aviation data for situational awareness — **not for
//! navigation**. WGS84 + meters internally; vertical limits always carry
//! their datum.

pub mod decode;
pub mod domain;
pub mod error;
pub mod paths;
pub mod providers;
pub mod store;

pub use error::Error;

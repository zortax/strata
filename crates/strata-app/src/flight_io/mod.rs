//! The flight/aircraft file library (plan §2.5).
//!
//! Flights are files — `<data_dir>/flights/<slug>.strata-flight` — and
//! aircraft profiles live next to them in `<data_dir>/aircraft/
//! <id>.strata-aircraft`. Both are pretty JSON through `strata-plan`'s
//! versioned, tolerant loaders. The library index is derived by scanning
//! the directory (no DB table at this scale); the recent list lives in the
//! app [`Config`](crate::config::Config) (`recent_flights`).
//!
//! All writes are atomic (temp file + fsync + rename) through the shared
//! [`crate::fsutil`] helper: process-unique temp names (concurrent writes
//! to one file cannot corrupt it) and write tickets (an older detached
//! snapshot cannot overwrite a newer one). Everything here is plain
//! blocking IO — callers run it on the background executor, capturing the
//! ticket with the snapshot on the UI thread.

pub mod aircraft;
pub mod flights;

// Consumed by the menu/library and aircraft-manager phases on top of the
// state API; not every entry point has its UI caller yet.
#[allow(unused_imports)]
pub use aircraft::{
    AIRCRAFT_EXTENSION, aircraft_dir, aircraft_path, allocate_aircraft_id, delete_aircraft,
    ensure_example_aircraft, list_aircraft, load_aircraft, save_aircraft, save_aircraft_ordered,
};
#[allow(unused_imports)]
pub use flights::{
    FLIGHT_EXTENSION, FlightSummary, allocate_flight_path, dedupe_flight_path, flights_dir,
    list_flights, load_flight, save_flight, save_flight_ordered,
};

/// Lowercase filesystem slug for a display name: `[a-z0-9]` kept, every
/// other run collapsed to one `-`, trimmed; empty input → `"flight"`.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "flight".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The write_atomic tests live with the shared implementation in
    // `crate::fsutil` now.
    #[test]
    fn slug_collapses_and_lowercases() {
        assert_eq!(slug("EDFE → EDQN"), "edfe-edqn");
        assert_eq!(slug("  Rhön trip #2  "), "rh-n-trip-2");
        assert_eq!(slug("Flight"), "flight");
        assert_eq!(slug(""), "flight");
        assert_eq!(slug("→→→"), "flight");
        assert_eq!(slug("a"), "a");
    }
}

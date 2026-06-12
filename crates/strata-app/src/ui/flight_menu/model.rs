//! Pure logic behind the title-bar Flight menu, the flight strip and the
//! library dialog — everything here is plain data-in/data-out so the menu
//! behaviour is unit-testable without a window.

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use strata_plan::FlightDoc;
use strata_plan::aircraft::AircraftId;

use crate::flight_io;

/// What the Flight popup menu offers in the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuModel {
    /// The document section (Save / Save As… / Duplicate / Close Flight)
    /// is present — exactly while a flight is open (design §2).
    pub document_section: bool,
    /// Save is clickable: a flight is open *and* has unsaved changes.
    pub save_enabled: bool,
}

/// Menu state from the open flight's dirty flag (`None` = explorer mode).
pub fn menu_model(flight_dirty: Option<bool>) -> MenuModel {
    MenuModel {
        document_section: flight_dirty.is_some(),
        save_enabled: flight_dirty == Some(true),
    }
}

/// `now` rounded **up** to the next 10-minute mark; instants already on the
/// mark stay put. The "New Flight" departure-time default (design §2).
pub fn round_up_to_10_min(now: DateTime<Utc>) -> DateTime<Utc> {
    let step = TimeDelta::minutes(10);
    // `duration_trunc` only fails on far-out-of-range timestamps; passing
    // `now` through unrounded is the honest fallback there.
    let floored = now.duration_trunc(step).unwrap_or(now);
    if floored == now { now } else { floored + step }
}

/// Applies the "New Flight" template to a freshly created document:
/// departure now-rounded-up, the first library aircraft preselected.
/// Returns `true` (something changed) to fit the `edit_flight_doc` funnel.
pub fn apply_new_flight_defaults(
    doc: &mut FlightDoc,
    now: DateTime<Utc>,
    aircraft: Option<AircraftId>,
) -> bool {
    doc.departure_time = Some(round_up_to_10_min(now));
    doc.aircraft_id = aircraft;
    true
}

/// Display name for a duplicated flight: `"Trip (copy)"`; unnamed flights
/// stay unnamed (their strip/library label falls back to the route).
pub fn duplicate_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        String::new()
    } else {
        format!("{name} (copy)")
    }
}

/// Menu label for a recent-flights entry: the file stem (the slugged flight
/// name), falling back to the full path for degenerate paths.
pub fn recent_label(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Forces the flight-file extension onto a user-picked Save As… path (the
/// portal returns whatever the user typed).
pub fn with_flight_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    if path.extension().and_then(|e| e.to_str()) == Some(flight_io::FLIGHT_EXTENSION) {
        path
    } else {
        path.with_extension(flight_io::FLIGHT_EXTENSION)
    }
}

/// The title-bar flight strip: `"EDFE → EDQN · D-EABC · 09:30Z"` — route
/// summary (or name while the route is empty), aircraft registration and
/// departure time, dropping the segments that have no value yet.
pub fn strip_text(doc: &FlightDoc, registration: Option<&str>) -> String {
    let mut parts = vec![strip_title(doc)];
    if let Some(reg) = registration.map(str::trim).filter(|r| !r.is_empty()) {
        parts.push(reg.to_owned());
    }
    if let Some(time) = doc.departure_time {
        parts.push(format!("{}Z", time.format("%H:%M")));
    }
    parts.join(" · ")
}

/// Leading strip segment: the route once it has waypoints, else the flight
/// name, else the "New flight" placeholder.
fn strip_title(doc: &FlightDoc) -> String {
    if !doc.route.is_empty() {
        flight_io::flights::route_summary(doc)
    } else if !doc.name.trim().is_empty() {
        doc.name.trim().to_owned()
    } else {
        "New flight".to_owned()
    }
}

/// Library-row age label: relative while recent ("12 min ago"), the date
/// once older than a day. Future or missing mtimes read "just now" — the
/// scan only yields real files, so this is a clock-skew fallback.
pub fn modified_label(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(modified) = modified else {
        return "just now".to_owned();
    };
    let Ok(age) = now.duration_since(modified) else {
        return "just now".to_owned();
    };
    let secs = age.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 24 * 3600 {
        format!("{} h ago", secs / 3600)
    } else {
        let date: DateTime<Utc> = modified.into();
        date.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use chrono::TimeZone as _;
    use strata_data::domain::LatLon;
    use strata_plan::flight::{FreePoint, RoutePoint, RouteWaypoint};

    use super::*;

    fn waypoint(name: &str) -> RouteWaypoint {
        RouteWaypoint::new(RoutePoint::Free(FreePoint {
            name: Some(name.to_owned()),
            position: LatLon::new(50.0, 8.0).unwrap(),
        }))
    }

    #[test]
    fn menu_shows_the_document_section_only_with_an_open_flight() {
        assert_eq!(
            menu_model(None),
            MenuModel {
                document_section: false,
                save_enabled: false
            }
        );
        // Open and clean: section present, Save disabled.
        assert_eq!(
            menu_model(Some(false)),
            MenuModel {
                document_section: true,
                save_enabled: false
            }
        );
        assert_eq!(
            menu_model(Some(true)),
            MenuModel {
                document_section: true,
                save_enabled: true
            }
        );
    }

    #[test]
    fn departure_time_rounds_up_to_the_next_10_minutes() {
        let t = |h, m, s| Utc.with_ymd_and_hms(2026, 6, 11, h, m, s).unwrap();
        assert_eq!(round_up_to_10_min(t(9, 23, 0)), t(9, 30, 0));
        assert_eq!(round_up_to_10_min(t(9, 20, 1)), t(9, 30, 0));
        // Already on the mark: stays.
        assert_eq!(round_up_to_10_min(t(9, 30, 0)), t(9, 30, 0));
        // Hour rollover.
        assert_eq!(round_up_to_10_min(t(9, 55, 59)), t(10, 0, 0));
    }

    #[test]
    fn new_flight_defaults_fill_departure_and_aircraft() {
        let mut doc = FlightDoc::new("");
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 9, 23, 0).unwrap();
        let id = AircraftId::new("d-eabc").unwrap();
        assert!(apply_new_flight_defaults(&mut doc, now, Some(id.clone())));
        assert_eq!(
            doc.departure_time,
            Some(Utc.with_ymd_and_hms(2026, 6, 11, 9, 30, 0).unwrap())
        );
        assert_eq!(doc.aircraft_id, Some(id));

        // No aircraft in the library: the field simply stays empty.
        let mut doc = FlightDoc::new("");
        assert!(apply_new_flight_defaults(&mut doc, now, None));
        assert_eq!(doc.aircraft_id, None);
    }

    #[test]
    fn duplicate_names_append_copy_but_keep_unnamed_unnamed() {
        assert_eq!(duplicate_name("Rhön trip"), "Rhön trip (copy)");
        assert_eq!(duplicate_name(""), "");
        assert_eq!(duplicate_name("   "), "");
    }

    #[test]
    fn strip_text_reads_route_registration_and_time() {
        let mut doc = FlightDoc::new("");
        doc.route = vec![waypoint("EDFE"), waypoint("EDQN")];
        doc.departure_time = Some(Utc.with_ymd_and_hms(2026, 6, 11, 9, 30, 0).unwrap());
        assert_eq!(
            strip_text(&doc, Some("D-EABC")),
            "EDFE → EDQN · D-EABC · 09:30Z"
        );
    }

    #[test]
    fn strip_text_drops_missing_segments() {
        // Brand-new flight: placeholder only.
        let doc = FlightDoc::new("");
        assert_eq!(strip_text(&doc, None), "New flight");

        // Named but empty route: the name leads.
        let mut doc = FlightDoc::new("Rhön trip");
        doc.departure_time = Some(Utc.with_ymd_and_hms(2026, 6, 11, 7, 0, 0).unwrap());
        assert_eq!(strip_text(&doc, None), "Rhön trip · 07:00Z");

        // Blank registration is a missing registration.
        doc.route = vec![waypoint("EDFE")];
        assert_eq!(strip_text(&doc, Some("  ")), "EDFE · 07:00Z");
    }

    #[test]
    fn recent_labels_are_file_stems() {
        assert_eq!(
            recent_label(Path::new("/data/flights/edfe-edqn.strata-flight")),
            "edfe-edqn"
        );
        assert_eq!(recent_label(Path::new("trip.strata-flight")), "trip");
    }

    #[test]
    fn save_as_paths_get_the_flight_extension() {
        assert_eq!(
            with_flight_extension(PathBuf::from("/tmp/trip")),
            PathBuf::from("/tmp/trip.strata-flight")
        );
        assert_eq!(
            with_flight_extension(PathBuf::from("/tmp/trip.json")),
            PathBuf::from("/tmp/trip.strata-flight")
        );
        // Already correct: untouched.
        assert_eq!(
            with_flight_extension(PathBuf::from("/tmp/trip.strata-flight")),
            PathBuf::from("/tmp/trip.strata-flight")
        );
    }

    #[test]
    fn modified_labels_scale_with_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_780_000_000);
        let ago = |secs| Some(now - Duration::from_secs(secs));
        assert_eq!(modified_label(ago(5), now), "just now");
        assert_eq!(modified_label(ago(12 * 60), now), "12 min ago");
        assert_eq!(modified_label(ago(3 * 3600), now), "3 h ago");
        // Older than a day: the date. (1_780_000_000 − 3 days, UTC.)
        let label = modified_label(ago(3 * 24 * 3600), now);
        assert!(label.starts_with("2026-"), "{label}");
        // Missing or future mtime never panics.
        assert_eq!(modified_label(None, now), "just now");
        assert_eq!(
            modified_label(Some(now + Duration::from_secs(60)), now),
            "just now"
        );
    }
}

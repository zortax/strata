//! Flight files: `<data_dir>/flights/<slug>.strata-flight`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context as _;
use strata_plan::FlightDoc;

use super::slug;
use crate::fsutil::{WriteTicket, write_atomic_ordered};

/// File extension of flight documents (no leading dot).
pub const FLIGHT_EXTENSION: &str = "strata-flight";

/// The flights directory under the app data dir.
pub fn flights_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("flights")
}

/// One library entry from scanning the flights directory.
// Constructed for the library dialog (menu phase); `list_flights` is the
// only producer and lives right below.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct FlightSummary {
    pub path: PathBuf,
    /// Display name: the document's name, falling back to the route
    /// summary, falling back to the file stem.
    pub name: String,
    /// `"EDFE → EDQN"` from the route's first/last waypoint labels;
    /// `"(no route)"` while the route has fewer than two waypoints.
    pub route_summary: String,
    /// File modification time (the scan sort key, newest first).
    pub modified: Option<SystemTime>,
}

/// `"EDFE → EDQN"` from the first/last waypoint labels (the flight strip /
/// library line); single-waypoint routes show that one label.
pub fn route_summary(doc: &FlightDoc) -> String {
    match (doc.route.first(), doc.route.last()) {
        (Some(first), Some(last)) if doc.route.len() >= 2 => {
            format!("{} → {}", first.point.label(), last.point.label())
        }
        (Some(only), _) => only.point.label(),
        _ => "(no route)".to_owned(),
    }
}

/// Scans `dir` for `*.strata-flight` files, newest first. A missing
/// directory is an empty library; unreadable or unparsable files are
/// skipped with a warning (the library never fails as a whole).
// Consumed by the library dialog (menu phase).
#[allow(dead_code)]
pub fn list_flights(dir: &Path) -> Vec<FlightSummary> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(dir = %dir.display(), %err, "reading flights directory failed");
            return Vec::new();
        }
    };

    let mut flights = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(FLIGHT_EXTENSION) {
            continue;
        }
        let doc = match load_flight(&path) {
            Ok(doc) => doc,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "skipping unreadable flight file");
                continue;
            }
        };
        let route_summary = route_summary(&doc);
        let name = if !doc.name.trim().is_empty() {
            doc.name.clone()
        } else if doc.route.is_empty() {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Unnamed flight".to_owned())
        } else {
            route_summary.clone()
        };
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        flights.push(FlightSummary {
            path,
            name,
            route_summary,
            modified,
        });
    }
    // Newest first; files without an mtime sort last, ties break by path
    // for a deterministic library order.
    flights.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    flights
}

/// Loads one flight document (versioned + tolerant, see
/// [`FlightDoc::from_json_str`]).
pub fn load_flight(path: &Path) -> anyhow::Result<FlightDoc> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read flight file {}", path.display()))?;
    FlightDoc::from_json_str(&text).with_context(|| format!("parse flight file {}", path.display()))
}

/// Saves `doc` to `path` as pretty JSON, atomically (ordering ticket
/// captured at call time — for synchronous/sequential callers; the app's
/// async save path goes through [`save_flight_ordered`]).
#[allow(dead_code)]
pub fn save_flight(path: &Path, doc: &FlightDoc) -> anyhow::Result<()> {
    save_flight_ordered(path, doc, WriteTicket::next())
}

/// [`save_flight`] with a caller-captured [`WriteTicket`] — detached
/// background savers capture the ticket together with the document
/// snapshot on the UI thread, so an older snapshot can never land over a
/// newer one regardless of write completion order.
pub fn save_flight_ordered(
    path: &Path,
    doc: &FlightDoc,
    ticket: WriteTicket,
) -> anyhow::Result<()> {
    let text = doc.to_json_string().context("serialize flight document")?;
    write_atomic_ordered(path, &text, ticket).map(|_| ())
}

/// A free path for a new flight named `name`: `<dir>/<slug>.strata-flight`,
/// deduplicated with `-2`, `-3`, … while taken. Best effort against
/// concurrent writers — the save itself is atomic either way.
pub fn allocate_flight_path(dir: &Path, name: &str) -> PathBuf {
    let stem = slug(name);
    let candidate = dir.join(format!("{stem}.{FLIGHT_EXTENSION}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2.. {
        let candidate = dir.join(format!("{stem}-{n}.{FLIGHT_EXTENSION}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("the counter loop returns")
}

/// Overwrite guard for Save As…: returns `path` when it is free or is the
/// document's own file (`current` — saving onto yourself is a plain save);
/// otherwise the first free sibling with a `" (2)"`, `" (3)"`, … stem
/// suffix. The portal's own overwrite confirmation checks the name *as
/// typed* — the flight extension is forced on afterwards, so a collision
/// with an existing library file would otherwise overwrite silently.
pub fn dedupe_flight_path(path: PathBuf, current: Option<&Path>) -> PathBuf {
    if current == Some(path.as_path()) || !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "flight".to_owned());
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    for n in 2.. {
        let candidate = dir.join(format!("{stem} ({n}).{FLIGHT_EXTENSION}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("the counter loop returns")
}

#[cfg(test)]
mod tests {
    use strata_data::domain::LatLon;
    use strata_plan::flight::{FreePoint, RoutePoint, RouteWaypoint};

    use super::*;

    fn free(name: &str, lat: f64, lon: f64) -> RouteWaypoint {
        RouteWaypoint::new(RoutePoint::Free(FreePoint {
            name: Some(name.to_owned()),
            position: LatLon::new(lat, lon).unwrap(),
        }))
    }

    fn doc(name: &str, waypoints: &[RouteWaypoint]) -> FlightDoc {
        let mut doc = FlightDoc::new(name);
        doc.route = waypoints.to_vec();
        doc
    }

    #[test]
    fn route_summary_shapes() {
        assert_eq!(route_summary(&doc("x", &[])), "(no route)");
        assert_eq!(route_summary(&doc("x", &[free("EDFE", 50.0, 8.6)])), "EDFE");
        assert_eq!(
            route_summary(&doc(
                "x",
                &[free("EDFE", 50.0, 8.6), free("EDQN", 49.6, 11.2)]
            )),
            "EDFE → EDQN"
        );
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flights").join("trip.strata-flight");
        let original = doc("Test trip", &[free("A", 50.0, 8.0), free("B", 51.0, 9.0)]);
        save_flight(&path, &original).unwrap();
        let loaded = load_flight(&path).unwrap();
        assert_eq!(loaded, original);

        // Pretty JSON with the version field, per the persistence spec.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"format_version\": 1"), "{text}");
        assert!(text.contains('\n'), "pretty-printed");
    }

    #[test]
    fn list_scans_newest_first_and_skips_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let flights = flights_dir(dir.path());

        // Missing directory: empty library, no error.
        assert!(list_flights(&flights).is_empty());

        let older = flights.join("older.strata-flight");
        save_flight(&older, &doc("Older", &[free("A", 50.0, 8.0)])).unwrap();
        // Distinct mtimes without sleeping in the test.
        let earlier = SystemTime::now() - std::time::Duration::from_secs(60);
        let file = fs::File::options().write(true).open(&older).unwrap();
        file.set_modified(earlier).unwrap();
        drop(file);

        let newer = flights.join("newer.strata-flight");
        save_flight(
            &newer,
            &doc("", &[free("EDFE", 50.0, 8.6), free("EDQN", 49.6, 11.2)]),
        )
        .unwrap();

        // Garbage and foreign files never break the scan.
        fs::write(flights.join("broken.strata-flight"), "not json").unwrap();
        fs::write(flights.join("notes.txt"), "ignored").unwrap();

        let list = list_flights(&flights);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, newer);
        // Unnamed flight: the route summary is the display name.
        assert_eq!(list[0].name, "EDFE → EDQN");
        assert_eq!(list[0].route_summary, "EDFE → EDQN");
        assert_eq!(list[1].name, "Older");
        assert!(list[1].modified.is_some());
    }

    #[test]
    fn dedupe_path_suffixes_collisions_but_keeps_own_and_free_paths() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("bavaria-test-hop.strata-flight");
        fs::write(&taken, "{}").unwrap();

        // A free path passes through untouched.
        let free = dir.path().join("new-trip.strata-flight");
        assert_eq!(dedupe_flight_path(free.clone(), None), free);

        // Saving onto the document's own file is a plain save, not a
        // collision.
        assert_eq!(
            dedupe_flight_path(taken.clone(), Some(&taken)),
            taken,
            "own path is never deduped"
        );

        // A collision with a *different* file gets the " (2)" suffix…
        let deduped = dedupe_flight_path(taken.clone(), None);
        assert_eq!(
            deduped,
            dir.path().join("bavaria-test-hop (2).strata-flight")
        );

        // …and the counter walks past every taken sibling.
        fs::write(&deduped, "{}").unwrap();
        assert_eq!(
            dedupe_flight_path(taken.clone(), Some(Path::new("/elsewhere/x.strata-flight"))),
            dir.path().join("bavaria-test-hop (3).strata-flight")
        );
    }

    #[test]
    fn allocate_path_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let first = allocate_flight_path(dir.path(), "EDFE → EDQN");
        assert_eq!(
            first,
            dir.path().join("edfe-edqn.strata-flight"),
            "slugged file name"
        );
        fs::write(&first, "{}").unwrap();
        let second = allocate_flight_path(dir.path(), "EDFE → EDQN");
        assert_eq!(second, dir.path().join("edfe-edqn-2.strata-flight"));
        fs::write(&second, "{}").unwrap();
        let third = allocate_flight_path(dir.path(), "EDFE → EDQN");
        assert_eq!(third, dir.path().join("edfe-edqn-3.strata-flight"));
    }
}

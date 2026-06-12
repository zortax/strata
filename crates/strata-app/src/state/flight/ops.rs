//! Pure flight-document operations behind the [`AppState`] mutation API.
//!
//! Every operation returns whether it changed the document — `false` means
//! the edit was a no-op (same value, out-of-range index) and the caller
//! emits no event, marks nothing dirty and schedules no compute. Route
//! operations preserve strata-plan's leg-plan invariants by running
//! [`route::normalize`] afterwards.
//!
//! [`AppState`]: crate::state::AppState

use chrono::{DateTime, Utc};
use strata_plan::aircraft::AircraftId;
use strata_plan::flight::{PlannedAltitude, RoutePoint};
use strata_plan::{FlightDoc, route};

/// Inserts `point` at `index` (0 = before the departure, `len` = append),
/// inheriting a split leg's plan; out-of-range is a warned no-op.
pub fn insert_waypoint(doc: &mut FlightDoc, index: usize, point: RoutePoint) -> bool {
    if let Err(err) = route::insert(&mut doc.route, index, point) {
        tracing::warn!(%err, "insert_waypoint rejected");
        return false;
    }
    route::normalize(&mut doc.route);
    true
}

/// Appends `point` to the end of the route.
pub fn append_waypoint(doc: &mut FlightDoc, point: RoutePoint) -> bool {
    let index = doc.route.len();
    insert_waypoint(doc, index, point)
}

/// Removes the waypoint at `index`; out-of-range is a warned no-op.
pub fn remove_waypoint(doc: &mut FlightDoc, index: usize) -> bool {
    if index >= doc.route.len() {
        tracing::warn!(index, len = doc.route.len(), "remove_waypoint out of range");
        return false;
    }
    doc.route.remove(index);
    route::normalize(&mut doc.route);
    true
}

/// Reorders the route: the waypoint at `from` moves to position `to`
/// (list drag-reorder semantics). Leg plans travel with the waypoint they
/// are stored on; the trailing waypoint's plan is cleared by normalize.
pub fn move_waypoint(doc: &mut FlightDoc, from: usize, to: usize) -> bool {
    let len = doc.route.len();
    if from >= len || to >= len {
        tracing::warn!(from, to, len, "move_waypoint out of range");
        return false;
    }
    if from == to {
        return false;
    }
    let waypoint = doc.route.remove(from);
    doc.route.insert(to, waypoint);
    route::normalize(&mut doc.route);
    true
}

/// Replaces the *point* of the waypoint at `index` — the map-drag /
/// re-snap operation. The leg plan (altitude/wind) stays with the waypoint.
pub fn replace_waypoint_point(doc: &mut FlightDoc, index: usize, point: RoutePoint) -> bool {
    let Some(waypoint) = doc.route.get_mut(index) else {
        tracing::warn!(
            index,
            len = doc.route.len(),
            "replace_waypoint out of range"
        );
        return false;
    };
    if waypoint.point == point {
        return false;
    }
    waypoint.point = point;
    route::normalize(&mut doc.route);
    true
}

/// Sets (or clears) the planned altitude of the leg *from* waypoint
/// `index`. The final waypoint has no outgoing leg → warned no-op.
pub fn set_leg_altitude(
    doc: &mut FlightDoc,
    index: usize,
    altitude: Option<PlannedAltitude>,
) -> bool {
    let last = doc.route.len().saturating_sub(1);
    let Some(waypoint) = doc.route.get_mut(index).filter(|_| index < last) else {
        tracing::warn!(
            index,
            len = doc.route.len(),
            "set_leg_altitude out of range"
        );
        return false;
    };
    if waypoint.leg_altitude == altitude {
        return false;
    }
    waypoint.leg_altitude = altitude;
    true
}

/// Sets (or clears) the flight's default cruise altitude.
pub fn set_cruise_altitude(doc: &mut FlightDoc, altitude: Option<PlannedAltitude>) -> bool {
    if doc.cruise_altitude == altitude {
        return false;
    }
    doc.cruise_altitude = altitude;
    true
}

/// Sets (or clears) the alternate. The document model holds a list, but
/// planning consumes the first alternate (design §3.4) — this replaces the
/// whole list.
pub fn set_alternate(doc: &mut FlightDoc, alternate: Option<RoutePoint>) -> bool {
    let next: Vec<RoutePoint> = alternate.into_iter().collect();
    if doc.alternates == next {
        return false;
    }
    doc.alternates = next;
    true
}

/// Selects the aircraft profile the flight is planned with.
pub fn set_aircraft(doc: &mut FlightDoc, id: Option<AircraftId>) -> bool {
    if doc.aircraft_id == id {
        return false;
    }
    doc.aircraft_id = id;
    true
}

/// Sets (or clears) the planned departure time (UTC).
pub fn set_departure_time(doc: &mut FlightDoc, time: Option<DateTime<Utc>>) -> bool {
    if doc.departure_time == time {
        return false;
    }
    doc.departure_time = time;
    true
}

/// Renames the flight.
pub fn set_name(doc: &mut FlightDoc, name: String) -> bool {
    if doc.name == name {
        return false;
    }
    doc.name = name;
    true
}

/// Sets the nav-log notes of the waypoint at `index` (the drawer's Notes
/// column); out-of-range is a warned no-op. No normalize — notes never
/// affect route geometry.
pub fn set_waypoint_notes(doc: &mut FlightDoc, index: usize, notes: String) -> bool {
    let Some(waypoint) = doc.route.get_mut(index) else {
        tracing::warn!(
            index,
            len = doc.route.len(),
            "set_waypoint_notes out of range"
        );
        return false;
    };
    if waypoint.notes == notes {
        return false;
    }
    waypoint.notes = notes;
    true
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::{LatLon, MetersAmsl};
    use strata_plan::flight::{FreePoint, ManualWind};
    use strata_plan::units::{DegreesTrue, Knots};

    use super::*;

    fn pt(name: &str, lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Free(FreePoint {
            name: Some(name.to_owned()),
            position: LatLon::new(lat, lon).unwrap(),
        })
    }

    fn labels(doc: &FlightDoc) -> Vec<String> {
        doc.route.iter().map(|w| w.point.label()).collect()
    }

    fn alt(feet: f64) -> PlannedAltitude {
        PlannedAltitude::Amsl(MetersAmsl::from_feet(feet))
    }

    #[test]
    fn append_insert_remove_keep_route_order() {
        let mut doc = FlightDoc::new("t");
        assert!(append_waypoint(&mut doc, pt("A", 50.0, 8.0)));
        assert!(append_waypoint(&mut doc, pt("C", 51.0, 10.0)));
        assert!(insert_waypoint(&mut doc, 1, pt("B", 50.5, 9.0)));
        assert_eq!(labels(&doc), ["A", "B", "C"]);

        // Out-of-range indices are rejected without touching the doc.
        assert!(!insert_waypoint(&mut doc, 5, pt("X", 0.0, 0.0)));
        assert!(!remove_waypoint(&mut doc, 3));
        assert_eq!(labels(&doc), ["A", "B", "C"]);

        assert!(remove_waypoint(&mut doc, 1));
        assert_eq!(labels(&doc), ["A", "C"]);
    }

    #[test]
    fn appending_a_coincident_point_normalizes_away() {
        let mut doc = FlightDoc::new("t");
        append_waypoint(&mut doc, pt("A", 50.0, 8.0));
        append_waypoint(&mut doc, pt("A again", 50.0, 8.0));
        // The duplicate collapsed (normalize) — the route stays valid.
        assert_eq!(doc.route.len(), 1);
    }

    #[test]
    fn inserting_into_a_leg_inherits_the_leg_plan() {
        let mut doc = FlightDoc::new("t");
        append_waypoint(&mut doc, pt("A", 50.0, 8.0));
        append_waypoint(&mut doc, pt("B", 51.0, 10.0));
        assert!(set_leg_altitude(&mut doc, 0, Some(alt(4500.0))));

        assert!(insert_waypoint(&mut doc, 1, pt("M", 50.5, 9.0)));
        assert_eq!(doc.route[0].leg_altitude, Some(alt(4500.0)));
        assert_eq!(doc.route[1].leg_altitude, Some(alt(4500.0)), "inherited");
        assert_eq!(doc.route[2].leg_altitude, None, "final waypoint clean");
    }

    #[test]
    fn move_reorders_and_clears_trailing_leg_fields() {
        let mut doc = FlightDoc::new("t");
        append_waypoint(&mut doc, pt("A", 50.0, 8.0));
        append_waypoint(&mut doc, pt("B", 50.5, 9.0));
        append_waypoint(&mut doc, pt("C", 51.0, 10.0));
        set_leg_altitude(&mut doc, 1, Some(alt(3000.0))); // leg B→C

        // Move B to the end: A, C, B.
        assert!(move_waypoint(&mut doc, 1, 2));
        assert_eq!(labels(&doc), ["A", "C", "B"]);
        // B is now final: its leg plan was cleared by normalize.
        assert_eq!(doc.route[2].leg_altitude, None);

        assert!(!move_waypoint(&mut doc, 1, 1), "same index is a no-op");
        assert!(!move_waypoint(&mut doc, 0, 9), "out of range");
    }

    #[test]
    fn replace_point_keeps_the_leg_plan() {
        let mut doc = FlightDoc::new("t");
        append_waypoint(&mut doc, pt("A", 50.0, 8.0));
        append_waypoint(&mut doc, pt("B", 51.0, 10.0));
        set_leg_altitude(&mut doc, 0, Some(alt(5500.0)));
        doc.route[0].leg_wind = Some(ManualWind {
            direction: DegreesTrue::new(270.0),
            speed: Knots(15.0),
        });

        // Drag A elsewhere: position changes, plan stays.
        assert!(replace_waypoint_point(&mut doc, 0, pt("A'", 50.1, 8.1)));
        assert_eq!(doc.route[0].point.label(), "A'");
        assert_eq!(doc.route[0].leg_altitude, Some(alt(5500.0)));
        assert!(doc.route[0].leg_wind.is_some());

        // Same point again: no-op.
        assert!(!replace_waypoint_point(&mut doc, 0, pt("A'", 50.1, 8.1)));
        assert!(!replace_waypoint_point(&mut doc, 7, pt("X", 0.0, 0.0)));
    }

    #[test]
    fn leg_altitude_rejects_the_final_waypoint() {
        let mut doc = FlightDoc::new("t");
        append_waypoint(&mut doc, pt("A", 50.0, 8.0));
        append_waypoint(&mut doc, pt("B", 51.0, 10.0));
        assert!(
            !set_leg_altitude(&mut doc, 1, Some(alt(3000.0))),
            "no outgoing leg"
        );
        assert!(set_leg_altitude(&mut doc, 0, Some(alt(3000.0))));
        assert!(
            !set_leg_altitude(&mut doc, 0, Some(alt(3000.0))),
            "same value"
        );
        assert!(set_leg_altitude(&mut doc, 0, None), "clearing");
    }

    #[test]
    fn scalar_setters_report_change_honestly() {
        let mut doc = FlightDoc::new("t");

        assert!(set_cruise_altitude(&mut doc, Some(alt(4500.0))));
        assert!(!set_cruise_altitude(&mut doc, Some(alt(4500.0))));

        let alternate = pt("ALT", 50.2, 8.8);
        assert!(set_alternate(&mut doc, Some(alternate.clone())));
        assert_eq!(doc.alternates, vec![alternate.clone()]);
        assert!(!set_alternate(&mut doc, Some(alternate)));
        assert!(set_alternate(&mut doc, None));
        assert!(doc.alternates.is_empty());

        let id = AircraftId::new("example-c172").unwrap();
        assert!(set_aircraft(&mut doc, Some(id.clone())));
        assert!(!set_aircraft(&mut doc, Some(id)));

        let time = chrono::Utc.with_ymd_and_hms(2026, 6, 14, 9, 30, 0).unwrap();
        assert!(set_departure_time(&mut doc, Some(time)));
        assert!(!set_departure_time(&mut doc, Some(time)));
        assert!(set_departure_time(&mut doc, None));

        assert!(set_name(&mut doc, "Rhön trip".into()));
        assert!(!set_name(&mut doc, "Rhön trip".into()));
    }
}

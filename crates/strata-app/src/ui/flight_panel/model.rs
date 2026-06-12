//! Pure view-model helpers for the flight panel: route-row readouts,
//! badge derivation from the conflict list, and the formatting/parsing
//! conventions of the panel's compact fields. No gpui here beyond the
//! icon enum — everything is unit-testable.

use chrono::{DateTime, NaiveDate, NaiveTime, Timelike as _, Utc};
use strata_data::domain::MetersAmsl;
use strata_plan::AircraftProfile;
use strata_plan::compute::ComputedLeg;
use strata_plan::conflict::{Conflict, ConflictKind, ConflictSeverity};
use strata_plan::flight::{FreePoint, NamedPointKind, PlannedAltitude, RoutePoint, RouteWaypoint};
use strata_plan::navlog::{NavLogRow, NavLogRowKind};
use strata_plan::units::{DegreesMagnetic, Knots, Liters, Minutes, NauticalMiles};

use crate::assets::IconName;
use crate::state::briefing::{BriefingRelevance, NotamBadge};
use crate::state::flight::FocusRequest;

// --- route rows ---------------------------------------------------------------

/// Kind glyph for a route point (matches the search-row vocabulary; free
/// points get the crosshair — a user-placed coordinate).
pub fn waypoint_icon(point: &RoutePoint) -> IconName {
    match point {
        RoutePoint::Named(named) => match named.kind {
            NamedPointKind::Airport => IconName::Plane,
            NamedPointKind::Navaid => IconName::RadioTower,
            NamedPointKind::ReportingPoint => IconName::Waypoints,
        },
        RoutePoint::Free(_) => IconName::Crosshair,
    }
}

/// Row title: ident for named points; free points show their name, or the
/// coordinates when unnamed (design §3.1).
pub fn waypoint_title(point: &RoutePoint) -> String {
    point.label()
}

/// Muted second line: the published name for named points (when it adds
/// information over the ident), the coordinates for *named* free points
/// (unnamed ones already show coords as the title).
pub fn waypoint_subtitle(point: &RoutePoint) -> Option<String> {
    match point {
        RoutePoint::Named(named) if !named.name.is_empty() && named.name != named.id => {
            Some(named.name.clone())
        }
        RoutePoint::Named(_) => None,
        RoutePoint::Free(free) if free.name.is_some() => Some(free.position.to_string()),
        RoutePoint::Free(_) => None,
    }
}

/// Fly-to zoom per point kind (the search fly-to convention).
pub fn waypoint_fly_zoom(point: &RoutePoint) -> f64 {
    match point {
        RoutePoint::Named(named) => match named.kind {
            NamedPointKind::Airport | NamedPointKind::ReportingPoint => 11.0,
            NamedPointKind::Navaid => 10.0,
        },
        RoutePoint::Free(_) => 11.0,
    }
}

/// The free point the list's "+" affordance inserts into leg `leg`: the
/// great-circle midpoint between its bounding waypoints.
pub fn leg_insert_point(route: &[RouteWaypoint], leg: usize) -> Option<RoutePoint> {
    let from = route.get(leg)?;
    let to = route.get(leg + 1)?;
    Some(RoutePoint::Free(FreePoint {
        name: None,
        position: strata_plan::route::midpoint(from.position(), to.position()),
    }))
}

// --- per-leg computed readouts --------------------------------------------------

/// The compact computed numbers shown on a route-list leg row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegReadout {
    pub distance: NauticalMiles,
    pub magnetic_heading: Option<DegreesMagnetic>,
    pub ground_speed: Option<Knots>,
    pub ete: Option<Minutes>,
}

/// Folds the nav log back onto route legs: rows between two waypoint rows
/// (TOC/TOD splits) belong to the leg arriving at the second one. ETE sums
/// over the leg's intervals; MH/GS come from the leg's dominant (longest)
/// interval — the cruise portion on any leg that reaches cruise. Distances
/// come from the leg summaries, which exist independently of the log.
pub fn leg_readouts(legs: &[ComputedLeg], rows: &[NavLogRow]) -> Vec<LegReadout> {
    let mut readouts: Vec<LegReadout> = legs
        .iter()
        .map(|leg| LegReadout {
            distance: NauticalMiles::from_meters(leg.distance),
            magnetic_heading: None,
            ground_speed: None,
            ete: None,
        })
        .collect();

    let mut leg = 0usize;
    let mut ete_sum = 0.0f64;
    let mut any_ete = false;
    let mut dominant: Option<(f64, Option<DegreesMagnetic>, Option<Knots>)> = None;
    // The first row is the departure waypoint (no arriving leg).
    for row in rows.iter().skip(1) {
        if let Some(ete) = row.ete {
            ete_sum += ete.0;
            any_ete = true;
        }
        let distance = row.distance.map_or(0.0, |d| d.0);
        if dominant.as_ref().is_none_or(|(d, _, _)| distance >= *d) {
            dominant = Some((distance, row.magnetic_heading, row.ground_speed));
        }
        if row.kind == NavLogRowKind::Waypoint {
            if let Some(readout) = readouts.get_mut(leg) {
                readout.ete = any_ete.then_some(Minutes(ete_sum));
                if let Some((_, mh, gs)) = dominant {
                    readout.magnetic_heading = mh;
                    readout.ground_speed = gs;
                }
            }
            leg += 1;
            ete_sum = 0.0;
            any_ete = false;
            dominant = None;
        }
    }
    readouts
}

/// ETA at the destination: the last nav-log row's ETA (rows are in
/// along-track order; `None` without a departure time).
pub fn final_eta(rows: &[NavLogRow]) -> Option<DateTime<Utc>> {
    rows.last()?.eta
}

/// The leg row's compact readout line; `None` (nothing computed) is a
/// single em-dash, missing individual values dash out per field.
pub fn leg_readout_text(readout: Option<&LegReadout>) -> String {
    let Some(readout) = readout else {
        return "—".to_owned();
    };
    let mh = readout
        .magnetic_heading
        .map_or_else(|| "—".to_owned(), fmt_heading);
    let gs = readout
        .ground_speed
        .map_or_else(|| "—".to_owned(), fmt_knots);
    let ete = readout.ete.map_or_else(|| "—".to_owned(), fmt_minutes);
    format!(
        "{} NM · MH {mh} · {gs} kt · {ete}",
        fmt_nm(readout.distance)
    )
}

// --- status badges --------------------------------------------------------------

/// Traffic-light state of one summary badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    /// Not evaluated (no computed flight, or — for NOTAM — no data wired
    /// yet). Renders as an em-dash, never as a false green.
    Unknown,
    Ok,
    Caution,
    Alert,
}

/// One badge of the summary row.
#[derive(Debug, Clone, PartialEq)]
pub struct BadgeVm {
    pub label: &'static str,
    pub tone: BadgeTone,
    /// First offending conflict message (the badge's tooltip).
    pub tooltip: Option<String>,
    /// Where the badge navigates on click (design §3.1: "clicking a badge
    /// opens the relevant surface"); `None` = nothing to open yet (NOTAM
    /// until the briefing phase wires data).
    pub focus: Option<FocusRequest>,
}

/// Badge order, the conflict kinds each one aggregates, and the surface a
/// click opens (design §3.1: W&B / Fuel / Terrain / Airspace; obstacle
/// clearance reports under Terrain; the NOTAM badge is briefing-derived —
/// see [`notam_badge_vm`]). The profile-bound badges navigate to the
/// first conflict of exactly the kinds they aggregate.
const BADGE_KINDS: [(&str, &[ConflictKind], Option<FocusRequest>); 4] = [
    (
        "W&B",
        &[ConflictKind::WeightBalance],
        Some(FocusRequest::Loading),
    ),
    ("Fuel", &[ConflictKind::Fuel], Some(FocusRequest::Fuel)),
    (
        "Terrain",
        &[ConflictKind::Terrain, ConflictKind::Obstacle],
        Some(FocusRequest::Conflicts(&[
            ConflictKind::Terrain,
            ConflictKind::Obstacle,
        ])),
    ),
    (
        "Airspace",
        &[ConflictKind::Airspace],
        Some(FocusRequest::Conflicts(&[ConflictKind::Airspace])),
    ),
];

/// Derives the badge row: the conflict-driven badges from the computed
/// conflicts (`None` = nothing computed → Unknown; no matching conflicts →
/// green; any `Warning` → red, `Caution`/`Info` → amber — the design's
/// "informational amber" for TMZ/RMZ), then the briefing-driven NOTAM
/// badge ([`notam_badge_vm`]) as the row's last entry.
pub fn badge_row(conflicts: Option<&[Conflict]>, notam: BadgeVm) -> Vec<BadgeVm> {
    let mut badges: Vec<BadgeVm> = BADGE_KINDS
        .iter()
        .map(|(label, kinds, focus)| {
            let Some(conflicts) = conflicts else {
                return BadgeVm {
                    label,
                    tone: BadgeTone::Unknown,
                    tooltip: None,
                    focus: *focus,
                };
            };
            let mut matching = conflicts.iter().filter(|c| kinds.contains(&c.kind));
            let Some(first) = matching.next() else {
                return BadgeVm {
                    label,
                    tone: BadgeTone::Ok,
                    tooltip: None,
                    focus: *focus,
                };
            };
            let max_severity = matching
                .map(|c| c.severity)
                .fold(first.severity, ConflictSeverity::max);
            BadgeVm {
                label,
                tone: if max_severity == ConflictSeverity::Warning {
                    BadgeTone::Alert
                } else {
                    BadgeTone::Caution
                },
                tooltip: Some(first.message.clone()),
                focus: *focus,
            }
        })
        .collect();
    badges.push(notam);
    badges
}

/// The NOTAM badge from the briefing state (design §3.1 + §3.4): em-dash
/// before any usable snapshot, green when nothing relevant is active
/// during the flight, amber for active relevant NOTAMs, red for an active
/// restriction activation in the corridor. Clicking always opens the
/// Briefing tab — also the place to fetch when nothing is fetched yet.
pub fn notam_badge_vm(badge: NotamBadge, briefing: Option<&BriefingRelevance>) -> BadgeVm {
    let active_count = briefing.map_or(0, |briefing| {
        briefing
            .relevant
            .iter()
            .filter(|entry| entry.active_during_flight)
            .count()
    });
    let (tone, tooltip) = match badge {
        NotamBadge::NotFetched => (
            BadgeTone::Unknown,
            Some("No NOTAM data yet — fetch from the Briefing tab.".to_owned()),
        ),
        NotamBadge::Clear => (BadgeTone::Ok, None),
        NotamBadge::Relevant => (
            BadgeTone::Caution,
            Some(if active_count == 1 {
                "1 NOTAM active during the flight.".to_owned()
            } else {
                format!("{active_count} NOTAMs active during the flight.")
            }),
        ),
        NotamBadge::RestrictionActive => (
            BadgeTone::Alert,
            Some("Active restriction (ED-R/D/P class) in the route corridor.".to_owned()),
        ),
    };
    BadgeVm {
        label: "NOTAM",
        tone,
        tooltip,
        focus: Some(FocusRequest::Briefing),
    }
}

// --- formatting -----------------------------------------------------------------

/// Distance: one decimal under 100 NM, whole numbers above.
pub fn fmt_nm(distance: NauticalMiles) -> String {
    if distance.0 >= 99.95 {
        format!("{:.0}", distance.0)
    } else {
        format!("{:.1}", distance.0)
    }
}

/// Magnetic heading/track: three digits with the aviation 360-for-north
/// convention.
pub fn fmt_heading(heading: DegreesMagnetic) -> String {
    let deg = (heading.0.round() as i64).rem_euclid(360);
    let deg = if deg == 0 { 360 } else { deg };
    format!("{deg:03}°")
}

/// Ground speed, whole knots.
pub fn fmt_knots(speed: Knots) -> String {
    format!("{:.0}", speed.0)
}

/// Duration as `H:MM` (`0:25`, `1:35`).
pub fn fmt_minutes(minutes: Minutes) -> String {
    let total = minutes.0.round().max(0.0) as i64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// UTC instant as `14:25Z`.
pub fn fmt_eta(t: DateTime<Utc>) -> String {
    t.format("%H:%MZ").to_string()
}

/// Fuel volume, whole liters.
pub fn fmt_liters(volume: Liters) -> String {
    format!("{:.0} L", volume.0)
}

// --- altitude field --------------------------------------------------------------

/// What an altitude field's text means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AltitudeEdit {
    /// Empty field: clear the override (leg fields fall back to cruise;
    /// the cruise field falls back to nothing).
    Clear,
    Set(PlannedAltitude),
    /// Not parseable (mid-edit state) — leave the document untouched.
    Invalid,
}

/// Parses the panel's altitude convention: plain feet (`"3500"`, with an
/// optional `ft` suffix) or a flight level (`"FL95"`).
pub fn parse_altitude(text: &str) -> AltitudeEdit {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return AltitudeEdit::Clear;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(level) = lower.strip_prefix("fl") {
        return match level.trim().parse::<u16>() {
            Ok(n) if n > 0 => AltitudeEdit::Set(PlannedAltitude::FlightLevel(n)),
            _ => AltitudeEdit::Invalid,
        };
    }
    let number = lower
        .strip_suffix("ft")
        .map(str::trim_end)
        .unwrap_or(&lower);
    match number.parse::<f64>() {
        Ok(feet) if feet.is_finite() && (0.0..=100_000.0).contains(&feet) => {
            AltitudeEdit::Set(PlannedAltitude::Amsl(MetersAmsl::from_feet(feet)))
        }
        _ => AltitudeEdit::Invalid,
    }
}

/// Canonical field text for a stored altitude (`""` when unset; what
/// [`parse_altitude`] round-trips).
pub fn altitude_text(altitude: Option<PlannedAltitude>) -> String {
    match altitude {
        None => String::new(),
        Some(PlannedAltitude::Amsl(meters)) => format!("{:.0}", meters.as_feet()),
        Some(PlannedAltitude::FlightLevel(n)) => format!("FL{n}"),
    }
}

/// Display label with unit: `"3500 ft"` / `"FL95"`.
pub fn altitude_label(altitude: PlannedAltitude) -> String {
    match altitude {
        PlannedAltitude::Amsl(meters) => format!("{:.0} ft", meters.as_feet()),
        PlannedAltitude::FlightLevel(n) => format!("FL{n}"),
    }
}

// --- departure date/time field -----------------------------------------------------

/// Parses the departure time field (UTC). Accepted: `""` (clear),
/// `"9"`/`"09"` (whole hour), `"930"`/`"0930"`, `"9:30"`/`"09:30"`, with an
/// optional trailing `Z`. Outer `None` = not parseable.
pub fn parse_time_utc(text: &str) -> Option<Option<NaiveTime>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(None);
    }
    let trimmed = trimmed
        .strip_suffix(['z', 'Z'])
        .unwrap_or(trimmed)
        .trim_end();
    let (hours, minutes) = match trimmed.split_once(':') {
        Some((h, m)) => (h, m),
        None if trimmed.len() <= 2 => (trimmed, "0"),
        None if trimmed.len() <= 4 && trimmed.chars().all(|c| c.is_ascii_digit()) => {
            trimmed.split_at(trimmed.len() - 2)
        }
        None => return None,
    };
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    NaiveTime::from_hms_opt(hours, minutes, 0).map(Some)
}

/// Canonical time-field text (`"09:30"`; empty when unset).
pub fn time_text(departure: Option<DateTime<Utc>>) -> String {
    departure.map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

/// New departure after the *time* field commits: keeps the current date
/// (falling back to `today`).
pub fn departure_with_time(
    current: Option<DateTime<Utc>>,
    time: NaiveTime,
    today: NaiveDate,
) -> DateTime<Utc> {
    let date = current.map_or(today, |t| t.date_naive());
    date.and_time(time).and_utc()
}

/// New departure after the *date* picker commits: keeps the current
/// time-of-day (falling back to 12:00Z — a neutral midday default).
pub fn departure_with_date(current: Option<DateTime<Utc>>, date: NaiveDate) -> DateTime<Utc> {
    let time = current
        .and_then(|t| NaiveTime::from_hms_opt(t.hour(), t.minute(), 0))
        .unwrap_or_else(|| NaiveTime::from_hms_opt(12, 0, 0).expect("12:00 is a valid time"));
    date.and_time(time).and_utc()
}

// --- aircraft selector ----------------------------------------------------------

/// Sentinel select value of the "Manage aircraft…" item (never a profile
/// id — those are slug-validated).
pub const MANAGE_AIRCRAFT_VALUE: &str = "::manage-aircraft::";

/// Trigger/row title for an aircraft profile: registration (falling back
/// to the id) plus the type designator.
pub fn aircraft_choice_title(profile: &AircraftProfile) -> String {
    let registration = if profile.registration.is_empty() {
        profile.id.as_str()
    } else {
        profile.registration.as_str()
    };
    if profile.type_designator.is_empty() {
        registration.to_string()
    } else {
        format!("{registration} · {}", profile.type_designator)
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::LatLon;
    use strata_plan::conflict::ConflictLocation;
    use strata_plan::flight::NamedPoint;
    use strata_plan::units::DegreesTrue;

    use super::*;

    fn named(kind: NamedPointKind, id: &str, name: &str) -> RoutePoint {
        RoutePoint::Named(NamedPoint {
            kind,
            id: id.to_owned(),
            name: name.to_owned(),
            position: LatLon::new(50.0, 8.0).unwrap(),
        })
    }

    fn free(name: Option<&str>) -> RoutePoint {
        RoutePoint::Free(FreePoint {
            name: name.map(str::to_owned),
            position: LatLon::new(50.0, 8.0).unwrap(),
        })
    }

    // --- rows -----------------------------------------------------------

    #[test]
    fn waypoint_rows_show_ident_name_and_coords_for_free_points() {
        let airport = named(NamedPointKind::Airport, "EDFE", "Frankfurt-Egelsbach");
        assert_eq!(waypoint_title(&airport), "EDFE");
        assert_eq!(
            waypoint_subtitle(&airport).as_deref(),
            Some("Frankfurt-Egelsbach")
        );
        assert_eq!(waypoint_icon(&airport), IconName::Plane);

        // Name == id adds nothing.
        let navaid = named(NamedPointKind::Navaid, "FFM", "FFM");
        assert_eq!(waypoint_subtitle(&navaid), None);
        assert_eq!(waypoint_icon(&navaid), IconName::RadioTower);

        // Unnamed free point: coordinates as the title.
        let unnamed = free(None);
        assert_eq!(waypoint_title(&unnamed), "50.00000°N 8.00000°E");
        assert_eq!(waypoint_subtitle(&unnamed), None);
        assert_eq!(waypoint_icon(&unnamed), IconName::Crosshair);

        // Named free point: name as title, coords as subtitle.
        let named_free = free(Some("Lake bend"));
        assert_eq!(waypoint_title(&named_free), "Lake bend");
        assert_eq!(
            waypoint_subtitle(&named_free).as_deref(),
            Some("50.00000°N 8.00000°E")
        );
    }

    #[test]
    fn leg_insert_point_is_the_leg_midpoint() {
        let route = vec![
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: None,
                position: LatLon::new(50.0, 8.0).unwrap(),
            })),
            RouteWaypoint::new(RoutePoint::Free(FreePoint {
                name: None,
                position: LatLon::new(50.0, 10.0).unwrap(),
            })),
        ];
        let Some(RoutePoint::Free(mid)) = leg_insert_point(&route, 0) else {
            panic!("midpoint exists for leg 0");
        };
        assert!(mid.name.is_none());
        assert!((mid.position.lon() - 9.0).abs() < 0.01);
        // Out-of-range legs yield nothing.
        assert_eq!(leg_insert_point(&route, 1), None);
        assert_eq!(leg_insert_point(&[], 0), None);
    }

    // --- readouts ---------------------------------------------------------

    fn row(
        kind: NavLogRowKind,
        distance: Option<f64>,
        ete: Option<f64>,
        mh: Option<f64>,
        gs: Option<f64>,
    ) -> NavLogRow {
        NavLogRow {
            kind,
            label: "x".into(),
            altitude: None,
            true_track: None,
            magnetic_track: None,
            wind: None,
            wind_correction_angle_deg: None,
            magnetic_heading: mh.map(DegreesMagnetic::new),
            tas: None,
            ground_speed: gs.map(Knots),
            distance: distance.map(NauticalMiles),
            ete: ete.map(Minutes),
            eta: None,
            leg_fuel: None,
            cumulative_fuel: None,
            remaining_fuel: None,
            frequency: None,
            notes: String::new(),
        }
    }

    fn leg(index: usize, meters: f64) -> ComputedLeg {
        ComputedLeg {
            index,
            from: "A".into(),
            to: "B".into(),
            distance: strata_data::domain::Meters(meters),
            true_track: DegreesTrue::new(80.0),
            magnetic_track: DegreesMagnetic::new(77.0),
            midpoint: LatLon::new(50.0, 8.5).unwrap(),
        }
    }

    #[test]
    fn toc_split_legs_sum_ete_and_take_cruise_heading() {
        // Leg 0 splits at TOC: 5 NM climb + 30 NM cruise; leg 1 is one row.
        let rows = vec![
            row(NavLogRowKind::Waypoint, None, None, None, None), // departure
            row(
                NavLogRowKind::TopOfClimb,
                Some(5.0),
                Some(4.0),
                Some(82.0),
                Some(70.0),
            ),
            row(
                NavLogRowKind::Waypoint,
                Some(30.0),
                Some(18.0),
                Some(78.0),
                Some(100.0),
            ),
            row(
                NavLogRowKind::Waypoint,
                Some(20.0),
                Some(12.0),
                Some(120.0),
                Some(100.0),
            ),
        ];
        let legs = [leg(0, 35.0 * 1852.0), leg(1, 20.0 * 1852.0)];
        let readouts = leg_readouts(&legs, &rows);
        assert_eq!(readouts.len(), 2);

        assert!((readouts[0].distance.0 - 35.0).abs() < 1e-9);
        assert_eq!(readouts[0].ete, Some(Minutes(22.0)));
        // Cruise interval (30 NM) dominates the climb (5 NM).
        assert_eq!(
            readouts[0].magnetic_heading,
            Some(DegreesMagnetic::new(78.0))
        );
        assert_eq!(readouts[0].ground_speed, Some(Knots(100.0)));

        assert_eq!(readouts[1].ete, Some(Minutes(12.0)));
        assert_eq!(
            readouts[1].magnetic_heading,
            Some(DegreesMagnetic::new(120.0))
        );
    }

    #[test]
    fn readouts_survive_an_empty_or_truncated_navlog() {
        let legs = [leg(0, 1852.0)];
        // No rows at all: distance still present, the rest unknown.
        let readouts = leg_readouts(&legs, &[]);
        assert_eq!(readouts.len(), 1);
        assert!((readouts[0].distance.0 - 1.0).abs() < 1e-9);
        assert_eq!(readouts[0].ete, None);
        assert_eq!(readouts[0].magnetic_heading, None);

        // More waypoint rows than legs: the extras are ignored.
        let rows = vec![
            row(NavLogRowKind::Waypoint, None, None, None, None),
            row(
                NavLogRowKind::Waypoint,
                Some(1.0),
                Some(1.0),
                Some(90.0),
                Some(60.0),
            ),
            row(
                NavLogRowKind::Waypoint,
                Some(9.0),
                Some(9.0),
                Some(90.0),
                Some(60.0),
            ),
        ];
        let readouts = leg_readouts(&legs, &rows);
        assert_eq!(readouts.len(), 1);
        assert_eq!(readouts[0].ete, Some(Minutes(1.0)));
    }

    #[test]
    fn leg_readout_lines_dash_out_missing_values() {
        assert_eq!(leg_readout_text(None), "—");
        let readout = LegReadout {
            distance: NauticalMiles(38.62),
            magnetic_heading: Some(DegreesMagnetic::new(83.0)),
            ground_speed: Some(Knots(95.0)),
            ete: Some(Minutes(25.0)),
        };
        assert_eq!(
            leg_readout_text(Some(&readout)),
            "38.6 NM · MH 083° · 95 kt · 0:25"
        );
        let readout = LegReadout {
            magnetic_heading: None,
            ground_speed: None,
            ete: None,
            ..readout
        };
        assert_eq!(
            leg_readout_text(Some(&readout)),
            "38.6 NM · MH — · — kt · —"
        );
    }

    #[test]
    fn final_eta_is_the_last_rows_eta() {
        assert_eq!(final_eta(&[]), None);
        let mut rows = vec![row(NavLogRowKind::Waypoint, None, None, None, None)];
        assert_eq!(final_eta(&rows), None);
        let eta = chrono::Utc::now();
        rows.push(NavLogRow {
            eta: Some(eta),
            ..row(NavLogRowKind::Waypoint, Some(1.0), Some(1.0), None, None)
        });
        assert_eq!(final_eta(&rows), Some(eta));
    }

    // --- badges ------------------------------------------------------------

    fn conflict(kind: ConflictKind, severity: ConflictSeverity, message: &str) -> Conflict {
        Conflict {
            kind,
            severity,
            location: ConflictLocation::Flight,
            message: message.to_owned(),
        }
    }

    #[test]
    fn badges_are_unknown_without_a_computed_flight() {
        let badges = badge_row(None, notam_badge_vm(NotamBadge::NotFetched, None));
        assert_eq!(badges.len(), 5);
        assert!(badges.iter().all(|b| b.tone == BadgeTone::Unknown));
        // The unfetched NOTAM badge tooltips its call to action; the
        // conflict badges have nothing to say.
        assert!(
            badges
                .iter()
                .filter(|b| b.label != "NOTAM")
                .all(|b| b.tooltip.is_none())
        );
    }

    #[test]
    fn clean_conflicts_are_green_except_an_unfetched_notam_badge() {
        let badges = badge_row(Some(&[]), notam_badge_vm(NotamBadge::NotFetched, None));
        let by_label = |label: &str| badges.iter().find(|b| b.label == label).unwrap();
        for label in ["W&B", "Fuel", "Terrain", "Airspace"] {
            assert_eq!(by_label(label).tone, BadgeTone::Ok, "{label}");
        }
        // No usable snapshot — an absent check must not render green.
        assert_eq!(by_label("NOTAM").tone, BadgeTone::Unknown);
    }

    #[test]
    fn badge_severity_takes_the_worst_conflict_and_tooltips_the_first() {
        let conflicts = [
            conflict(
                ConflictKind::Airspace,
                ConflictSeverity::Caution,
                "TMZ ahead",
            ),
            conflict(
                ConflictKind::Airspace,
                ConflictSeverity::Warning,
                "ED-R crossing",
            ),
            conflict(
                ConflictKind::Obstacle,
                ConflictSeverity::Caution,
                "mast near WP2",
            ),
            conflict(
                ConflictKind::Fuel,
                ConflictSeverity::Info,
                "tabs fueling note",
            ),
        ];
        let badges = badge_row(
            Some(&conflicts),
            notam_badge_vm(NotamBadge::NotFetched, None),
        );
        let by_label = |label: &str| badges.iter().find(|b| b.label == label).unwrap();

        let airspace = by_label("Airspace");
        assert_eq!(airspace.tone, BadgeTone::Alert, "warning escalates to red");
        assert_eq!(
            airspace.tooltip.as_deref(),
            Some("TMZ ahead"),
            "first message"
        );

        // Obstacles report under the Terrain badge.
        let terrain = by_label("Terrain");
        assert_eq!(terrain.tone, BadgeTone::Caution);
        assert_eq!(terrain.tooltip.as_deref(), Some("mast near WP2"));

        // Info-level conflicts still show amber (informational, design §4).
        assert_eq!(by_label("Fuel").tone, BadgeTone::Caution);
        assert_eq!(by_label("W&B").tone, BadgeTone::Ok);
    }

    /// Design §3.1: every badge navigates to its surface — profile-bound
    /// badges to the first conflict of exactly the kinds they aggregate,
    /// W&B/Fuel/NOTAM to their context tabs.
    #[test]
    fn badges_carry_their_navigation_focus() {
        let notam = || notam_badge_vm(NotamBadge::NotFetched, None);
        // The focus mapping is static — identical with and without
        // computed conflicts.
        for badges in [badge_row(None, notam()), badge_row(Some(&[]), notam())] {
            let by_label = |label: &str| badges.iter().find(|b| b.label == label).unwrap();
            assert_eq!(by_label("W&B").focus, Some(FocusRequest::Loading));
            assert_eq!(by_label("Fuel").focus, Some(FocusRequest::Fuel));
            assert_eq!(
                by_label("Terrain").focus,
                Some(FocusRequest::Conflicts(&[
                    ConflictKind::Terrain,
                    ConflictKind::Obstacle,
                ]))
            );
            assert_eq!(
                by_label("Airspace").focus,
                Some(FocusRequest::Conflicts(&[ConflictKind::Airspace]))
            );
            assert_eq!(
                by_label("NOTAM").focus,
                Some(FocusRequest::Briefing),
                "the NOTAM badge opens the Briefing tab"
            );
        }
    }

    /// The NOTAM badge mapping covers the design table: em-dash / green /
    /// amber / red, with the active count in the amber tooltip.
    #[test]
    fn notam_badge_vm_maps_badge_states_to_tones() {
        use chrono::TimeZone as _;
        use strata_data::domain::Notam;
        use strata_plan::notam_relevance::{NotamRelevance, RelevantNotam};

        let unknown = notam_badge_vm(NotamBadge::NotFetched, None);
        assert_eq!(unknown.tone, BadgeTone::Unknown);
        assert!(unknown.tooltip.is_some());

        let clear = notam_badge_vm(NotamBadge::Clear, None);
        assert_eq!(clear.tone, BadgeTone::Ok);
        assert_eq!(clear.tooltip, None);

        let raw = "W0001/26 NOTAMN\nQ) EDGG/QWPLW/IV/M/W/000/050/5000N00815E003\nA) EDGG B) 2606160700 C) 2606181500\nE) PJE";
        let entry = |active| RelevantNotam {
            notam: Notam::parse(raw).expect("parses"),
            relevance: NotamRelevance::Fir,
            active_during_flight: active,
        };
        let briefing = BriefingRelevance {
            taken_at: Utc.with_ymd_and_hms(2026, 6, 16, 8, 30, 0).unwrap(),
            relevant: vec![entry(true), entry(true), entry(false)],
        };

        let amber = notam_badge_vm(NotamBadge::Relevant, Some(&briefing));
        assert_eq!(amber.tone, BadgeTone::Caution);
        assert_eq!(
            amber.tooltip.as_deref(),
            Some("2 NOTAMs active during the flight.")
        );

        let single = BriefingRelevance {
            relevant: vec![entry(true)],
            ..briefing.clone()
        };
        let amber = notam_badge_vm(NotamBadge::Relevant, Some(&single));
        assert_eq!(
            amber.tooltip.as_deref(),
            Some("1 NOTAM active during the flight.")
        );

        let red = notam_badge_vm(NotamBadge::RestrictionActive, Some(&briefing));
        assert_eq!(red.tone, BadgeTone::Alert);
        assert!(red.tooltip.as_deref().unwrap().contains("restriction"));
    }

    // --- formatting ----------------------------------------------------------

    #[test]
    fn distances_headings_speeds_and_durations_format_compactly() {
        assert_eq!(fmt_nm(NauticalMiles(38.62)), "38.6");
        assert_eq!(fmt_nm(NauticalMiles(123.4)), "123");
        assert_eq!(fmt_nm(NauticalMiles(99.96)), "100");

        assert_eq!(fmt_heading(DegreesMagnetic::new(83.4)), "083°");
        assert_eq!(fmt_heading(DegreesMagnetic::new(359.6)), "360°");
        assert_eq!(fmt_heading(DegreesMagnetic::new(0.2)), "360°");

        assert_eq!(fmt_knots(Knots(95.4)), "95");

        assert_eq!(fmt_minutes(Minutes(25.4)), "0:25");
        assert_eq!(fmt_minutes(Minutes(95.0)), "1:35");
        assert_eq!(fmt_minutes(Minutes(-1.0)), "0:00");

        assert_eq!(fmt_liters(Liters(46.6)), "47 L");

        let eta = chrono::DateTime::parse_from_rfc3339("2026-06-14T14:25:31Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(fmt_eta(eta), "14:25Z");
    }

    // --- altitude field ---------------------------------------------------------

    #[test]
    fn altitude_parsing_covers_feet_flight_levels_and_mid_edit_states() {
        assert_eq!(parse_altitude(""), AltitudeEdit::Clear);
        assert_eq!(parse_altitude("  "), AltitudeEdit::Clear);
        assert_eq!(
            parse_altitude("3500"),
            AltitudeEdit::Set(PlannedAltitude::Amsl(MetersAmsl::from_feet(3500.0)))
        );
        assert_eq!(
            parse_altitude("3500 ft"),
            AltitudeEdit::Set(PlannedAltitude::Amsl(MetersAmsl::from_feet(3500.0)))
        );
        assert_eq!(
            parse_altitude("FL95"),
            AltitudeEdit::Set(PlannedAltitude::FlightLevel(95))
        );
        assert_eq!(
            parse_altitude("fl 100"),
            AltitudeEdit::Set(PlannedAltitude::FlightLevel(100))
        );
        // Mid-edit / nonsense states leave the document untouched.
        assert_eq!(parse_altitude("f"), AltitudeEdit::Invalid);
        assert_eq!(parse_altitude("fl"), AltitudeEdit::Invalid);
        assert_eq!(parse_altitude("-500"), AltitudeEdit::Invalid);
        assert_eq!(parse_altitude("abc"), AltitudeEdit::Invalid);
    }

    #[test]
    fn altitude_text_round_trips_through_parse() {
        for altitude in [
            PlannedAltitude::Amsl(MetersAmsl::from_feet(3500.0)),
            PlannedAltitude::FlightLevel(95),
        ] {
            let text = altitude_text(Some(altitude));
            let AltitudeEdit::Set(parsed) = parse_altitude(&text) else {
                panic!("{text:?} must parse");
            };
            assert_eq!(altitude_text(Some(parsed)), text);
        }
        assert_eq!(altitude_text(None), "");
        assert_eq!(
            altitude_label(PlannedAltitude::Amsl(MetersAmsl::from_feet(3000.0))),
            "3000 ft"
        );
        assert_eq!(altitude_label(PlannedAltitude::FlightLevel(95)), "FL95");
    }

    // --- departure field ---------------------------------------------------------

    #[test]
    fn time_parsing_accepts_the_usual_utc_spellings() {
        assert_eq!(parse_time_utc(""), Some(None));
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(parse_time_utc("09:30"), Some(Some(t(9, 30))));
        assert_eq!(parse_time_utc("9:30"), Some(Some(t(9, 30))));
        assert_eq!(parse_time_utc("0930"), Some(Some(t(9, 30))));
        assert_eq!(parse_time_utc("930"), Some(Some(t(9, 30))));
        assert_eq!(parse_time_utc("9"), Some(Some(t(9, 0))));
        assert_eq!(parse_time_utc("09:30Z"), Some(Some(t(9, 30))));
        assert_eq!(parse_time_utc("24:00"), None);
        assert_eq!(parse_time_utc("9:"), None);
        assert_eq!(parse_time_utc("x"), None);
    }

    #[test]
    fn departure_edits_keep_the_other_half() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
        let time = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        let current = date.and_time(time).and_utc();

        // Time edit keeps the date; without a departure it uses today.
        let today = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let new_time = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        assert_eq!(
            departure_with_time(Some(current), new_time, today),
            date.and_time(new_time).and_utc()
        );
        assert_eq!(
            departure_with_time(None, new_time, today),
            today.and_time(new_time).and_utc()
        );

        // Date edit keeps the time-of-day; without one it lands at 12:00Z.
        let new_date = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        assert_eq!(
            departure_with_date(Some(current), new_date),
            new_date.and_time(time).and_utc()
        );
        assert_eq!(
            departure_with_date(None, new_date),
            new_date
                .and_time(NaiveTime::from_hms_opt(12, 0, 0).unwrap())
                .and_utc()
        );
        assert_eq!(time_text(Some(current)), "09:30");
        assert_eq!(time_text(None), "");
    }

    // --- aircraft selector ----------------------------------------------------------

    #[test]
    fn aircraft_titles_prefer_registration_and_type() {
        let mut profile = crate::flight_io::aircraft::example_c172();
        assert_eq!(aircraft_choice_title(&profile), "D-EXAA · C172");
        profile.registration = String::new();
        assert_eq!(aircraft_choice_title(&profile), "example-c172 · C172");
        profile.type_designator = String::new();
        assert_eq!(aircraft_choice_title(&profile), "example-c172");
    }
}

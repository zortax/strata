//! Document + computed outputs → the renderer's route layer input
//! (plan §5.1/§4): a thin, pure conversion to
//! [`strata_render::features::RenderRoute`].
//!
//! The seam: [`AppState::flight_render_route`] produces the route for the
//! current flight state; `MapView` calls it on
//! `FlightOpened/FlightChanged/FlightComputed/FlightClosed` and pushes the
//! result via `MapRenderer::set_route` (the map-editing phase wires that
//! side). View-only adornments — the corridor outline toggle and the
//! profile-drawer scrub marker — are **not** document state: the convert
//! leaves them `None` and the map view fills them in
//! (`corridor_halfwidth_m` from `computed.corridor.params.half_width`).
//!
//! Waypoint ids are the hit-testing contract: the main track uses the
//! route index (`0..route.len()`), alternates use
//! [`ALTERNATE_ID_BASE`]` + alternate index` — `MapView` maps a picked id
//! back to the matching `AppState` mutation.

use strata_plan::FlightDoc;
use strata_plan::compute::{ComputedFlight, ComputedLeg};
use strata_plan::conflict::{Conflict, ConflictLocation};
use strata_plan::flight::PlannedAltitude;
use strata_plan::units::MagneticVariation;
use strata_render::features::{RenderRoute, RoutePointKind, RouteVertex};

use crate::state::AppState;

/// Vertex-id offset for alternates (route indices stay well below this).
pub const ALTERNATE_ID_BASE: u64 = 1 << 32;

/// Converts the open flight into the renderer's route. Works with or
/// without computed outputs: while `computed` is `None` (editing burst,
/// non-computable state) the polyline and handles still render — only
/// conflict tinting and TOC/TOD need a compute.
///
/// `computed` may be **stale** by construction (it belongs to an earlier
/// document generation while the debounced recompute is in flight — e.g.
/// right after deleting a waypoint). The conversion therefore sizes
/// `leg_conflict` by the **current document's** leg count, truncating or
/// padding the stale flags, so the renderer never receives more flags
/// than the track has legs (its defensive warning stays as a backstop and
/// can no longer fire from this path). Stale TOC/TOD markers may briefly
/// sit on the old geometry — visually benign and resolved by the landing
/// recompute.
pub fn render_route(doc: &FlightDoc, computed: Option<&ComputedFlight>) -> RenderRoute {
    let main_len = doc.route.len();
    let mut points = Vec::with_capacity(main_len + doc.alternates.len());
    for (index, waypoint) in doc.route.iter().enumerate() {
        let kind = if index == 0 {
            RoutePointKind::Departure
        } else if index == main_len - 1 {
            RoutePointKind::Destination
        } else {
            RoutePointKind::Waypoint
        };
        let p = waypoint.position();
        points.push(RouteVertex {
            id: index as u64,
            pos: [p.lon(), p.lat()],
            kind,
        });
    }
    for (index, alternate) in doc.alternates.iter().enumerate() {
        let p = alternate.position();
        points.push(RouteVertex {
            id: ALTERNATE_ID_BASE + index as u64,
            pos: [p.lon(), p.lat()],
            kind: RoutePointKind::Alternate,
        });
    }

    let (leg_conflict, leg_labels, toc, tod) = match computed {
        Some(computed) => {
            let leg_distances: Vec<f64> = computed.legs.iter().map(|leg| leg.distance.0).collect();
            let marker = |m: &strata_plan::perf::ProfileMarker| {
                (m.along_track.0, [m.position.lon(), m.position.lat()])
            };
            (
                conflict_leg_flags(
                    main_len.saturating_sub(1),
                    &leg_distances,
                    &computed.conflicts,
                ),
                leg_labels(doc, computed),
                computed.phases.toc.as_ref().map(marker),
                computed.phases.tod.as_ref().map(marker),
            )
        }
        None => (Vec::new(), Vec::new(), None, None),
    };

    RenderRoute {
        points,
        leg_conflict,
        leg_labels,
        snap_ring: None,
        highlight: None,
        toc,
        tod,
        corridor_halfwidth_m: None,
        scrub_along_m: None,
    }
}

/// Per-leg map labels ("MH 053 · 135 kt · 4500"), sized by the **current
/// document's** leg count like [`conflict_leg_flags`]: legs the (possibly
/// stale) compute does not cover stay unlabelled.
fn leg_labels(doc: &FlightDoc, computed: &ComputedFlight) -> Vec<Option<String>> {
    (0..doc.route.len().saturating_sub(1))
        .map(|leg| leg_label(doc, computed, leg))
        .collect()
}

/// One leg's label, or `None` unless **every** part is computable for this
/// leg — where the nav log shows em-dash parts, the map omits the label
/// entirely (a partial "MH — · — kt" annotation would be chart noise).
///
/// Parts mirror the nav-log row semantics: MH from the leg's wind-triangle
/// true heading converted with the leg's variation (recovered from the
/// computed true/magnetic track pair), GS from the same triangle, and the
/// leg's planned altitude (leg override, else the flight cruise default) in
/// the route list's compact grammar (feet without a unit, or `FL95`).
fn leg_label(doc: &FlightDoc, computed: &ComputedFlight, leg: usize) -> Option<String> {
    let summary = computed.legs.get(leg).filter(|l| l.index == leg)?;
    let wind = computed.winds.iter().find(|w| w.leg_index == leg)?;
    let ground_speed = wind.triangle.ground_speed.0;
    if !ground_speed.is_finite() || ground_speed <= 0.0 {
        return None;
    }
    let heading = wind.triangle.true_heading.to_magnetic(leg_variation(summary));
    let altitude = doc.route.get(leg)?.leg_altitude.or(doc.cruise_altitude)?;
    Some(format!(
        "MH {} · {ground_speed:.0} kt · {}",
        fmt_heading(heading),
        fmt_altitude(altitude),
    ))
}

/// The leg's magnetic variation, recovered from the computed true/magnetic
/// track pair (`magnetic = true − variation`, east positive), normalized
/// into (−180°, 180°].
fn leg_variation(leg: &ComputedLeg) -> MagneticVariation {
    let delta = (leg.true_track.0 - leg.magnetic_track.0).rem_euclid(360.0);
    MagneticVariation(if delta > 180.0 { delta - 360.0 } else { delta })
}

/// Three digits with the aviation 360-for-north convention, no degree sign
/// (the label grammar carries the "MH" prefix).
fn fmt_heading(heading: strata_plan::units::DegreesMagnetic) -> String {
    let degrees = (heading.0.round() as i64).rem_euclid(360);
    let degrees = if degrees == 0 { 360 } else { degrees };
    format!("{degrees:03}")
}

/// Compact altitude: whole feet without a unit, or `FL95`.
fn fmt_altitude(altitude: PlannedAltitude) -> String {
    match altitude {
        PlannedAltitude::Amsl(meters) => format!("{:.0}", meters.as_feet()),
        PlannedAltitude::FlightLevel(level) => format!("FL{level}"),
    }
}

/// Per-leg conflict flags from the conflict list, sized by the **current
/// document's** leg count (`doc_leg_count`): `Leg`-anchored conflicts flag
/// their leg directly, `Station`-anchored ones flag the leg containing the
/// station's along-track distance per the *computed* leg distances
/// (past-the-end rounds into the computed run's final leg — distances
/// carry float noise). Document-level conflicts (W&B, fuel) tint no leg;
/// they live in the badge row.
///
/// `leg_distances_m` and `conflicts` may describe a **stale** compute with
/// more or fewer legs than the document (the debounce window after a
/// waypoint edit): flags beyond the document's legs are dropped, missing
/// trailing legs stay conflict-free — the output length is always
/// `doc_leg_count`, the renderer's contract.
fn conflict_leg_flags(
    doc_leg_count: usize,
    leg_distances_m: &[f64],
    conflicts: &[Conflict],
) -> Vec<bool> {
    let mut flags = vec![false; doc_leg_count];
    if flags.is_empty() || leg_distances_m.is_empty() {
        return flags;
    }
    for conflict in conflicts {
        let leg = match conflict.location {
            ConflictLocation::Leg { index } => index,
            ConflictLocation::Station { along_track, .. } => {
                let mut cumulative = 0.0;
                let mut leg = leg_distances_m.len() - 1;
                for (index, distance) in leg_distances_m.iter().enumerate() {
                    cumulative += distance;
                    if along_track.0 <= cumulative {
                        leg = index;
                        break;
                    }
                }
                leg
            }
            ConflictLocation::Flight => continue,
        };
        // Stale legs beyond the current document are dropped.
        if let Some(flag) = flags.get_mut(leg) {
            *flag = true;
        }
    }
    flags
}

// Consumed by the map-editing phase (MapView pushes it into the renderer).
#[allow(dead_code)]
impl AppState {
    /// The renderer route for the open flight; `None` in explorer mode
    /// (the map view then calls `set_route(None)`).
    pub fn flight_render_route(&self) -> Option<RenderRoute> {
        let flight = self.flight.as_ref()?;
        Some(render_route(&flight.doc, flight.computed.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::{LatLon, Meters, MetersAmsl};
    use strata_plan::conflict::{ConflictKind, ConflictSeverity};
    use strata_plan::corridor::{Corridor, CorridorParams};
    use strata_plan::flight::{FreePoint, RoutePoint, RouteWaypoint};
    use strata_plan::fuel::FuelLadder;
    use strata_plan::navlog::{NavLog, NavLogTotals};
    use strata_plan::perf::PhasePlan;
    use strata_plan::sources::{Provenance, WindsAloft};
    use strata_plan::units::{Celsius, DegreesTrue, Knots, Liters, Minutes, NauticalMiles};
    use strata_plan::wind::{LegWind, LegWindOrigin, WindTriangle};

    use super::*;

    fn pt(lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Free(FreePoint {
            name: None,
            position: LatLon::new(lat, lon).unwrap(),
        })
    }

    fn doc(waypoints: &[(f64, f64)], alternates: &[(f64, f64)]) -> FlightDoc {
        let mut doc = FlightDoc::new("t");
        doc.route = waypoints
            .iter()
            .map(|&(lat, lon)| RouteWaypoint::new(pt(lat, lon)))
            .collect();
        doc.alternates = alternates.iter().map(|&(lat, lon)| pt(lat, lon)).collect();
        doc
    }

    /// A minimal computed flight: one summary per leg distance (true track
    /// 090°, 3°E variation → magnetic 087°), plus the given winds and
    /// conflicts; everything else empty.
    fn computed_flight(
        leg_distances_m: &[f64],
        winds: Vec<LegWind>,
        conflicts: Vec<Conflict>,
    ) -> ComputedFlight {
        ComputedFlight {
            legs: leg_distances_m
                .iter()
                .enumerate()
                .map(|(index, &distance)| strata_plan::compute::ComputedLeg {
                    index,
                    from: format!("W{index}"),
                    to: format!("W{}", index + 1),
                    distance: Meters(distance),
                    true_track: DegreesTrue::new(90.0),
                    magnetic_track: DegreesTrue::new(90.0).to_magnetic(MagneticVariation(3.0)),
                    midpoint: LatLon::new(50.0, 8.0).unwrap(),
                })
                .collect(),
            corridor: Corridor {
                params: CorridorParams::default(),
                samples: Vec::new(),
                crossings: Vec::new(),
            },
            winds,
            phases: PhasePlan {
                segments: Vec::new(),
                toc: None,
                tod: None,
                total_duration: Minutes(0.0),
                total_fuel: Liters(0.0),
            },
            weight_balance: strata_plan::wb::WbReport {
                states: Vec::new(),
                burn_track: Vec::new(),
            },
            fuel: FuelLadder {
                taxi: Liters(0.0),
                trip: Liters(0.0),
                contingency: Liters(0.0),
                alternate: Liters(0.0),
                final_reserve: Liters(0.0),
                extra: Liters(0.0),
                minimum_required: Liters(0.0),
                loaded: Liters(0.0),
                margin: Liters(0.0),
            },
            conflicts,
            navlog: NavLog {
                rows: Vec::new(),
                totals: NavLogTotals {
                    distance: NauticalMiles(0.0),
                    ete: Minutes(0.0),
                    fuel: Liters(0.0),
                },
            },
        }
    }

    /// A solved leg wind: the triangle's true heading and ground speed are
    /// what the label reads.
    fn leg_wind(leg_index: usize, true_heading: f64, ground_speed: f64) -> LegWind {
        LegWind {
            leg_index,
            wind: WindsAloft {
                direction: DegreesTrue::new(270.0),
                speed: Knots(10.0),
                temperature: Celsius(5.0),
                temperature_provenance: Provenance::Isa,
            },
            origin: LegWindOrigin::Sampled,
            triangle: WindTriangle {
                wind_correction_angle_deg: 0.0,
                true_heading: DegreesTrue::new(true_heading),
                ground_speed: Knots(ground_speed),
            },
        }
    }

    #[test]
    fn vertices_carry_kinds_ids_and_lon_lat_positions() {
        let doc = doc(&[(50.0, 8.0), (50.5, 9.0), (51.0, 10.0)], &[(50.8, 10.5)]);
        let route = render_route(&doc, None);

        assert_eq!(route.points.len(), 4);
        assert_eq!(route.points[0].kind, RoutePointKind::Departure);
        assert_eq!(route.points[1].kind, RoutePointKind::Waypoint);
        assert_eq!(route.points[2].kind, RoutePointKind::Destination);
        assert_eq!(route.points[3].kind, RoutePointKind::Alternate);

        // Ids: route index for the main track, offset for alternates.
        assert_eq!(route.points[1].id, 1);
        assert_eq!(route.points[3].id, ALTERNATE_ID_BASE);

        // pos is [lon, lat] (the renderer's ring convention).
        assert_eq!(route.points[0].pos, [8.0, 50.0]);

        // Without computed outputs: no tinting, no labels, no markers.
        assert!(route.leg_conflict.is_empty());
        assert!(route.leg_labels.is_empty());
        assert!(route.toc.is_none() && route.tod.is_none());
        assert!(route.corridor_halfwidth_m.is_none());
        assert!(route.scrub_along_m.is_none());
        // The snap ring and the hover highlight are interaction state —
        // never set by the conversion (the push seam overlays them).
        assert!(route.snap_ring.is_none());
        assert!(route.highlight.is_none());
    }

    #[test]
    fn single_point_and_empty_routes_degrade_gracefully() {
        let route = render_route(&doc(&[], &[]), None);
        assert!(route.points.is_empty());

        let route = render_route(&doc(&[(50.0, 8.0)], &[]), None);
        assert_eq!(route.points.len(), 1);
        assert_eq!(route.points[0].kind, RoutePointKind::Departure);
    }

    fn conflict(location: ConflictLocation) -> Conflict {
        Conflict {
            kind: ConflictKind::Terrain,
            severity: ConflictSeverity::Warning,
            location,
            message: "test".to_owned(),
        }
    }

    fn station(m: f64) -> ConflictLocation {
        ConflictLocation::Station {
            along_track: Meters(m),
            position: LatLon::new(50.0, 8.0).unwrap(),
        }
    }

    #[test]
    fn conflicts_map_to_legs_by_anchor() {
        // Three legs: 10 km, 20 km, 30 km (computed matches the doc).
        let legs = [10_000.0, 20_000.0, 30_000.0];

        // Leg-anchored.
        let flags = conflict_leg_flags(3, &legs, &[conflict(ConflictLocation::Leg { index: 1 })]);
        assert_eq!(flags, [false, true, false]);

        // Station-anchored: 25 km lies in leg 1 (10–30 km).
        let flags = conflict_leg_flags(3, &legs, &[conflict(station(25_000.0))]);
        assert_eq!(flags, [false, true, false]);

        // Leg boundary belongs to the earlier leg (≤).
        let flags = conflict_leg_flags(3, &legs, &[conflict(station(10_000.0))]);
        assert_eq!(flags, [true, false, false]);

        // Past the end (float noise) rounds into the final leg.
        let flags = conflict_leg_flags(3, &legs, &[conflict(station(60_001.0))]);
        assert_eq!(flags, [false, false, true]);

        // Document-level conflicts tint nothing; out-of-range leg indices
        // are ignored.
        let flags = conflict_leg_flags(
            3,
            &legs,
            &[
                conflict(ConflictLocation::Flight),
                conflict(ConflictLocation::Leg { index: 9 }),
            ],
        );
        assert_eq!(flags, [false, false, false]);

        assert!(conflict_leg_flags(0, &[], &[conflict(ConflictLocation::Flight)]).is_empty());
    }

    /// The stale-compute race: the flags are sized by the *document's* leg
    /// count even when the computed outputs describe a different route
    /// generation.
    #[test]
    fn stale_computed_conflicts_clamp_to_the_document_legs() {
        // Stale compute saw three legs; the doc now has two (a waypoint
        // was deleted). Conflicts on the vanished third leg are dropped,
        // earlier ones keep their tint, and the length is the doc's.
        let stale = [10_000.0, 20_000.0, 30_000.0];
        let flags = conflict_leg_flags(
            2,
            &stale,
            &[
                conflict(station(25_000.0)),                    // stale leg 1 — kept
                conflict(station(45_000.0)),                    // stale leg 2 — dropped
                conflict(ConflictLocation::Leg { index: 2 }),   // dropped
            ],
        );
        assert_eq!(flags, [false, true]);

        // Past-the-end float noise rounds into the stale run's final leg,
        // which no longer exists — dropped, never a panic.
        let flags = conflict_leg_flags(2, &stale, &[conflict(station(60_001.0))]);
        assert_eq!(flags, [false, false]);

        // The opposite race (waypoint added): stale compute saw one leg,
        // the doc has three — trailing legs pad conflict-free.
        let flags = conflict_leg_flags(3, &[10_000.0], &[conflict(station(5_000.0))]);
        assert_eq!(flags, [true, false, false]);

        // Stale compute with no legs at all: nothing to map, doc-sized.
        let flags = conflict_leg_flags(2, &[], &[conflict(station(5_000.0))]);
        assert_eq!(flags, [false, false]);
    }

    /// Full-conversion guarantee the renderer relies on: `leg_conflict`
    /// never exceeds the pushed track's leg count, even with a computed
    /// flight from an older document generation.
    #[test]
    fn render_route_sizes_leg_conflicts_by_the_current_doc() {
        // A stale compute for the *4-waypoint* (3-leg) version of a route
        // whose document now has 3 waypoints (2 legs).
        let stale = computed_flight(
            &[10_000.0, 20_000.0, 30_000.0],
            Vec::new(),
            vec![
                conflict(station(25_000.0)), // stale leg 1
                conflict(station(45_000.0)), // stale leg 2 (vanished)
            ],
        );

        let doc = doc(&[(50.0, 8.0), (50.5, 9.0), (51.0, 10.0)], &[]);
        let route = render_route(&doc, Some(&stale));
        assert_eq!(
            route.leg_conflict.len(),
            doc.route.len() - 1,
            "flags sized by the current doc, not the stale compute"
        );
        assert_eq!(route.leg_conflict, [false, true]);
    }

    /// Leg labels compose MH (wind-triangle heading through the leg's
    /// variation) · GS · planned altitude in the compact map grammar.
    #[test]
    fn leg_labels_compose_mh_gs_and_altitude() {
        let mut doc = doc(&[(50.0, 8.0), (50.5, 9.0), (51.0, 10.0)], &[]);
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)));
        // Leg 1 overrides its altitude as a flight level.
        doc.route[1].leg_altitude = Some(PlannedAltitude::FlightLevel(95));

        let computed = computed_flight(
            &[10_000.0, 20_000.0],
            // True heading 056° with the fixture's 3°E variation → MH 053.
            vec![leg_wind(0, 56.0, 135.4), leg_wind(1, 182.6, 98.6)],
            Vec::new(),
        );
        let route = render_route(&doc, Some(&computed));
        assert_eq!(
            route.leg_labels,
            vec![
                Some("MH 053 · 135 kt · 4500".to_owned()),
                Some("MH 180 · 99 kt · FL95".to_owned()),
            ]
        );

        // Magnetic north renders as 360 (the aviation convention), and the
        // label count always follows the document's legs.
        let north = computed_flight(
            &[10_000.0, 20_000.0],
            vec![leg_wind(0, 3.2, 110.0)], // 3.2° − 3°E → 000.2 → "360"
            Vec::new(),
        );
        let route = render_route(&doc, Some(&north));
        assert_eq!(route.leg_labels.len(), 2);
        assert_eq!(
            route.leg_labels[0].as_deref(),
            Some("MH 360 · 110 kt · 4500")
        );
        assert_eq!(route.leg_labels[1], None, "no wind solved for leg 1");
    }

    /// Where the nav log would show em-dash parts the map omits the label
    /// entirely: missing wind, missing altitude or a stale compute that
    /// no longer covers the leg all yield `None` — never a partial label.
    #[test]
    fn unresolvable_label_parts_omit_the_leg_label() {
        // No resolvable altitude (no cruise default, no leg override).
        let bare = doc(&[(50.0, 8.0), (50.5, 9.0)], &[]);
        let computed = computed_flight(&[10_000.0], vec![leg_wind(0, 56.0, 120.0)], Vec::new());
        let route = render_route(&bare, Some(&computed));
        assert_eq!(route.leg_labels, vec![None]);

        // A degenerate (non-positive) ground speed labels nothing.
        let mut with_alt = doc(&[(50.0, 8.0), (50.5, 9.0)], &[]);
        with_alt.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)));
        let stalled = computed_flight(&[10_000.0], vec![leg_wind(0, 56.0, 0.0)], Vec::new());
        assert_eq!(render_route(&with_alt, Some(&stalled)).leg_labels, vec![None]);

        // Stale compute with fewer legs than the doc: uncovered legs stay
        // unlabelled, the vector is still doc-sized.
        let mut grown = doc(&[(50.0, 8.0), (50.5, 9.0), (51.0, 10.0)], &[]);
        grown.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)));
        let stale = computed_flight(&[10_000.0], vec![leg_wind(0, 56.0, 120.0)], Vec::new());
        let route = render_route(&grown, Some(&stale));
        assert_eq!(route.leg_labels.len(), 2);
        assert!(route.leg_labels[0].is_some());
        assert_eq!(route.leg_labels[1], None);
    }

    /// The variation recovered from the computed true/magnetic pair stays
    /// in (−180°, 180°] across the wrap (a west variation stored as a
    /// 359°-ish delta must come back as −1°, not +359°).
    #[test]
    fn leg_variation_recovers_signed_variation_across_the_wrap() {
        let mut leg = computed_flight(&[10_000.0], Vec::new(), Vec::new()).legs[0].clone();
        leg.true_track = DegreesTrue::new(1.0);
        leg.magnetic_track = DegreesTrue::new(1.0).to_magnetic(MagneticVariation(5.0)); // 356°
        assert!((leg_variation(&leg).0 - 5.0).abs() < 1e-9);

        leg.true_track = DegreesTrue::new(359.0);
        leg.magnetic_track = DegreesTrue::new(359.0).to_magnetic(MagneticVariation(-2.0)); // 1°
        assert!((leg_variation(&leg).0 + 2.0).abs() < 1e-9);
    }
}

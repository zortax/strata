//! The [`BriefingInput`] conversion (design §4 "Briefing PDF"): flight
//! document + computed outputs + live weather + the derived NOTAM
//! relevance → the plain serializable context `strata-brief` renders.
//!
//! Every section degrades honestly: a missing source yields `None` and the
//! PDF renders that section as "not available" — the conversion itself
//! never fails. All quantities are unwrapped into the unit-suffixed `f64`s
//! of the brief contract; altitudes become datum-carrying strings.
//!
//! Provenance is part of the contract: the NOTAM section carries a caveat
//! when the snapshot is rendered without configured autorouter
//! credentials (a stored snapshot that cannot be refreshed), and the winds
//! table carries the [`winds_source_note`] derived from the per-leg
//! [`Provenance`]/origin chain — the PDF never passes an ISA assumption
//! off as forecast data.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use strata_brief::{
    AerodromeWeather, BriefingInput, CgPoint, FlightSummary, FuelSection, LegWind, NavLogRow,
    NavLogRowKind, NavLogSection, NotamCard, NotamSection, WbLoadingRow, WbSection, WbStateRow,
    WeatherSection, WindsAloftRow,
};
use strata_data::domain::{FlightCategory, IcaoCode, Metar, Taf};
use strata_plan::AircraftProfile;
use strata_plan::aircraft::StationKind;
use strata_plan::compute::ComputedFlight;
use strata_plan::flight::{Contingency, FlightDoc, RoutePoint};
use strata_plan::fuel;
use strata_plan::navlog;
use strata_plan::sources::Provenance;
use strata_plan::wb::WbStateKind;
use strata_plan::wind::LegWindOrigin;

use crate::sources::WindsAloftFrames;
use crate::state::briefing::{BriefingRelevance, NotamSource};
use crate::ui::context_tabs::weather::{freezing_level_readout, route_weather_stations};
use crate::ui::info_panel::metar_summary;
use crate::ui::profile_drawer::navlog::waypoint_row_indices;

use super::labels;

/// Everything the conversion reads, borrowed from `AppState` for one call.
/// Optional sources mean "render that section as not available".
pub(crate) struct BriefingSources<'a> {
    pub doc: &'a FlightDoc,
    pub aircraft: Option<&'a AircraftProfile>,
    pub computed: Option<&'a ComputedFlight>,
    /// The derived NOTAM relevance (snapshot + ranking; see
    /// `state::briefing`).
    pub briefing: Option<&'a BriefingRelevance>,
    /// Which provider class would serve a NOTAM refresh — fills the
    /// section's provenance caveat when credentials are missing.
    pub notam_source: NotamSource,
    /// Live METARs/TAFs by station (the same source the Weather tab reads).
    pub metars: &'a HashMap<IcaoCode, Metar>,
    pub tafs: &'a HashMap<IcaoCode, Taf>,
    /// The prefetched winds/temperature frames (the freezing-level chain;
    /// an empty default degrades to the labelled estimate).
    pub winds_frames: &'a WindsAloftFrames,
    /// When the live weather was last fetched (`None` = never).
    pub weather_taken_at: Option<DateTime<Utc>>,
    /// Caller-provided generation timestamp (cover/footer/PDF metadata —
    /// part of the input so rendering stays deterministic).
    pub generated_at: DateTime<Utc>,
}

/// Builds the complete briefing context. Pure and infallible.
pub(crate) fn briefing_input(sources: &BriefingSources<'_>) -> BriefingInput {
    BriefingInput {
        flight: flight_summary(sources),
        generated_at: sources.generated_at,
        navlog: sources.computed.map(|c| navlog_section(sources.doc, c)),
        fuel: sources.computed.map(|c| fuel_section(sources, c)),
        weight_balance: sources
            .computed
            .map(|c| wb_section(sources.doc, sources.aircraft, c)),
        weather: weather_section(sources),
        notams: sources
            .briefing
            .map(|briefing| notam_section(briefing, sources.notam_source)),
    }
}

// --- cover ----------------------------------------------------------------------

fn flight_summary(sources: &BriefingSources<'_>) -> FlightSummary {
    let doc = sources.doc;
    let name = if doc.name.trim().is_empty() {
        crate::flight_io::flights::route_summary(doc)
    } else {
        doc.name.clone()
    };
    let totals = sources.computed.map(|c| &c.navlog.totals);
    FlightSummary {
        name,
        route: doc.route.iter().map(|w| w.point.label()).collect(),
        alternate: doc.alternates.first().map(alternate_label),
        aircraft_type: sources
            .aircraft
            .map(|a| a.type_designator.clone())
            .filter(|t| !t.is_empty()),
        registration: sources
            .aircraft
            .map(|a| a.registration.clone())
            .filter(|r| !r.is_empty()),
        callsign: sources.aircraft.and_then(callsign),
        departure_time: doc.departure_time,
        cruise_altitude: doc.cruise_altitude.map(labels::fmt_planned_altitude),
        total_distance_nm: totals.map(|t| t.distance.0),
        total_ete_minutes: totals.map(|t| t.ete.0),
        total_fuel_liters: totals.map(|t| t.fuel.0),
        remarks: None,
    }
}

/// `"EDQA Bamberg"` for named alternates, the plain label otherwise.
fn alternate_label(point: &RoutePoint) -> String {
    match point {
        RoutePoint::Named(named) if !named.name.is_empty() && named.name != named.id => {
            format!("{} {}", named.id, named.name)
        }
        other => other.label(),
    }
}

/// The filed callsign: the profile's explicit callsign, else the
/// registration without hyphens (the GA norm, same as FPL item 7).
fn callsign(aircraft: &AircraftProfile) -> Option<String> {
    if !aircraft.callsign.is_empty() {
        return Some(aircraft.callsign.clone());
    }
    let derived = aircraft.registration.replace('-', "");
    (!derived.is_empty()).then_some(derived)
}

// --- nav log --------------------------------------------------------------------

/// The PLOG table. Notes come from the **document** (overlaid via the
/// drawer's row↔waypoint mapping): notes edits deliberately skip the
/// recompute (the notes-only fast path in `state::flight`), so the
/// computed rows' copies can be stale — the document's never are. When the
/// mapping is unresolvable (a stale compute mid-debounce) the rows' copies
/// stand.
fn navlog_section(doc: &FlightDoc, computed: &ComputedFlight) -> NavLogSection {
    let indices = waypoint_row_indices(&computed.navlog.rows, doc.route.len());
    let rows = computed
        .navlog
        .rows
        .iter()
        .zip(&indices)
        .map(|(row, route_index)| {
            let mut converted = navlog_row(row);
            if let Some(waypoint) = route_index.and_then(|i| doc.route.get(i)) {
                converted.notes = waypoint.notes.clone();
            }
            converted
        })
        .collect();
    NavLogSection {
        rows,
        total_distance_nm: computed.navlog.totals.distance.0,
        total_ete_minutes: computed.navlog.totals.ete.0,
        total_fuel_liters: computed.navlog.totals.fuel.0,
    }
}

fn navlog_row(row: &navlog::NavLogRow) -> NavLogRow {
    NavLogRow {
        kind: match row.kind {
            navlog::NavLogRowKind::Waypoint => NavLogRowKind::Waypoint,
            navlog::NavLogRowKind::TopOfClimb => NavLogRowKind::TopOfClimb,
            navlog::NavLogRowKind::TopOfDescent => NavLogRowKind::TopOfDescent,
        },
        label: row.label.clone(),
        altitude: row.altitude.map(labels::fmt_planned_altitude),
        true_track_deg: row.true_track.map(|t| t.0),
        magnetic_track_deg: row.magnetic_track.map(|t| t.0),
        magnetic_heading_deg: row.magnetic_heading.map(|h| h.0),
        wind: row.wind.as_ref().map(|w| LegWind {
            direction_deg: w.direction.0,
            speed_kt: w.speed.0,
            temperature_c: Some(w.temperature.0),
        }),
        wind_correction_angle_deg: row.wind_correction_angle_deg,
        tas_kt: row.tas.map(|t| t.0),
        ground_speed_kt: row.ground_speed.map(|g| g.0),
        distance_nm: row.distance.map(|d| d.0),
        ete_minutes: row.ete.map(|e| e.0),
        eta: row.eta,
        leg_fuel_liters: row.leg_fuel.map(|f| f.0),
        remaining_fuel_liters: row.remaining_fuel.map(|f| f.0),
        frequency: row.frequency.as_ref().map(frequency_label),
        notes: row.notes.clone(),
    }
}

/// `"LANGEN INFORMATION 128.950 MHz"`, falling back to the kind when the
/// station has no published name.
fn frequency_label(f: &strata_data::domain::Frequency) -> String {
    if f.name.is_empty() {
        format!("{:?} {}", f.kind, f.frequency)
    } else {
        format!("{} {}", f.name, f.frequency)
    }
}

// --- fuel -----------------------------------------------------------------------

fn fuel_section(sources: &BriefingSources<'_>, computed: &ComputedFlight) -> FuelSection {
    let ladder = &computed.fuel;
    let endurance = sources.aircraft.and_then(|aircraft| {
        fuel::endurance(
            aircraft,
            sources.doc.power_setting.as_deref(),
            ladder.loaded,
        )
        .ok()
    });
    FuelSection {
        taxi_liters: ladder.taxi.0,
        trip_liters: ladder.trip.0,
        contingency_liters: ladder.contingency.0,
        alternate_liters: ladder.alternate.0,
        final_reserve_liters: ladder.final_reserve.0,
        extra_liters: ladder.extra.0,
        minimum_required_liters: ladder.minimum_required.0,
        loaded_liters: ladder.loaded.0,
        margin_liters: ladder.margin.0,
        endurance_minutes: endurance.map(|e| e.0),
        policy_note: Some(policy_note(sources.doc)),
    }
}

/// The fuel policy spelled out, with the regulation disclaimer the design
/// requires on every policy surface.
fn policy_note(doc: &FlightDoc) -> String {
    let policy = &doc.fuel_policy;
    let contingency = match policy.contingency {
        Contingency::PercentOfTrip(percent) => format!("{percent:.0} % of trip"),
        Contingency::Fixed(liters) => format!("{:.1} L fixed", liters.0),
    };
    format!(
        "Fuel policy: taxi {:.0} min, contingency {contingency}, final reserve {:.0} min, \
         extra {:.1} L — EASA Part-NCO template values, verify current regulation.",
        policy.taxi.0, policy.final_reserve.0, policy.extra.0
    )
}

// --- weight & balance -------------------------------------------------------------

fn wb_section(
    doc: &FlightDoc,
    aircraft: Option<&AircraftProfile>,
    computed: &ComputedFlight,
) -> WbSection {
    WbSection {
        loading: aircraft.map(|a| loading_rows(doc, a)).unwrap_or_default(),
        states: computed
            .weight_balance
            .states
            .iter()
            .map(|state| WbStateRow {
                label: wb_state_label(state.kind).to_owned(),
                mass_kg: state.mass.0,
                cg_arm_m: state.arm.0,
                within_limits: state.within_envelope,
            })
            .collect(),
        envelope: aircraft
            .map(|a| {
                a.weight_balance
                    .envelope
                    .iter()
                    .map(|p| CgPoint {
                        arm_m: p.arm.0,
                        mass_kg: p.mass.0,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        burn_track: computed
            .weight_balance
            .burn_track
            .iter()
            .map(|p| CgPoint {
                arm_m: p.arm.0,
                mass_kg: p.mass.0,
            })
            .collect(),
        notes: None,
    }
}

fn wb_state_label(kind: WbStateKind) -> &'static str {
    match kind {
        WbStateKind::Ramp => "Ramp",
        WbStateKind::Takeoff => "Takeoff",
        WbStateKind::ZeroFuel => "Zero fuel",
        WbStateKind::Landing => "Landing",
    }
}

/// The loading table mirroring the W&B model: empty aircraft, every
/// station load at its profile arm, and the scenario fuel at the mean
/// fuel-station arm (the wb module's equal-split convention).
fn loading_rows(doc: &FlightDoc, aircraft: &AircraftProfile) -> Vec<WbLoadingRow> {
    let wb = &aircraft.weight_balance;
    let mut rows = vec![WbLoadingRow {
        station: "Empty aircraft".to_owned(),
        mass_kg: wb.empty_mass.0,
        arm_m: wb.empty_arm.0,
    }];
    for load in &doc.loading.station_loads {
        let Some(station) = wb.stations.iter().find(|s| s.name == load.station) else {
            continue; // stale scenario row; the compute surfaces the error
        };
        rows.push(WbLoadingRow {
            station: station.name.clone(),
            mass_kg: load.mass.0,
            arm_m: station.arm.0,
        });
    }
    let fuel_arms: Vec<f64> = wb
        .stations
        .iter()
        .filter(|s| s.kind == StationKind::Fuel)
        .map(|s| s.arm.0)
        .collect();
    if doc.loading.fuel.0 > 0.0 && !fuel_arms.is_empty() {
        rows.push(WbLoadingRow {
            station: format!("Fuel ({:.0} L)", doc.loading.fuel.0),
            mass_kg: doc.loading.fuel.0 * aircraft.fuel.density.0,
            arm_m: fuel_arms.iter().sum::<f64>() / fuel_arms.len() as f64,
        });
    }
    rows
}

// --- weather --------------------------------------------------------------------

/// `None` when nothing weather-shaped exists (no stations on the route and
/// nothing computed) — the section then prints "not available" instead of
/// an empty husk.
fn weather_section(sources: &BriefingSources<'_>) -> Option<WeatherSection> {
    let aerodromes: Vec<AerodromeWeather> = route_weather_stations(sources.doc)
        .into_iter()
        .map(|station| {
            let (metar, taf) = IcaoCode::new(&station.icao)
                .ok()
                .map(|icao| (sources.metars.get(&icao), sources.tafs.get(&icao)))
                .unwrap_or((None, None));
            AerodromeWeather {
                icao: station.icao.clone(),
                name: (!station.name.is_empty()).then(|| station.name.clone()),
                role: station.role.label().to_owned(),
                flight_category: metar
                    .and_then(|m| m.decoded.as_ref())
                    .and_then(|d| d.flight_category())
                    .map(|c| flight_category_label(c).to_owned()),
                metar_raw: metar.map(|m| m.raw.clone()),
                metar_decoded: metar.and_then(|m| m.decoded.as_ref()).map(metar_summary),
                taf_raw: taf.map(|t| t.raw.clone()),
                taf_decoded: None,
            }
        })
        .collect();

    let winds_aloft: Vec<WindsAloftRow> = sources
        .computed
        .map(|computed| {
            computed
                .legs
                .iter()
                .zip(computed.winds.iter())
                .map(|(leg, wind)| WindsAloftRow {
                    leg: format!("{} → {}", leg.from, leg.to),
                    altitude: leg_altitude_label(sources.doc, wind.leg_index),
                    direction_deg: wind.wind.direction.0,
                    speed_kt: wind.wind.speed.0,
                    temperature_c: Some(wind.wind.temperature.0),
                })
                .collect()
        })
        .unwrap_or_default();

    // The labelled freezing-level chain (hzerocl → real temperatures →
    // lapse-rate estimate) — the label travels into the PDF string.
    let freezing_level = sources.computed.and_then(|computed| {
        freezing_level_readout(sources.winds_frames, sources.doc, computed)
            .map(|readout| format!("≈ {:.0} ft AMSL ({})", readout.feet, readout.source_label))
    });

    if aerodromes.is_empty() && winds_aloft.is_empty() {
        return None;
    }
    Some(WeatherSection {
        snapshot_time: sources.weather_taken_at,
        aerodromes,
        winds_aloft,
        freezing_level,
        winds_source_note: sources
            .computed
            .and_then(|computed| winds_source_note(&computed.winds)),
    })
}

/// The winds-aloft provenance caveat — a pure decision over the per-leg
/// origin/[`Provenance`] chain:
///
/// - every leg fell back to calm-ISA → `"ISA estimate — no forecast data"`;
/// - manual overrides only → labelled as such (their OATs are ISA);
/// - any sampled leg → the forecast is named, with explicit qualifiers for
///   manual overrides and for legs/temperatures the model could not serve.
///   All sampled with real OATs → the plain forecast note (no caveat
///   needed beyond naming the source).
pub(crate) fn winds_source_note(winds: &[strata_plan::wind::LegWind]) -> Option<String> {
    if winds.is_empty() {
        return None;
    }
    let any = |origin: LegWindOrigin| winds.iter().any(|w| w.origin == origin);
    let sampled = any(LegWindOrigin::Sampled);
    let fallback = any(LegWindOrigin::IsaFallback);
    let manual = any(LegWindOrigin::Manual);
    let isa_oat = winds
        .iter()
        .any(|w| w.wind.temperature_provenance == Provenance::Isa);

    if !sampled {
        return Some(match (manual, fallback) {
            (true, false) => "Manual wind overrides — temperatures are ISA estimates".to_owned(),
            (true, true) => "Manual wind overrides and ISA estimates — no forecast data".to_owned(),
            _ => "ISA estimate — no forecast data".to_owned(),
        });
    }
    let mut note = "ICON-D2 forecast (DWD)".to_owned();
    if manual {
        note.push_str("; manual overrides on some legs");
    }
    if fallback || isa_oat {
        note.push_str("; ISA estimates where no data");
    }
    Some(note)
}

/// The planned altitude of leg `index` as the datum-carrying string
/// (leg override, else the cruise altitude), em-dash when unplanned.
fn leg_altitude_label(doc: &FlightDoc, index: usize) -> String {
    doc.route
        .get(index)
        .and_then(|w| w.leg_altitude)
        .or(doc.cruise_altitude)
        .map(labels::fmt_planned_altitude)
        .unwrap_or_else(|| "—".to_owned())
}

fn flight_category_label(category: FlightCategory) -> &'static str {
    match category {
        FlightCategory::Vfr => "VFR",
        FlightCategory::Mvfr => "MVFR",
        FlightCategory::Ifr => "IFR",
        FlightCategory::Lifr => "LIFR",
    }
}

// --- NOTAMs ---------------------------------------------------------------------

/// The relevance-ordered briefing list, one card per entry. An empty list
/// is `Some` with no cards — "no relevant NOTAMs" is a statement, distinct
/// from "never fetched" (`None`). A snapshot exported while autorouter
/// credentials are missing carries its visible caveat (it cannot have
/// been refreshed); the configured live source carries none.
fn notam_section(briefing: &BriefingRelevance, source: NotamSource) -> NotamSection {
    NotamSection {
        snapshot_time: Some(briefing.taken_at),
        source_note: match source {
            NotamSource::NotConfigured => Some(
                "Stored snapshot — autorouter credentials not configured, \
                 refresh unavailable"
                    .to_owned(),
            ),
            NotamSource::Autorouter => None,
        },
        notams: briefing
            .relevant
            .iter()
            .map(|entry| NotamCard {
                id: entry.notam.id.to_string(),
                location: labels::location_label(&entry.notam),
                relevance: Some(labels::relevance_label(&entry.relevance)),
                validity: labels::validity_label(&entry.notam.validity),
                schedule: entry.notam.schedule.clone(),
                limits: labels::q_limits(&entry.notam.q),
                summary: entry.notam.text.clone(),
                raw: entry.notam.raw.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::{LatLon, MetersAmsl, Notam};
    use strata_data::providers::autorouter::FixtureNotamProvider;
    use strata_plan::flight::{NamedPoint, NamedPointKind, PlannedAltitude, RouteWaypoint};
    use strata_plan::notam_relevance::{NotamRelevance, RelevantNotam, TimeWindow};
    use strata_plan::units::NauticalMiles;

    use crate::state::briefing::{NotamSnapshotPayload, encode_snapshot};

    use super::*;

    fn utc(d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, d, h, mi, 0)
            .single()
            .expect("valid")
    }

    fn airport(id: &str, name: &str, lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Named(NamedPoint {
            kind: NamedPointKind::Airport,
            id: id.to_owned(),
            name: name.to_owned(),
            position: LatLon::new(lat, lon).expect("valid"),
        })
    }

    /// The computed-flight recipe shared with the briefing state tests:
    /// EDFE → EDQN over a synthetic elevation store.
    fn computed_flight() -> (
        tempfile::TempDir,
        FlightDoc,
        AircraftProfile,
        ComputedFlight,
    ) {
        use strata_data::store::{ELEVATION_TILE_SIDE, ElevationTile, ElevationTileId, Store};

        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::open(&dir.path().join("store.sqlite")).expect("store opens");
        for lon in [8.0, 8.5, 9.0] {
            let id = ElevationTileId::containing(50.0, lon);
            let tile = ElevationTile::new(id, vec![250; ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE])
                .expect("tile");
            store.put_elevation_tile(&tile).expect("tile stored");
        }

        let aircraft = crate::flight_io::aircraft::example_c172();
        let mut doc = FlightDoc::new("Bavaria Test Hop");
        doc.route = vec![
            RouteWaypoint::new(airport("EDFE", "Egelsbach", 50.0, 8.0)),
            RouteWaypoint::new(airport("EDQN", "Neustadt/Aisch", 50.0, 9.0)),
        ];
        doc.alternates = vec![airport("EDQA", "Bamberg", 49.92, 10.91)];
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)));
        doc.departure_time = Some(utc(16, 9, 30));
        doc.aircraft_id = Some(aircraft.id.clone());
        doc.loading.fuel = strata_plan::units::Liters(150.0);

        let (outcome, _) = crate::state::flight::compute::run_compute(
            &doc,
            Some(&aircraft),
            Some(std::sync::Arc::new(store)),
            std::sync::Arc::new(crate::sources::WindsAloftFrames::default()),
            &strata_plan::compute::ComputeParams::default(),
            None,
        );
        let crate::state::flight::compute::ComputeOutcome::Computed(computed) = outcome else {
            panic!("test flight computes: {outcome:?}");
        };
        (
            dir,
            doc,
            aircraft,
            std::sync::Arc::unwrap_or_clone(computed),
        )
    }

    fn briefing_relevance(doc: &FlightDoc, computed: &ComputedFlight) -> BriefingRelevance {
        let payload = NotamSnapshotPayload {
            notams: FixtureNotamProvider::builtin().notams().to_vec(),
            window: TimeWindow::new(utc(16, 8, 0), utc(17, 9, 0)),
        };
        let mut doc = doc.clone();
        doc.notam_snapshot = Some(encode_snapshot(&payload, utc(16, 8, 30)).expect("encodes"));
        crate::state::briefing::derive_briefing(&doc, Some(computed)).expect("derives")
    }

    #[test]
    fn full_flight_populates_every_section() {
        let (_dir, doc, aircraft, computed) = computed_flight();
        let briefing = briefing_relevance(&doc, &computed);
        let metar =
            strata_data::decode::decode_metar("EDDF 160920Z 24008KT 9999 FEW035 18/09 Q1021")
                .expect("decodes");
        let metars: HashMap<IcaoCode, Metar> = [(
            IcaoCode::new("EDFE").expect("valid"),
            Metar {
                raw: "EDFE 160920Z 24008KT 9999 FEW035 18/09 Q1021".to_owned(),
                station: IcaoCode::new("EDFE").expect("valid"),
                observed_at: utc(16, 9, 20),
                decoded: Some(metar),
            },
        )]
        .into();

        let input = briefing_input(&BriefingSources {
            doc: &doc,
            aircraft: Some(&aircraft),
            computed: Some(&computed),
            briefing: Some(&briefing),
            notam_source: NotamSource::Autorouter,
            metars: &metars,
            tafs: &HashMap::new(),
            winds_frames: &WindsAloftFrames::default(),
            weather_taken_at: Some(utc(16, 9, 25)),
            generated_at: utc(16, 9, 30),
        });

        // Cover.
        assert_eq!(input.flight.name, "Bavaria Test Hop");
        assert_eq!(input.flight.route, ["EDFE", "EDQN"].map(str::to_owned));
        assert_eq!(input.flight.alternate.as_deref(), Some("EDQA Bamberg"));
        assert_eq!(input.flight.aircraft_type.as_deref(), Some("C172"));
        assert_eq!(
            input.flight.cruise_altitude.as_deref(),
            Some("4500 ft AMSL")
        );
        assert!(input.flight.total_distance_nm.expect("computed") > 30.0);
        assert_eq!(input.generated_at, utc(16, 9, 30));

        // Nav log: row shape mirrors the computed PLOG.
        let navlog = input.navlog.expect("computed navlog");
        assert_eq!(navlog.rows.len(), computed.navlog.rows.len());
        assert_eq!(navlog.rows[0].kind, NavLogRowKind::Waypoint);
        assert_eq!(navlog.rows[0].label, "EDFE");
        assert!(navlog.total_distance_nm > 30.0);
        let toc = navlog
            .rows
            .iter()
            .find(|r| r.kind == NavLogRowKind::TopOfClimb)
            .expect("climb produces a TOC row");
        assert_eq!(toc.altitude.as_deref(), Some("4500 ft AMSL"));

        // Fuel: the ladder carried over, the note names the template.
        let fuel = input.fuel.expect("computed fuel");
        assert!(fuel.trip_liters > 0.0);
        assert_eq!(fuel.loaded_liters, 150.0);
        assert!(fuel.endurance_minutes.expect("endurance computes") > 0.0);
        let note = fuel.policy_note.expect("policy note");
        assert!(note.contains("verify current regulation"), "{note}");

        // W&B: four states, loading rows, envelope + burn track present.
        let wb = input.weight_balance.expect("computed wb");
        assert_eq!(wb.states.len(), 4);
        assert_eq!(wb.states[0].label, "Ramp");
        assert!(wb.loading[0].station == "Empty aircraft");
        assert!(
            wb.loading
                .iter()
                .any(|row| row.station.starts_with("Fuel (150 L")),
            "{:?}",
            wb.loading
        );
        assert!(wb.envelope.len() >= 3);
        assert!(!wb.burn_track.is_empty());

        // Weather: departure has METAR + decoded summary; alternate listed
        // without data; winds-aloft rows mirror the legs.
        let weather = input.weather.expect("stations on the route");
        assert_eq!(weather.snapshot_time, Some(utc(16, 9, 25)));
        assert_eq!(weather.aerodromes.len(), 3);
        let departure = &weather.aerodromes[0];
        assert_eq!(departure.icao, "EDFE");
        assert_eq!(departure.role, "Departure");
        assert_eq!(departure.flight_category.as_deref(), Some("VFR"));
        assert!(
            departure
                .metar_decoded
                .as_deref()
                .expect("decoded")
                .contains("240°")
        );
        let alternate = &weather.aerodromes[2];
        assert_eq!(alternate.role, "Alternate");
        assert_eq!(alternate.metar_raw, None);
        assert_eq!(weather.winds_aloft.len(), computed.winds.len());
        assert_eq!(weather.winds_aloft[0].altitude, "4500 ft AMSL");
        // Provenance: no frames were prefetched, so the winds are the
        // calm-ISA fallback and both caveats say so.
        assert_eq!(
            weather.winds_source_note.as_deref(),
            Some("ISA estimate — no forecast data")
        );
        let freezing = weather.freezing_level.as_deref().expect("estimate");
        assert!(
            freezing.contains("ISA estimate — no forecast data"),
            "{freezing}"
        );

        // NOTAMs: the relevance order carries over 1:1; the configured
        // live source carries no provenance caveat.
        let notams = input.notams.expect("briefing present");
        assert_eq!(notams.snapshot_time, Some(utc(16, 8, 30)));
        assert_eq!(notams.source_note, None);
        assert_eq!(notams.notams.len(), briefing.relevant.len());
        assert!(!notams.notams.is_empty());
        let ids: Vec<&str> = notams.notams.iter().map(|n| n.id.as_str()).collect();
        let expected: Vec<String> = briefing
            .relevant
            .iter()
            .map(|e| e.notam.id.to_string())
            .collect();
        assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(notams.notams.iter().all(|n| !n.raw.is_empty()));
    }

    /// The sparse path: a bare two-point document with nothing computed,
    /// no aircraft, no weather, no snapshot. Every optional section is
    /// `None` (the PDF renders "not available"), and the cover still
    /// carries the honest minimum.
    #[test]
    fn sparse_flight_yields_none_sections_not_lies() {
        let mut doc = FlightDoc::new("");
        doc.route = vec![
            RouteWaypoint::new(RoutePoint::Free(strata_plan::flight::FreePoint {
                name: None,
                position: LatLon::new(49.0, 9.0).expect("valid"),
            })),
            RouteWaypoint::new(RoutePoint::Free(strata_plan::flight::FreePoint {
                name: Some("Hill".to_owned()),
                position: LatLon::new(49.5, 9.5).expect("valid"),
            })),
        ];

        let input = briefing_input(&BriefingSources {
            doc: &doc,
            aircraft: None,
            computed: None,
            briefing: None,
            notam_source: NotamSource::Autorouter,
            metars: &HashMap::new(),
            tafs: &HashMap::new(),
            winds_frames: &WindsAloftFrames::default(),
            weather_taken_at: None,
            generated_at: utc(16, 9, 30),
        });

        // Blank name falls back to the route summary.
        assert!(!input.flight.name.is_empty());
        assert_eq!(input.flight.route.len(), 2);
        assert_eq!(input.flight.route[1], "Hill");
        assert_eq!(input.flight.alternate, None);
        assert_eq!(input.flight.aircraft_type, None);
        assert_eq!(input.flight.registration, None);
        assert_eq!(input.flight.callsign, None);
        assert_eq!(input.flight.departure_time, None);
        assert_eq!(input.flight.cruise_altitude, None);
        assert_eq!(input.flight.total_distance_nm, None);

        assert_eq!(input.navlog, None);
        assert_eq!(input.fuel, None);
        assert_eq!(input.weight_balance, None);
        // Free points carry no weather stations and nothing computed: the
        // weather section is honestly absent, not empty.
        assert_eq!(input.weather, None);
        assert_eq!(input.notams, None);
    }

    /// A briefing with zero relevant NOTAMs is `Some` with no cards —
    /// "no relevant NOTAMs" prints as a statement, never as "no data".
    #[test]
    fn empty_relevance_is_a_statement_not_missing_data() {
        let briefing = BriefingRelevance {
            taken_at: utc(16, 8, 30),
            relevant: Vec::new(),
        };
        let section = notam_section(&briefing, NotamSource::Autorouter);
        assert_eq!(section.snapshot_time, Some(utc(16, 8, 30)));
        assert!(section.notams.is_empty());
    }

    #[test]
    fn notam_cards_decode_relevance_validity_and_limits() {
        let raw = "D0001/26 NOTAMN\nQ) EDMM/QRRCA/IV/BO/W/000/100/4942N01156E010\nA) EDMM B) 2606160700 C) 2606181500\nE) ED-R 136 ACT";
        let briefing = BriefingRelevance {
            taken_at: utc(16, 8, 30),
            relevant: vec![RelevantNotam {
                notam: Notam::parse(raw).expect("parses"),
                relevance: NotamRelevance::RouteCorridor {
                    distance_nm: NauticalMiles(1.2),
                },
                active_during_flight: true,
            }],
        };
        let card = &notam_section(&briefing, NotamSource::Autorouter).notams[0];
        assert_eq!(card.id, "D0001/26");
        assert_eq!(card.location, "EDMM");
        assert_eq!(
            card.relevance.as_deref(),
            Some("Corridor · 1.2 NM off track")
        );
        assert_eq!(card.validity, "16 Jun 07:00Z → 18 Jun 15:00Z");
        assert_eq!(card.limits.as_deref(), Some("GND → FL 100"));
        assert_eq!(card.summary, "ED-R 136 ACT");
        assert_eq!(card.raw, raw);
    }

    /// The NOTAM caveat tracks the provider seam: a snapshot exported
    /// without configured credentials is labelled a stored snapshot, the
    /// configured live source carries no caveat.
    #[test]
    fn notam_source_note_flags_missing_credentials() {
        let briefing = BriefingRelevance {
            taken_at: utc(16, 8, 30),
            relevant: Vec::new(),
        };
        assert_eq!(
            notam_section(&briefing, NotamSource::NotConfigured)
                .source_note
                .as_deref(),
            Some("Stored snapshot — autorouter credentials not configured, refresh unavailable")
        );
        assert_eq!(
            notam_section(&briefing, NotamSource::Autorouter).source_note,
            None
        );
    }

    // --- winds provenance note (pure decisions) ------------------------------

    fn leg(origin: LegWindOrigin, provenance: Provenance) -> strata_plan::wind::LegWind {
        strata_plan::wind::LegWind {
            leg_index: 0,
            wind: strata_plan::sources::WindsAloft {
                direction: strata_plan::units::DegreesTrue::new(270.0),
                speed: strata_plan::units::Knots(10.0),
                temperature: strata_plan::units::Celsius(5.0),
                temperature_provenance: provenance,
            },
            origin,
            triangle: strata_plan::wind::WindTriangle {
                wind_correction_angle_deg: 0.0,
                true_heading: strata_plan::units::DegreesTrue::new(90.0),
                ground_speed: strata_plan::units::Knots(100.0),
            },
        }
    }

    #[test]
    fn winds_source_note_walks_the_provenance_decisions() {
        use LegWindOrigin::*;
        use Provenance::*;

        // Nothing computed → nothing to caveat.
        assert_eq!(winds_source_note(&[]), None);

        // Every leg fell back: the honest ISA note.
        assert_eq!(
            winds_source_note(&[leg(IsaFallback, Isa), leg(IsaFallback, Isa)]).as_deref(),
            Some("ISA estimate — no forecast data")
        );

        // Manual-only and manual-plus-fallback flights are labelled as such.
        assert_eq!(
            winds_source_note(&[leg(Manual, Isa)]).as_deref(),
            Some("Manual wind overrides — temperatures are ISA estimates")
        );
        assert_eq!(
            winds_source_note(&[leg(Manual, Isa), leg(IsaFallback, Isa)]).as_deref(),
            Some("Manual wind overrides and ISA estimates — no forecast data")
        );

        // All sampled with real OATs: the plain forecast provenance.
        assert_eq!(
            winds_source_note(&[leg(Sampled, Real), leg(Sampled, Real)]).as_deref(),
            Some("ICON-D2 forecast (DWD)")
        );

        // Sampled but with ISA-pinned OATs (temperature grids missing):
        // qualified, never passed off as fully real.
        assert_eq!(
            winds_source_note(&[leg(Sampled, Isa), leg(Sampled, Real)]).as_deref(),
            Some("ICON-D2 forecast (DWD); ISA estimates where no data")
        );
        // Same for a calm-fallback leg among sampled ones.
        assert_eq!(
            winds_source_note(&[leg(Sampled, Real), leg(IsaFallback, Isa)]).as_deref(),
            Some("ICON-D2 forecast (DWD); ISA estimates where no data")
        );

        // Every source class present: both qualifiers.
        assert_eq!(
            winds_source_note(&[leg(Sampled, Real), leg(Manual, Isa), leg(IsaFallback, Isa)])
                .as_deref(),
            Some(
                "ICON-D2 forecast (DWD); manual overrides on some legs; ISA estimates where no data"
            )
        );
    }

    /// Notes in the PDF come from the document, not the (possibly stale)
    /// computed rows — the notes-only fast path skips the recompute.
    #[test]
    fn navlog_notes_overlay_from_the_document() {
        let (_dir, mut doc, _aircraft, computed) = computed_flight();
        // A notes edit after the compute landed (no recompute scheduled).
        doc.route[0].notes = "noise abatement: climb straight ahead".to_owned();

        let navlog = navlog_section(&doc, &computed);
        assert_eq!(
            navlog.rows[0].notes,
            "noise abatement: climb straight ahead"
        );

        // An unresolvable mapping (route changed under the compute) keeps
        // the rows' own copies instead of guessing.
        doc.route.remove(1);
        let stale = navlog_section(&doc, &computed);
        assert_eq!(stale.rows[0].notes, computed.navlog.rows[0].notes);
    }

    /// The full input renders to an actual PDF (the strata-brief smoke
    /// contract: bytes, %PDF header) — the conversion output satisfies the
    /// renderer's expectations end to end.
    #[test]
    fn converted_input_renders_to_pdf() {
        let (_dir, doc, aircraft, computed) = computed_flight();
        let briefing = briefing_relevance(&doc, &computed);
        let input = briefing_input(&BriefingSources {
            doc: &doc,
            aircraft: Some(&aircraft),
            computed: Some(&computed),
            briefing: Some(&briefing),
            notam_source: NotamSource::Autorouter,
            metars: &HashMap::new(),
            tafs: &HashMap::new(),
            winds_frames: &WindsAloftFrames::default(),
            weather_taken_at: None,
            generated_at: utc(16, 9, 30),
        });
        let pdf = strata_brief::render_briefing(&input).expect("renders");
        assert!(pdf.starts_with(b"%PDF"), "PDF header expected");
    }
}

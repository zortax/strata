//! Shared test fixtures: a fully populated briefing input (every section
//! present, realistic Bavaria-hop values) and a minimal one (every section
//! `None`) for the conditional-rendering tests.

use chrono::{TimeZone, Utc};

use crate::input::*;

/// Fixed generation timestamp — determinism tests rely on it.
pub(crate) fn generated_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 11, 9, 30, 0).unwrap()
}

/// A briefing input with every section populated.
pub(crate) fn full_input() -> BriefingInput {
    BriefingInput {
        flight: FlightSummary {
            name: "Bavaria Test Hop".to_owned(),
            route: ["EDMA", "DON", "TRU", "ROT", "EDDN"]
                .map(str::to_owned)
                .to_vec(),
            alternate: Some("EDQA Bamberg".to_owned()),
            aircraft_type: Some("C172".to_owned()),
            registration: Some("D-EABC".to_owned()),
            callsign: Some("DEABC".to_owned()),
            departure_time: Some(Utc.with_ymd_and_hms(2026, 6, 12, 8, 0, 0).unwrap()),
            cruise_altitude: Some("5500 ft AMSL".to_owned()),
            total_distance_nm: Some(78.4),
            total_ete_minutes: Some(47.0),
            total_fuel_liters: Some(38.2),
            remarks: Some("Training flight, right-hand circuit at EDDN expected.".to_owned()),
        },
        generated_at: generated_at(),
        navlog: Some(navlog()),
        fuel: Some(fuel()),
        weight_balance: Some(weight_balance()),
        weather: Some(weather()),
        notams: Some(notams()),
    }
}

/// A briefing input with no computed/snapshotted data at all.
pub(crate) fn minimal_input() -> BriefingInput {
    BriefingInput {
        flight: FlightSummary {
            name: "Empty Flight".to_owned(),
            route: vec!["EDMA".to_owned(), "EDDN".to_owned()],
            alternate: None,
            aircraft_type: None,
            registration: None,
            callsign: None,
            departure_time: None,
            cruise_altitude: None,
            total_distance_nm: None,
            total_ete_minutes: None,
            total_fuel_liters: None,
            remarks: None,
        },
        generated_at: generated_at(),
        navlog: None,
        fuel: None,
        weight_balance: None,
        weather: None,
        notams: None,
    }
}

fn navlog() -> NavLogSection {
    let waypoint = |label: &str| NavLogRow {
        kind: NavLogRowKind::Waypoint,
        label: label.to_owned(),
        altitude: None,
        true_track_deg: None,
        magnetic_track_deg: None,
        magnetic_heading_deg: None,
        wind: None,
        wind_correction_angle_deg: None,
        tas_kt: None,
        ground_speed_kt: None,
        distance_nm: None,
        ete_minutes: None,
        eta: None,
        leg_fuel_liters: None,
        remaining_fuel_liters: None,
        frequency: None,
        notes: String::new(),
    };
    let leg = |kind: NavLogRowKind,
               label: &str,
               altitude: &str,
               track: f64,
               dist: f64,
               ete: f64,
               fuel: f64,
               remaining: f64| NavLogRow {
        kind,
        label: label.to_owned(),
        altitude: Some(altitude.to_owned()),
        true_track_deg: Some(track),
        magnetic_track_deg: Some(track + 2.4),
        magnetic_heading_deg: Some(track + 6.0),
        wind: Some(LegWind {
            direction_deg: 240.0,
            speed_kt: 15.0,
            temperature_c: Some(2.5),
        }),
        wind_correction_angle_deg: Some(3.6),
        tas_kt: Some(105.0),
        ground_speed_kt: Some(98.0),
        distance_nm: Some(dist),
        ete_minutes: Some(ete),
        eta: Some(Utc.with_ymd_and_hms(2026, 6, 12, 8, ete as u32, 0).unwrap()),
        leg_fuel_liters: Some(fuel),
        remaining_fuel_liters: Some(remaining),
        frequency: Some("Langen Info 128.950".to_owned()),
        notes: String::new(),
    };
    let mut rows = vec![waypoint("EDMA")];
    rows.push(leg(
        NavLogRowKind::TopOfClimb,
        "TOC",
        "5500 ft AMSL",
        21.0,
        8.2,
        7.0,
        6.1,
        110.4,
    ));
    rows.push(leg(
        NavLogRowKind::Waypoint,
        "DON",
        "5500 ft AMSL",
        21.0,
        9.9,
        6.0,
        4.4,
        106.0,
    ));
    rows.push(leg(
        NavLogRowKind::Waypoint,
        "TRU",
        "5500 ft AMSL",
        47.0,
        18.7,
        11.0,
        8.3,
        97.7,
    ));
    rows.push(leg(
        NavLogRowKind::Waypoint,
        "ROT",
        "5500 ft AMSL",
        12.0,
        19.6,
        12.0,
        8.7,
        89.0,
    ));
    rows.push(leg(
        NavLogRowKind::TopOfDescent,
        "TOD",
        "5500 ft AMSL",
        9.0,
        14.5,
        9.0,
        6.4,
        82.6,
    ));
    let mut destination = leg(
        NavLogRowKind::Waypoint,
        "EDDN",
        "1045 ft AMSL",
        9.0,
        7.5,
        6.0,
        4.3,
        78.3,
    );
    destination.notes = "Expect RWY 28".to_owned();
    rows.push(destination);
    NavLogSection {
        rows,
        total_distance_nm: 78.4,
        total_ete_minutes: 51.0,
        total_fuel_liters: 38.2,
    }
}

fn fuel() -> FuelSection {
    FuelSection {
        taxi_liters: 1.5,
        trip_liters: 38.2,
        contingency_liters: 1.9,
        alternate_liters: 8.4,
        final_reserve_liters: 9.0,
        extra_liters: 0.0,
        minimum_required_liters: 59.0,
        loaded_liters: 120.0,
        margin_liters: 61.0,
        endurance_minutes: Some(290.0),
        policy_note: Some(
            "Fuel policy: EASA Part-NCO template (30 min day-VFR final reserve, \
             5 % contingency) — template values, verify current regulation."
                .to_owned(),
        ),
    }
}

fn weight_balance() -> WbSection {
    WbSection {
        loading: vec![
            WbLoadingRow {
                station: "Empty aircraft".to_owned(),
                mass_kg: 743.0,
                arm_m: 1.006,
            },
            WbLoadingRow {
                station: "Pilot & front passenger".to_owned(),
                mass_kg: 154.0,
                arm_m: 0.94,
            },
            WbLoadingRow {
                station: "Rear passengers".to_owned(),
                mass_kg: 70.0,
                arm_m: 1.85,
            },
            WbLoadingRow {
                station: "Baggage A".to_owned(),
                mass_kg: 15.0,
                arm_m: 2.41,
            },
            WbLoadingRow {
                station: "Fuel (120 L AVGAS)".to_owned(),
                mass_kg: 86.4,
                arm_m: 1.17,
            },
        ],
        states: vec![
            WbStateRow {
                label: "Ramp".to_owned(),
                mass_kg: 1068.4,
                cg_arm_m: 1.078,
                within_limits: true,
            },
            WbStateRow {
                label: "Takeoff".to_owned(),
                mass_kg: 1067.3,
                cg_arm_m: 1.078,
                within_limits: true,
            },
            WbStateRow {
                label: "Zero fuel".to_owned(),
                mass_kg: 982.0,
                cg_arm_m: 1.070,
                within_limits: true,
            },
            WbStateRow {
                label: "Landing".to_owned(),
                mass_kg: 1039.8,
                cg_arm_m: 1.076,
                within_limits: true,
            },
        ],
        envelope: vec![
            CgPoint {
                arm_m: 0.89,
                mass_kg: 757.0,
            },
            CgPoint {
                arm_m: 0.89,
                mass_kg: 885.0,
            },
            CgPoint {
                arm_m: 1.00,
                mass_kg: 1111.0,
            },
            CgPoint {
                arm_m: 1.20,
                mass_kg: 1111.0,
            },
            CgPoint {
                arm_m: 1.20,
                mass_kg: 757.0,
            },
        ],
        burn_track: vec![
            CgPoint {
                arm_m: 1.078,
                mass_kg: 1067.3,
            },
            CgPoint {
                arm_m: 1.075,
                mass_kg: 1024.6,
            },
            CgPoint {
                arm_m: 1.070,
                mass_kg: 982.0,
            },
        ],
        notes: Some("Example aircraft data — replace with your POH values.".to_owned()),
    }
}

fn weather() -> WeatherSection {
    WeatherSection {
        snapshot_time: Some(Utc.with_ymd_and_hms(2026, 6, 11, 9, 20, 0).unwrap()),
        aerodromes: vec![
            AerodromeWeather {
                icao: "EDMA".to_owned(),
                name: Some("Augsburg".to_owned()),
                role: "Departure".to_owned(),
                flight_category: Some("VFR".to_owned()),
                metar_raw: Some("EDMA 110920Z 24008KT 9999 FEW035 SCT100 18/09 Q1021".to_owned()),
                metar_decoded: Some(
                    "Wind 240° at 8 kt, visibility 10 km or more, few clouds at \
                     3500 ft, scattered at 10000 ft, 18 °C / dew point 9 °C, QNH 1021."
                        .to_owned(),
                ),
                taf_raw: Some(
                    "TAF EDMA 110900Z 1109/1209 24008KT 9999 SCT040\n  TEMPO 1112/1118 25012G22KT"
                        .to_owned(),
                ),
                taf_decoded: Some(
                    "Wind 240° at 8 kt, visibility 10 km or more, scattered at 4000 ft.\n\
                     Temporarily 25012G22KT between 12Z and 18Z."
                        .to_owned(),
                ),
            },
            AerodromeWeather {
                icao: "EDDN".to_owned(),
                name: Some("Nuremberg".to_owned()),
                role: "Destination".to_owned(),
                flight_category: Some("VFR".to_owned()),
                metar_raw: Some("EDDN 110920Z 26010KT 9999 SCT042 17/08 Q1020 NOSIG".to_owned()),
                metar_decoded: None,
                taf_raw: None,
                taf_decoded: None,
            },
            AerodromeWeather {
                icao: "EDQA".to_owned(),
                name: Some("Bamberg".to_owned()),
                role: "Alternate".to_owned(),
                flight_category: None,
                metar_raw: None,
                metar_decoded: None,
                taf_raw: None,
                taf_decoded: None,
            },
        ],
        winds_aloft: vec![
            WindsAloftRow {
                leg: "EDMA → DON".to_owned(),
                altitude: "5500 ft AMSL".to_owned(),
                direction_deg: 243.0,
                speed_kt: 16.0,
                temperature_c: Some(2.4),
            },
            WindsAloftRow {
                leg: "DON → TRU".to_owned(),
                altitude: "5500 ft AMSL".to_owned(),
                direction_deg: 247.0,
                speed_kt: 17.0,
                temperature_c: Some(2.1),
            },
            WindsAloftRow {
                leg: "TRU → ROT → EDDN".to_owned(),
                altitude: "5500 ft AMSL".to_owned(),
                direction_deg: 251.0,
                speed_kt: 14.0,
                temperature_c: Some(1.8),
            },
        ],
        freezing_level: Some("approx. 9800 ft AMSL along the route".to_owned()),
        winds_source_note: Some("ISA estimate — no forecast data".to_owned()),
    }
}

fn notams() -> NotamSection {
    NotamSection {
        snapshot_time: Some(Utc.with_ymd_and_hms(2026, 6, 11, 9, 25, 0).unwrap()),
        notams: vec![
            NotamCard {
                id: "B0612/26".to_owned(),
                location: "EDDN".to_owned(),
                relevance: Some("Destination".to_owned()),
                validity: "2026-06-08 06:00Z → 2026-06-20 18:00Z".to_owned(),
                schedule: Some("DAILY 0600-1800".to_owned()),
                limits: None,
                summary: "Taxiway D closed due to maintenance; expect taxi via C.".to_owned(),
                raw: "B0612/26 NOTAMN\nQ) EDMM/QMXLC/IV/M/A/000/999/4921N01107E005\n\
                      A) EDDN B) 2606080600 C) 2606201800\nD) DAILY 0600-1800\n\
                      E) TWY D CLSD DUE TO MAINT, USE TWY C"
                    .to_owned(),
            },
            NotamCard {
                id: "D0488/26".to_owned(),
                location: "EDMM (FIR)".to_owned(),
                relevance: Some("Route corridor, 2 NM off track".to_owned()),
                validity: "2026-06-12 07:00Z → 2026-06-12 16:00Z".to_owned(),
                schedule: None,
                limits: Some("GND → 4500 ft AMSL".to_owned()),
                summary: "Parachute jumping exercise at Treuchtlingen; intense activity \
                          up to 4500 ft."
                    .to_owned(),
                raw: "D0488/26 NOTAMN\nQ) EDMM/QWPLW/IV/M/W/000/045/4858N01055E002\n\
                      A) EDMM B) 2606120700 C) 2606121600\n\
                      E) PJE WI 2NM RADIUS OF 485800N0105500E (TREUCHTLINGEN)\n\
                      F) GND G) 4500FT AMSL"
                    .to_owned(),
            },
        ],
        source_note: Some("Built-in sample NOTAMs — not a real briefing".to_owned()),
    }
}

//! Item builders for the FPL message (Doc 4444 Appendix 2 field
//! conventions, VFR scope).

use crate::aircraft::{AircraftProfile, PowerSetting};
use crate::compute::ComputedFlight;
use crate::flight::{FlightDoc, FlightRules, NamedPointKind, RoutePoint};
use crate::units::Kilograms;

use super::format::{hhmm_duration, icao_coords, level_block, speed_block};
use super::{FplError, PilotInfo};

/// Item 7 — aircraft identification: the profile's default callsign when
/// set, otherwise the registration without hyphens (`D-EABC` files as
/// `DEABC`).
pub(crate) fn item7(aircraft: &AircraftProfile) -> Result<String, FplError> {
    let source = if aircraft.callsign.trim().is_empty() {
        &aircraft.registration
    } else {
        &aircraft.callsign
    };
    let identification: String = source
        .to_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if identification.is_empty() {
        return Err(FplError::MissingData {
            item: 7,
            what: "aircraft registration",
        });
    }
    Ok(identification)
}

/// Item 8 — flight rules + type of flight. VFR rules file as `V`; type of
/// flight is `G` (general aviation) for this milestone.
pub(crate) fn item8(doc: &FlightDoc) -> String {
    let rules = match doc.rules {
        FlightRules::Vfr => 'V',
    };
    format!("{rules}G")
}

/// Item 9 — type designator + wake turbulence category. The category is
/// derived from MTOW per ICAO bands: ≤ 7000 kg `L`, ≤ 136 000 kg `M`,
/// above `H` (an unset/zero MTOW defaults to `L` — the GA template).
pub(crate) fn item9(aircraft: &AircraftProfile) -> Result<String, FplError> {
    let designator = aircraft.type_designator.to_uppercase();
    if designator.is_empty() {
        return Err(FplError::MissingData {
            item: 9,
            what: "ICAO type designator",
        });
    }
    Ok(format!(
        "{designator}/{}",
        wake_category(aircraft.weight_balance.max_takeoff)
    ))
}

fn wake_category(max_takeoff: Kilograms) -> char {
    if max_takeoff.0 > 136_000.0 {
        'H'
    } else if max_takeoff.0 > 7_000.0 {
        'M'
    } else {
        'L'
    }
}

/// Item 10 — equipment `10a/10b` from the profile defaults.
pub(crate) fn item10(aircraft: &AircraftProfile) -> Result<String, FplError> {
    let com = aircraft.equipment.com_nav_approach.to_uppercase();
    let surveillance = aircraft.equipment.surveillance.to_uppercase();
    if com.is_empty() || surveillance.is_empty() {
        return Err(FplError::MissingData {
            item: 10,
            what: "equipment strings",
        });
    }
    Ok(format!("{com}/{surveillance}"))
}

/// An aerodrome's item 13/16 code: the ICAO indicator for named airport
/// points with a 4-letter id, `ZZZZ` otherwise (with the position carried
/// into item 18 `DEP/`/`DEST/`).
pub(crate) fn aerodrome_code(point: &RoutePoint) -> (String, Option<String>) {
    if let RoutePoint::Named(named) = point
        && named.kind == NamedPointKind::Airport
    {
        let id = named.id.to_uppercase();
        if id.len() == 4 && id.chars().all(|c| c.is_ascii_uppercase()) {
            return (id, None);
        }
    }
    ("ZZZZ".to_owned(), Some(icao_coords(point.position())))
}

/// Item 13 — departure aerodrome + time. Returns the item and the
/// optional `DEP/` value for item 18.
pub(crate) fn item13(doc: &FlightDoc) -> Result<(String, Option<String>), FplError> {
    let departure = doc.route.first().ok_or(FplError::MissingData {
        item: 13,
        what: "a departure waypoint",
    })?;
    let time = doc.departure_time.ok_or(FplError::MissingData {
        item: 13,
        what: "a departure time",
    })?;
    let (code, dep_value) = aerodrome_code(&departure.point);
    Ok((format!("{code}{}", time.format("%H%M")), dep_value))
}

/// The planned cruise power setting (`None` = the profile's first).
pub(crate) fn cruise_setting<'a>(
    doc: &FlightDoc,
    aircraft: &'a AircraftProfile,
    item: u8,
) -> Result<&'a PowerSetting, FplError> {
    let settings = &aircraft.performance.cruise_settings;
    match doc.power_setting.as_deref() {
        Some(name) => settings.iter().find(|s| s.name == name),
        None => settings.first(),
    }
    .ok_or(FplError::MissingData {
        item,
        what: "a cruise power setting",
    })
}

/// Item 15 — cruising speed, level, route. The route string is the
/// DCT-joined intermediate waypoints: named points by ident where it fits
/// the 2–5 alphanumeric element format, everything else as ICAO
/// degrees-minutes coordinates; departure and destination stay in items
/// 13/16.
pub(crate) fn item15(doc: &FlightDoc, aircraft: &AircraftProfile) -> Result<String, FplError> {
    let setting = cruise_setting(doc, aircraft, 15)?;
    if setting.tas.0 <= 0.0 {
        return Err(FplError::MissingData {
            item: 15,
            what: "a positive cruise TAS",
        });
    }
    let mut item = format!("{}{}", speed_block(setting.tas), level_block(doc.cruise_altitude));
    let intermediates = if doc.route.len() > 2 {
        &doc.route[1..doc.route.len() - 1]
    } else {
        &[]
    };
    for waypoint in intermediates {
        item.push_str(" DCT ");
        item.push_str(&route_element(&waypoint.point));
    }
    item.push_str(" DCT");
    Ok(item)
}

fn route_element(point: &RoutePoint) -> String {
    if let Some(id) = point.ident() {
        let id = id.to_uppercase();
        if (2..=5).contains(&id.len())
            && id.chars().all(|c| c.is_ascii_alphanumeric())
            && id.chars().any(|c| c.is_ascii_uppercase())
        {
            return id;
        }
    }
    icao_coords(point.position())
}

/// Item 16 — destination + total EET + up to two alternates. Returns the
/// item plus optional `DEST/` and `ALTN/` values for item 18.
pub(crate) fn item16(
    doc: &FlightDoc,
    computed: &ComputedFlight,
) -> Result<(String, Option<String>, Option<String>), FplError> {
    if doc.route.len() < 2 {
        return Err(FplError::MissingData {
            item: 16,
            what: "a destination waypoint",
        });
    }
    let destination = &doc.route[doc.route.len() - 1];
    let (code, dest_value) = aerodrome_code(&destination.point);
    let eet = computed.navlog.totals.ete;
    if eet.0 <= 0.0 {
        return Err(FplError::MissingData {
            item: 16,
            what: "a computed total EET",
        });
    }
    let mut item = format!("{code}{}", hhmm_duration(eet.0));

    let mut altn_value = None;
    let mut included = 0;
    for alternate in &doc.alternates {
        if included == 2 {
            break;
        }
        let (alt_code, alt_coords) = aerodrome_code(alternate);
        if alt_coords.is_none() {
            item.push(' ');
            item.push_str(&alt_code);
            included += 1;
        } else if altn_value.is_none() {
            // One non-ICAO alternate files as ZZZZ + ALTN/…; further
            // non-ICAO alternates are dropped (cannot be distinguished).
            item.push_str(" ZZZZ");
            altn_value = alt_coords;
            included += 1;
        }
    }
    Ok((item, dest_value, altn_value))
}

/// Item 18 — other information: `DOF/` plus any `DEP/`, `DEST/`, `ALTN/`
/// groups for `ZZZZ` aerodromes; `0` when empty.
pub(crate) fn item18(
    doc: &FlightDoc,
    dep: Option<String>,
    dest: Option<String>,
    altn: Option<String>,
) -> String {
    let mut groups: Vec<String> = Vec::new();
    if let Some(time) = doc.departure_time {
        groups.push(format!("DOF/{}", time.format("%y%m%d")));
    }
    if let Some(value) = dep {
        groups.push(format!("DEP/{value}"));
    }
    if let Some(value) = dest {
        groups.push(format!("DEST/{value}"));
    }
    if let Some(value) = altn {
        groups.push(format!("ALTN/{value}"));
    }
    if groups.is_empty() {
        "0".to_owned()
    } else {
        groups.join(" ")
    }
}

/// Item 19 — supplementary information: `E/` endurance from the fuel
/// plan (loaded fuel minus taxi at cruise flow), `P/` persons on board
/// (`TBN` when unknown), optional `A/` colour and markings, `C/` pilot in
/// command.
pub(crate) fn item19(
    doc: &FlightDoc,
    aircraft: &AircraftProfile,
    pilot: &PilotInfo,
) -> Result<String, FplError> {
    let setting = cruise_setting(doc, aircraft, 19)?;
    if setting.fuel_flow.0 <= 0.0 {
        return Err(FplError::MissingData {
            item: 19,
            what: "a positive cruise fuel flow",
        });
    }
    if doc.loading.fuel.0 <= 0.0 {
        return Err(FplError::MissingData {
            item: 19,
            what: "loaded fuel in the loading scenario",
        });
    }
    let taxi_fuel = doc.fuel_policy.taxi.as_hours() * aircraft.performance.taxi_fuel_flow.0;
    let usable = (doc.loading.fuel.0 - taxi_fuel).max(0.0);
    let endurance_minutes = usable / setting.fuel_flow.0 * 60.0;

    let persons = pilot
        .persons_on_board
        .map_or_else(|| "TBN".to_owned(), |n| n.to_string());
    let mut item = format!("E/{} P/{persons}", hhmm_duration(endurance_minutes));
    if let Some(color) = &pilot.aircraft_color {
        item.push_str(" A/");
        item.push_str(&color.to_uppercase());
    }
    let name = pilot.pilot_in_command.trim().to_uppercase();
    if name.is_empty() {
        return Err(FplError::MissingData {
            item: 19,
            what: "the pilot in command",
        });
    }
    item.push_str(" C/");
    item.push_str(&name);
    Ok(item)
}

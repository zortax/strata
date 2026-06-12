//! openAIP `/airports` payload → [`Airport`].

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::domain::{
    Airport, AirportKind, Frequency, FrequencyKind, IcaoCode, Runway, RunwaySurface,
};

use super::NormalizationReport;
use super::common::{
    RawMeasurement, distance_meters, elevation_amsl, item_id, point_position, radio_frequency,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAirport {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
    icao_code: Option<String>,
    geometry: Value,
    elevation: Option<RawMeasurement>,
    #[serde(default)]
    runways: Vec<RawRunway>,
    #[serde(default)]
    frequencies: Vec<RawFrequency>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRunway {
    designator: String,
    true_heading: Option<u16>,
    #[serde(default)]
    main_runway: bool,
    surface: Option<RawSurface>,
    dimension: Option<RawDimension>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSurface {
    main_composite: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct RawDimension {
    length: Option<RawDistance>,
    width: Option<RawDistance>,
}

#[derive(Debug, Deserialize)]
struct RawDistance {
    value: f64,
    unit: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFrequency {
    value: String,
    unit: u8,
    #[serde(rename = "type")]
    kind: Option<u16>,
    name: Option<String>,
    #[serde(default)]
    primary: bool,
}

pub(crate) fn normalize(items: &[Value]) -> (Vec<Airport>, NormalizationReport) {
    let mut airports = Vec::with_capacity(items.len());
    let mut report = NormalizationReport::new(items.len());
    for item in items {
        match normalize_one(item) {
            Ok(airport) => airports.push(airport),
            Err(reason) => report.skip(item_id(item), reason, "airport"),
        }
    }
    (airports, report)
}

/// Maps openAIP airport `_id`s to ICAO idents, for resolving the airport
/// references on reporting points. Fields without an ICAO code are absent.
pub(crate) fn icao_index(items: &[Value]) -> HashMap<String, IcaoCode> {
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("_id")?.as_str()?;
            let code = item.get("icaoCode")?.as_str()?;
            let icao = IcaoCode::new(code).ok()?;
            Some((id.to_owned(), icao))
        })
        .collect()
}

fn normalize_one(item: &Value) -> Result<Airport, String> {
    let raw: RawAirport =
        serde_json::from_value(item.clone()).map_err(|e| format!("malformed airport: {e}"))?;
    let position = point_position(&raw.geometry)?;
    let elevation = elevation_amsl(raw.elevation.ok_or("missing elevation")?)
        .map_err(|e| format!("elevation: {e}"))?;
    let ident = match &raw.icao_code {
        Some(code) => match IcaoCode::new(code) {
            Ok(icao) => Some(icao),
            Err(err) => {
                warn!(airport = %raw.name, %err, "dropping invalid ICAO code");
                None
            }
        },
        None => None,
    };
    let runways = raw.runways.into_iter().map(runway).collect();
    let frequencies = raw
        .frequencies
        .into_iter()
        .filter_map(|f| frequency(f, &raw.name))
        .collect();
    Ok(Airport {
        ident,
        name: raw.name,
        kind: airport_kind(raw.kind),
        position,
        elevation,
        runways,
        frequencies,
    })
}

fn runway(raw: RawRunway) -> Runway {
    let surface = raw
        .surface
        .and_then(|s| s.main_composite)
        .map_or(RunwaySurface::Unknown, runway_surface);
    let (length, width) = raw.dimension.map_or((None, None), |d| {
        (
            d.length
                .and_then(|l| dimension(l, "length", &raw.designator)),
            d.width.and_then(|w| dimension(w, "width", &raw.designator)),
        )
    });
    Runway {
        designator: raw.designator,
        true_heading_deg: raw.true_heading,
        length,
        width,
        surface,
        main: raw.main_runway,
    }
}

fn dimension(
    raw: RawDistance,
    what: &'static str,
    designator: &str,
) -> Option<crate::domain::Meters> {
    match distance_meters(raw.value, raw.unit) {
        Ok(meters) => Some(meters),
        Err(reason) => {
            warn!(
                runway = designator,
                what, reason, "dropping runway dimension"
            );
            None
        }
    }
}

/// A malformed frequency entry drops with a warning instead of skipping the
/// whole airport.
fn frequency(raw: RawFrequency, airport: &str) -> Option<Frequency> {
    let Some(kind) = raw.kind else {
        warn!(
            airport,
            value = raw.value,
            "dropping frequency without type"
        );
        return None;
    };
    match radio_frequency(&raw.value, raw.unit) {
        Ok(frequency) => Some(Frequency {
            frequency,
            name: raw.name.unwrap_or_default(),
            kind: frequency_kind(kind),
            primary: raw.primary,
        }),
        Err(reason) => {
            warn!(airport, reason, "dropping malformed frequency");
            None
        }
    }
}

/// openAIP airport `type` codes (verified against the airport schema):
/// 0 Airport (civil/military), 1 Glider Site, 2 Airfield Civil,
/// 3 International Airport, 4 Heliport Military, 5 Military Aerodrome,
/// 6 Ultra Light Flying Site, 7 Heliport Civil, 8 Aerodrome Closed,
/// 9 Airport resp. Airfield IFR, 10 Airfield Water, 11 Landing Strip,
/// 12 Agricultural Landing Strip, 13 Altiport.
fn airport_kind(code: u16) -> AirportKind {
    match code {
        0 | 9 => AirportKind::Regional,
        1 => AirportKind::GliderSite,
        2 => AirportKind::Airfield,
        3 => AirportKind::International,
        4 | 7 => AirportKind::Heliport,
        5 => AirportKind::MilitaryAerodrome,
        6 => AirportKind::UltraLightSite,
        8 => AirportKind::Closed,
        10 => AirportKind::WaterAirfield,
        11 | 12 => AirportKind::LandingStrip,
        other => AirportKind::Other(other),
    }
}

/// openAIP frequency `type` codes 0..=22. Codes without a dedicated domain
/// variant (17 Other, 18 AIRMET, 20 Lights) carry the raw code.
fn frequency_kind(code: u16) -> FrequencyKind {
    match code {
        0 => FrequencyKind::Approach,
        1 => FrequencyKind::Apron,
        2 => FrequencyKind::Arrival,
        3 => FrequencyKind::Center,
        4 => FrequencyKind::Ctaf,
        5 => FrequencyKind::Delivery,
        6 => FrequencyKind::Departure,
        7 => FrequencyKind::Fis,
        8 => FrequencyKind::Gliding,
        9 => FrequencyKind::Ground,
        10 => FrequencyKind::Information,
        11 => FrequencyKind::Multicom,
        12 => FrequencyKind::Unicom,
        13 => FrequencyKind::Radar,
        14 => FrequencyKind::Tower,
        15 => FrequencyKind::Atis,
        16 => FrequencyKind::Radio,
        19 => FrequencyKind::Awos,
        21 => FrequencyKind::Volmet,
        22 => FrequencyKind::Afis,
        other => FrequencyKind::Other(other),
    }
}

/// openAIP runway surface `mainComposite` codes 0..=22 (22 = Unknown).
fn runway_surface(code: u16) -> RunwaySurface {
    match code {
        0 => RunwaySurface::Asphalt,
        1 => RunwaySurface::Concrete,
        2 => RunwaySurface::Grass,
        3 => RunwaySurface::Sand,
        4 => RunwaySurface::Water,
        12 => RunwaySurface::Gravel,
        13 => RunwaySurface::Earth,
        14 => RunwaySurface::Ice,
        15 => RunwaySurface::Snow,
        22 => RunwaySurface::Unknown,
        other => RunwaySurface::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Meters, MetersAmsl, RadioFrequency};

    fn fixture_items() -> Vec<Value> {
        super::super::fixture_items(include_str!(
            "../../../tests/fixtures/openaip/airports.json"
        ))
    }

    #[test]
    fn every_fixture_airport_normalizes() {
        let items = fixture_items();
        let (airports, report) = normalize(&items);
        assert_eq!(report.total, 30);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(airports.len(), 30);
    }

    #[test]
    fn aachen_heliport_spot_check() {
        let (airports, _) = normalize(&fixture_items());
        let aachen = &airports[0];
        assert_eq!(aachen.name, "AACHEN");
        // openAIP type 7 = Heliport Civil.
        assert_eq!(aachen.kind, AirportKind::Heliport);
        assert_eq!(aachen.ident, None);
        assert!((aachen.position.lat() - 50.775_666_666_666_666).abs() < 1e-12);
        assert!((aachen.position.lon() - 6.044_388_888_888_889).abs() < 1e-12);
        assert_eq!(aachen.elevation, MetersAmsl(207.0));
    }

    #[test]
    fn merzbrueck_runways_and_frequencies() {
        let (airports, _) = normalize(&fixture_items());
        let edka = airports
            .iter()
            .find(|a| a.ident.as_ref().is_some_and(|i| i.as_str() == "EDKA"))
            .expect("EDKA in fixture");
        assert_eq!(edka.name, "AACHEN-MERZBRUECK");
        assert_eq!(edka.kind, AirportKind::Airfield);

        let rwy07 = edka
            .runways
            .iter()
            .find(|r| r.designator == "07")
            .expect("runway 07");
        assert_eq!(rwy07.true_heading_deg, Some(65));
        assert_eq!(rwy07.length, Some(Meters(1160.0)));
        assert_eq!(rwy07.width, Some(Meters(18.0)));
        assert_eq!(rwy07.surface, RunwaySurface::Asphalt);
        assert!(rwy07.main);

        let radio = &edka.frequencies[0];
        assert_eq!(radio.frequency, RadioFrequency::from_mhz(122.88));
        assert_eq!(radio.kind, FrequencyKind::Radio);
        assert_eq!(radio.name, "AACHEN RADIO");
        assert!(radio.primary);
    }

    #[test]
    fn fixture_covers_known_airport_kinds() {
        let (airports, _) = normalize(&fixture_items());
        for kind in [
            AirportKind::GliderSite,
            AirportKind::Airfield,
            AirportKind::UltraLightSite,
            AirportKind::Heliport,
            AirportKind::Closed,
            AirportKind::Regional, // type 9: airfield with IFR
        ] {
            assert!(
                airports.iter().any(|a| a.kind == kind),
                "no airport of kind {kind:?} normalized"
            );
        }
    }

    #[test]
    fn icao_index_only_contains_coded_airports() {
        let items = fixture_items();
        let index = icao_index(&items);
        // 11 of the 30 fixture airports carry an ICAO code.
        assert_eq!(index.len(), 11);
        assert_eq!(
            index.get("62614a351eacded7b7bbdc9c").map(IcaoCode::as_str),
            Some("EDKA")
        );
    }

    #[test]
    fn unknown_airport_type_maps_to_other() {
        assert_eq!(airport_kind(13), AirportKind::Other(13));
        assert_eq!(airport_kind(99), AirportKind::Other(99));
    }
}

//! openAIP `/airspaces` payload → [`Airspace`].
//!
//! Vertical limits are the classic-bug area: every limit is a
//! `{ value, unit, referenceDatum }` triple (unit 0 = m, 1 = ft, 6 = FL;
//! datum 0 = GND, 1 = MSL, 2 = STD) and is mapped exhaustively in
//! [`super::common::vertical_limit`]. Unknown encodings skip the item.

use serde::Deserialize;
use serde_json::Value;

use crate::domain::{AiracCycle, Airspace, AirspaceClass, AirspaceKind};

use super::NormalizationReport;
use super::common::{RawMeasurement, item_id, polygon_geometry, vertical_limit};

/// openAIP airspace `activity` code for parachuting.
const ACTIVITY_PARACHUTING: u16 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAirspace {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
    icao_class: Option<u16>,
    activity: Option<u16>,
    geometry: Value,
    upper_limit: Option<RawMeasurement>,
    lower_limit: Option<RawMeasurement>,
}

pub(crate) fn normalize(
    items: &[Value],
    airac: Option<&AiracCycle>,
) -> (Vec<Airspace>, NormalizationReport) {
    let mut airspaces = Vec::with_capacity(items.len());
    let mut report = NormalizationReport::new(items.len());
    for item in items {
        match normalize_one(item, airac) {
            Ok(airspace) => airspaces.push(airspace),
            Err(reason) => report.skip(item_id(item), reason, "airspace"),
        }
    }
    (airspaces, report)
}

fn normalize_one(item: &Value, airac: Option<&AiracCycle>) -> Result<Airspace, String> {
    let raw: RawAirspace =
        serde_json::from_value(item.clone()).map_err(|e| format!("malformed airspace: {e}"))?;
    let class = airspace_class(raw.icao_class.ok_or("missing icaoClass")?)?;
    let lower = vertical_limit(raw.lower_limit.ok_or("missing lowerLimit")?)
        .map_err(|e| format!("lowerLimit: {e}"))?;
    let upper = vertical_limit(raw.upper_limit.ok_or("missing upperLimit")?)
        .map_err(|e| format!("upperLimit: {e}"))?;
    let geometry = polygon_geometry(&raw.geometry)?;
    Ok(Airspace {
        name: raw.name,
        class,
        kind: airspace_kind(raw.kind, raw.activity),
        lower,
        upper,
        geometry,
        airac: airac.cloned(),
    })
}

/// openAIP `icaoClass` codes: 0..=6 are classes A..G, 8 is
/// "Unclassified / Special Use Airspace". 7 is unassigned.
fn airspace_class(code: u16) -> Result<AirspaceClass, String> {
    match code {
        0 => Ok(AirspaceClass::A),
        1 => Ok(AirspaceClass::B),
        2 => Ok(AirspaceClass::C),
        3 => Ok(AirspaceClass::D),
        4 => Ok(AirspaceClass::E),
        5 => Ok(AirspaceClass::F),
        6 => Ok(AirspaceClass::G),
        8 => Ok(AirspaceClass::Unclassified),
        other => Err(format!("unknown icaoClass code {other}")),
    }
}

/// openAIP airspace `type` codes, mapped exhaustively per the Core API
/// OpenAPI schema (`/airspaces` `type` parameter, 0 "Other" .. 36 "Military
/// Controlled Tower Region (MCTR)"). Parachute jump areas have no dedicated
/// type; openAIP publishes them as type 28 "Aerial Sporting Or Recreational
/// Activity" (or the catch-all type 0) with activity 1 "Parachuting".
fn airspace_kind(type_code: u16, activity: Option<u16>) -> AirspaceKind {
    if matches!(type_code, 0 | 28) && activity == Some(ACTIVITY_PARACHUTING) {
        return AirspaceKind::ParachuteJumpArea;
    }
    match type_code {
        0 => AirspaceKind::Area,
        1 => AirspaceKind::Restricted,
        2 => AirspaceKind::Danger,
        3 => AirspaceKind::Prohibited,
        4 => AirspaceKind::Ctr,
        5 => AirspaceKind::Tmz,
        6 => AirspaceKind::Rmz,
        7 => AirspaceKind::Tma,
        8 => AirspaceKind::Tra,
        9 => AirspaceKind::Tsa,
        10 => AirspaceKind::Fir,
        11 => AirspaceKind::Uir,
        12 => AirspaceKind::Adiz,
        13 => AirspaceKind::Atz,
        14 => AirspaceKind::Matz,
        15 => AirspaceKind::Airway,
        16 => AirspaceKind::MilitaryTrainingRoute,
        17 => AirspaceKind::AlertArea,
        18 => AirspaceKind::WarningArea,
        19 => AirspaceKind::ProtectedArea,
        20 => AirspaceKind::Htz,
        21 => AirspaceKind::GliderSector,
        22 => AirspaceKind::TransponderSetting,
        23 => AirspaceKind::Tiz,
        24 => AirspaceKind::Tia,
        25 => AirspaceKind::MilitaryTrainingArea,
        26 => AirspaceKind::Cta,
        27 => AirspaceKind::AccSector,
        28 => AirspaceKind::RecreationalActivity,
        29 => AirspaceKind::OverflightRestriction,
        30 => AirspaceKind::MilitaryRoute,
        31 => AirspaceKind::TsaTraFeedingRoute,
        32 => AirspaceKind::VfrSector,
        33 => AirspaceKind::FisSector,
        34 => AirspaceKind::LowerTrafficArea,
        35 => AirspaceKind::UpperTrafficArea,
        36 => AirspaceKind::Mctr,
        other => AirspaceKind::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MetersAgl, MetersAmsl, VerticalReference};

    fn fixture_items() -> Vec<Value> {
        super::super::fixture_items(include_str!(
            "../../../tests/fixtures/openaip/airspaces.json"
        ))
    }

    #[test]
    fn every_fixture_airspace_normalizes() {
        let items = fixture_items();
        let (airspaces, report) = normalize(&items, None);
        assert_eq!(report.total, 30);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(airspaces.len(), 30);
        assert!(airspaces.iter().all(|a| a.airac.is_none()));
    }

    #[test]
    fn glider_sector_with_fl_upper_limit() {
        let (airspaces, _) = normalize(&fixture_items(), None);
        let alb = &airspaces[0];
        assert_eq!(alb.name, "ALB-NORD_UGR. 4.5");
        assert_eq!(alb.kind, AirspaceKind::GliderSector);
        assert_eq!(alb.class, AirspaceClass::Unclassified);
        assert_eq!(alb.upper.reference, VerticalReference::Fl(100));
        assert_eq!(
            alb.lower.reference,
            VerticalReference::Amsl(MetersAmsl::from_feet(4500.0))
        );
        assert_eq!(alb.vertical_band(), "FL 100 / 4500 ft MSL");

        // GeoJSON is [lon, lat]; the domain polygon must come out lat/lon.
        let first = alb.geometry.exterior()[0];
        assert!((first.lat() - 48.49111).abs() < 1e-9);
        assert!((first.lon() - 9.19111).abs() < 1e-9);
    }

    #[test]
    fn fis_sector_spans_gnd_to_fl() {
        let (airspaces, _) = normalize(&fixture_items(), None);
        let fis = airspaces
            .iter()
            .find(|a| a.name == "ALPINE AREA FIS LANGEN")
            .expect("FIS sector in fixture");
        assert_eq!(fis.kind, AirspaceKind::FisSector);
        assert_eq!(fis.lower.reference, VerticalReference::Gnd);
        assert_eq!(fis.upper.reference, VerticalReference::Fl(130));
    }

    #[test]
    fn class_c_tma_with_msl_lower_limit() {
        let (airspaces, _) = normalize(&fixture_items(), None);
        let berlin = airspaces
            .iter()
            .find(|a| a.name == "BERLIN" && a.class == AirspaceClass::C)
            .expect("class C BERLIN TMA in fixture");
        assert_eq!(berlin.kind, AirspaceKind::Tma);
        assert_eq!(berlin.upper.reference, VerticalReference::Fl(100));
        assert_eq!(
            berlin.lower.reference,
            VerticalReference::Amsl(MetersAmsl::from_feet(2500.0))
        );
    }

    #[test]
    fn class_e_area_with_agl_band() {
        let (airspaces, _) = normalize(&fixture_items(), None);
        let altenburg = airspaces
            .iter()
            .find(|a| a.name == "ALTENBURG")
            .expect("ALTENBURG in fixture");
        assert_eq!(altenburg.class, AirspaceClass::E);
        assert_eq!(
            altenburg.lower.reference,
            VerticalReference::Agl(MetersAgl::from_feet(1000.0))
        );
        assert_eq!(
            altenburg.upper.reference,
            VerticalReference::Agl(MetersAgl::from_feet(2500.0))
        );
    }

    #[test]
    fn airac_cycle_is_stamped_when_given() {
        let cycle = AiracCycle::current();
        let (airspaces, _) = normalize(&fixture_items(), Some(&cycle));
        assert!(airspaces.iter().all(|a| a.airac.as_ref() == Some(&cycle)));
    }

    #[test]
    fn kind_mapping_covers_every_openaip_type_code() {
        // Full openAIP airspace `type` enumeration (Core API schema, 0..=36).
        let expected = [
            (0, AirspaceKind::Area),
            (1, AirspaceKind::Restricted),
            (2, AirspaceKind::Danger),
            (3, AirspaceKind::Prohibited),
            (4, AirspaceKind::Ctr),
            (5, AirspaceKind::Tmz),
            (6, AirspaceKind::Rmz),
            (7, AirspaceKind::Tma),
            (8, AirspaceKind::Tra),
            (9, AirspaceKind::Tsa),
            (10, AirspaceKind::Fir),
            (11, AirspaceKind::Uir),
            (12, AirspaceKind::Adiz),
            (13, AirspaceKind::Atz),
            (14, AirspaceKind::Matz),
            (15, AirspaceKind::Airway),
            (16, AirspaceKind::MilitaryTrainingRoute),
            (17, AirspaceKind::AlertArea),
            (18, AirspaceKind::WarningArea),
            (19, AirspaceKind::ProtectedArea),
            (20, AirspaceKind::Htz),
            (21, AirspaceKind::GliderSector),
            (22, AirspaceKind::TransponderSetting),
            (23, AirspaceKind::Tiz),
            (24, AirspaceKind::Tia),
            (25, AirspaceKind::MilitaryTrainingArea),
            (26, AirspaceKind::Cta),
            (27, AirspaceKind::AccSector),
            (28, AirspaceKind::RecreationalActivity),
            (29, AirspaceKind::OverflightRestriction),
            (30, AirspaceKind::MilitaryRoute),
            (31, AirspaceKind::TsaTraFeedingRoute),
            (32, AirspaceKind::VfrSector),
            (33, AirspaceKind::FisSector),
            (34, AirspaceKind::LowerTrafficArea),
            (35, AirspaceKind::UpperTrafficArea),
            (36, AirspaceKind::Mctr),
        ];
        for (code, kind) in expected {
            assert_eq!(airspace_kind(code, None), kind, "type code {code}");
            assert!(
                !matches!(airspace_kind(code, None), AirspaceKind::Other(_)),
                "documented type code {code} must not map to Other"
            );
        }
        // Only codes beyond the documented enumeration stay raw.
        assert_eq!(airspace_kind(37, None), AirspaceKind::Other(37));
        assert_eq!(airspace_kind(99, None), AirspaceKind::Other(99));
    }

    #[test]
    fn parachuting_activity_overrides_generic_kinds() {
        assert_eq!(airspace_kind(28, Some(1)), AirspaceKind::ParachuteJumpArea);
        assert_eq!(airspace_kind(0, Some(1)), AirspaceKind::ParachuteJumpArea);
        // Other activities keep the type's own kind.
        assert_eq!(
            airspace_kind(28, Some(2)),
            AirspaceKind::RecreationalActivity
        );
        assert_eq!(airspace_kind(0, Some(2)), AirspaceKind::Area);
        // Parachuting inside a specific kind does not demote it.
        assert_eq!(airspace_kind(2, Some(1)), AirspaceKind::Danger);
    }

    #[test]
    fn every_fixture_type_code_maps_to_a_known_kind() {
        let items = fixture_items();
        // The fixture exercises exactly these openAIP type codes.
        let codes: std::collections::BTreeSet<u64> = items
            .iter()
            .map(|i| i["type"].as_u64().expect("type code"))
            .collect();
        assert_eq!(codes, [0, 7, 13, 21, 33].into_iter().collect());

        let (airspaces, _) = normalize(&items, None);
        assert!(
            airspaces
                .iter()
                .all(|a| !matches!(a.kind, AirspaceKind::Other(_))),
            "all fixture type codes have dedicated kinds"
        );
    }

    #[test]
    fn atz_and_generic_area_kinds_from_fixture() {
        let (airspaces, _) = normalize(&fixture_items(), None);
        let atz = airspaces
            .iter()
            .find(|a| a.name == "ATZ ESSEN-MUELHEIM")
            .expect("ATZ in fixture");
        assert_eq!(atz.kind, AirspaceKind::Atz);
        assert_eq!(atz.class, AirspaceClass::Unclassified);

        // Type 0 "Other" with a plain ICAO class is a generic Area, not Other.
        let altenburg = airspaces
            .iter()
            .find(|a| a.name == "ALTENBURG")
            .expect("ALTENBURG in fixture");
        assert_eq!(altenburg.kind, AirspaceKind::Area);
        assert_eq!(altenburg.class, AirspaceClass::E);
    }

    #[test]
    fn unknown_icao_class_is_an_error() {
        assert!(airspace_class(7).is_err());
        assert!(airspace_class(42).is_err());
    }
}

//! openAIP `/navaids` payload → [`Navaid`].

use serde::Deserialize;
use serde_json::Value;

use crate::domain::{Navaid, NavaidKind};

use super::NormalizationReport;
use super::common::{RawMeasurement, elevation_amsl, item_id, point_position, radio_frequency};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNavaid {
    name: String,
    identifier: String,
    #[serde(rename = "type")]
    kind: u16,
    frequency: Option<RawFrequency>,
    channel: Option<String>,
    geometry: Value,
    elevation: Option<RawMeasurement>,
}

#[derive(Debug, Deserialize)]
struct RawFrequency {
    value: String,
    unit: u8,
}

pub(crate) fn normalize(items: &[Value]) -> (Vec<Navaid>, NormalizationReport) {
    let mut navaids = Vec::with_capacity(items.len());
    let mut report = NormalizationReport::new(items.len());
    for item in items {
        match normalize_one(item) {
            Ok(navaid) => navaids.push(navaid),
            Err(reason) => report.skip(item_id(item), reason, "navaid"),
        }
    }
    (navaids, report)
}

fn normalize_one(item: &Value) -> Result<Navaid, String> {
    let raw: RawNavaid =
        serde_json::from_value(item.clone()).map_err(|e| format!("malformed navaid: {e}"))?;
    let position = point_position(&raw.geometry)?;
    let elevation = elevation_amsl(raw.elevation.ok_or("missing elevation")?)
        .map_err(|e| format!("elevation: {e}"))?;
    let frequency = raw
        .frequency
        .map(|f| radio_frequency(&f.value, f.unit))
        .transpose()
        .map_err(|e| format!("frequency: {e}"))?;
    Ok(Navaid {
        ident: raw.identifier,
        name: raw.name,
        kind: navaid_kind(raw.kind)?,
        frequency,
        channel: raw.channel,
        position,
        elevation,
    })
}

/// openAIP navaid `type` codes 0..=8, which match [`NavaidKind`] exactly.
/// [`NavaidKind`] has no catch-all variant, so unknown codes skip the item.
fn navaid_kind(code: u16) -> Result<NavaidKind, String> {
    match code {
        0 => Ok(NavaidKind::Dme),
        1 => Ok(NavaidKind::Tacan),
        2 => Ok(NavaidKind::Ndb),
        3 => Ok(NavaidKind::Vor),
        4 => Ok(NavaidKind::VorDme),
        5 => Ok(NavaidKind::Vortac),
        6 => Ok(NavaidKind::Dvor),
        7 => Ok(NavaidKind::DvorDme),
        8 => Ok(NavaidKind::Dvortac),
        other => Err(format!("unknown navaid type code {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MetersAmsl, RadioFrequency};

    fn fixture_items() -> Vec<Value> {
        super::super::fixture_items(include_str!(
            "../../../tests/fixtures/openaip/navaids.json"
        ))
    }

    #[test]
    fn every_fixture_navaid_normalizes() {
        let items = fixture_items();
        let (navaids, report) = normalize(&items);
        assert_eq!(report.total, 30);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(navaids.len(), 30);
    }

    #[test]
    fn allgaeu_ndb_uses_khz() {
        let (navaids, _) = normalize(&fixture_items());
        let alg = &navaids[0];
        assert_eq!(alg.ident, "ALG");
        assert_eq!(alg.name, "ALLGAEU");
        assert_eq!(alg.kind, NavaidKind::Ndb);
        // NDB frequencies carry openAIP unit 1 (kHz) — not MHz.
        assert_eq!(alg.frequency, Some(RadioFrequency::from_khz(341.0)));
        assert_eq!(alg.frequency.map(|f| f.to_string()).as_deref(), Some("341 kHz"));
        assert_eq!(alg.channel, None);
        assert_eq!(alg.elevation, MetersAmsl(617.0));
        assert!((alg.position.lat() - 47.997_222_222_222).abs() < 1e-9);
        assert!((alg.position.lon() - 10.262_222_222_222).abs() < 1e-9);
    }

    #[test]
    fn barmen_dvor_dme_with_channel() {
        let (navaids, _) = normalize(&fixture_items());
        let bam = navaids
            .iter()
            .find(|n| n.ident == "BAM")
            .expect("BAM in fixture");
        assert_eq!(bam.kind, NavaidKind::DvorDme);
        assert_eq!(bam.frequency, Some(RadioFrequency::from_mhz(114.0)));
        assert_eq!(bam.channel.as_deref(), Some("87X"));
    }

    #[test]
    fn fixture_covers_known_navaid_kinds() {
        let (navaids, _) = normalize(&fixture_items());
        for kind in [
            NavaidKind::Dme,
            NavaidKind::Tacan,
            NavaidKind::Ndb,
            NavaidKind::VorDme,
            NavaidKind::DvorDme,
        ] {
            assert!(
                navaids.iter().any(|n| n.kind == kind),
                "no navaid of kind {kind:?} normalized"
            );
        }
    }

    #[test]
    fn unknown_navaid_type_is_an_error() {
        assert!(navaid_kind(9).is_err());
    }
}

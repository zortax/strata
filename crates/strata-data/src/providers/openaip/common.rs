//! Shared pieces of the raw openAIP JSON payload: measurement objects
//! (`{ value, unit, referenceDatum }`), GeoJSON geometry, frequency strings.
//!
//! Code values are verified against the official response schemas at
//! `https://api.core.openaip.net/api/schemas/response/<type>/<type>-schema.json`.

use serde::Deserialize;
use serde_json::Value;

use crate::domain::{
    LatLon, Meters, MetersAgl, MetersAmsl, Polygon, RadioFrequency, VerticalLimit,
    VerticalReference,
};

pub(crate) const PROVIDER: &str = "openaip";

/// openAIP measurement `unit` codes (vertical limits, elevations, heights,
/// runway dimensions).
pub(crate) const UNIT_METER: u8 = 0;
pub(crate) const UNIT_FEET: u8 = 1;
pub(crate) const UNIT_FLIGHT_LEVEL: u8 = 6;

/// openAIP `referenceDatum` codes.
pub(crate) const DATUM_GND: u8 = 0;
pub(crate) const DATUM_MSL: u8 = 1;
pub(crate) const DATUM_STD: u8 = 2;

/// openAIP frequency `unit` codes.
pub(crate) const FREQ_UNIT_KHZ: u8 = 1;
pub(crate) const FREQ_UNIT_MHZ: u8 = 2;

/// A raw `{ value, unit, referenceDatum }` measurement object.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMeasurement {
    pub value: f64,
    pub unit: u8,
    /// Absent on some measurement objects (e.g. runway dimensions).
    pub reference_datum: Option<u8>,
}

/// Maps an airspace vertical limit to a domain [`VerticalLimit`].
///
/// Exhaustive over the encodings openAIP publishes; anything else is an
/// error so the caller can skip (and count) the item instead of guessing.
pub(crate) fn vertical_limit(raw: RawMeasurement) -> Result<VerticalLimit, String> {
    let datum = raw
        .reference_datum
        .ok_or_else(|| "missing referenceDatum".to_owned())?;
    let reference = match (raw.unit, datum) {
        (UNIT_FLIGHT_LEVEL, DATUM_STD) => VerticalReference::Fl(flight_level(raw.value)?),
        (UNIT_FEET, DATUM_MSL) => VerticalReference::Amsl(MetersAmsl::from_feet(raw.value)),
        (UNIT_METER, DATUM_MSL) => VerticalReference::Amsl(MetersAmsl(raw.value)),
        (UNIT_FEET | UNIT_METER, DATUM_GND) if raw.value == 0.0 => VerticalReference::Gnd,
        (UNIT_FEET, DATUM_GND) => VerticalReference::Agl(MetersAgl::from_feet(raw.value)),
        (UNIT_METER, DATUM_GND) => VerticalReference::Agl(MetersAgl(raw.value)),
        (unit, datum) => {
            return Err(format!(
                "unsupported vertical limit encoding: unit={unit} referenceDatum={datum}"
            ));
        }
    };
    Ok(reference.into())
}

fn flight_level(value: f64) -> Result<u16, String> {
    if value.fract() == 0.0 && (0.0..=f64::from(u16::MAX)).contains(&value) {
        Ok(value as u16)
    } else {
        Err(format!("flight level value {value} is not a valid level"))
    }
}

/// An elevation above mean sea level (airport/navaid/obstacle `elevation`).
pub(crate) fn elevation_amsl(raw: RawMeasurement) -> Result<MetersAmsl, String> {
    if raw.reference_datum.is_some_and(|d| d != DATUM_MSL) {
        return Err(format!(
            "elevation referenceDatum {:?} is not MSL",
            raw.reference_datum
        ));
    }
    match raw.unit {
        UNIT_METER => Ok(MetersAmsl(raw.value)),
        UNIT_FEET => Ok(MetersAmsl::from_feet(raw.value)),
        other => Err(format!("unknown elevation unit code {other}")),
    }
}

/// A height above ground (obstacle `height`).
pub(crate) fn height_agl(raw: RawMeasurement) -> Result<MetersAgl, String> {
    if raw.reference_datum.is_some_and(|d| d != DATUM_GND) {
        return Err(format!(
            "height referenceDatum {:?} is not GND",
            raw.reference_datum
        ));
    }
    match raw.unit {
        UNIT_METER => Ok(MetersAgl(raw.value)),
        UNIT_FEET => Ok(MetersAgl::from_feet(raw.value)),
        other => Err(format!("unknown height unit code {other}")),
    }
}

/// A horizontal distance (runway length/width).
pub(crate) fn distance_meters(value: f64, unit: u8) -> Result<Meters, String> {
    match unit {
        UNIT_METER => Ok(Meters(value)),
        UNIT_FEET => Ok(Meters(value / crate::domain::FEET_PER_METER)),
        other => Err(format!("unknown distance unit code {other}")),
    }
}

/// Parses a frequency string (`"122.880"`) with its openAIP unit code.
pub(crate) fn radio_frequency(value: &str, unit: u8) -> Result<RadioFrequency, String> {
    let parsed: f64 = value
        .trim()
        .parse()
        .map_err(|_| format!("unparseable frequency value {value:?}"))?;
    match unit {
        FREQ_UNIT_KHZ => Ok(RadioFrequency::from_khz(parsed)),
        FREQ_UNIT_MHZ => Ok(RadioFrequency::from_mhz(parsed)),
        other => Err(format!("unknown frequency unit code {other}")),
    }
}

/// The mongo `_id` of a raw item, for skip reporting.
pub(crate) fn item_id(item: &Value) -> &str {
    item.get("_id")
        .and_then(Value::as_str)
        .unwrap_or("<missing _id>")
}

/// Extracts a position from a GeoJSON `Point` geometry (`[lon, lat]`).
pub(crate) fn point_position(geometry: &Value) -> Result<LatLon, String> {
    let kind = geometry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "geometry missing type".to_owned())?;
    if kind != "Point" {
        return Err(format!("expected Point geometry, got {kind}"));
    }
    let coordinates = geometry
        .get("coordinates")
        .and_then(Value::as_array)
        .ok_or_else(|| "geometry missing coordinates".to_owned())?;
    lat_lon(coordinates)
}

/// Extracts a polygon from a GeoJSON `Polygon` geometry. The first ring is
/// the exterior, any further rings are holes (RFC 7946).
pub(crate) fn polygon_geometry(geometry: &Value) -> Result<Polygon, String> {
    let kind = geometry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "geometry missing type".to_owned())?;
    if kind != "Polygon" {
        return Err(format!("expected Polygon geometry, got {kind}"));
    }
    let rings = geometry
        .get("coordinates")
        .and_then(Value::as_array)
        .ok_or_else(|| "geometry missing coordinates".to_owned())?;
    let mut rings = rings
        .iter()
        .map(ring_points)
        .collect::<Result<Vec<_>, _>>()?;
    if rings.is_empty() {
        return Err("polygon has no rings".to_owned());
    }
    let exterior = rings.remove(0);
    Polygon::new(exterior, rings).map_err(|e| e.to_string())
}

fn ring_points(ring: &Value) -> Result<Vec<LatLon>, String> {
    ring.as_array()
        .ok_or_else(|| "polygon ring is not an array".to_owned())?
        .iter()
        .map(|position| {
            let coordinates = position
                .as_array()
                .ok_or_else(|| "polygon position is not an array".to_owned())?;
            lat_lon(coordinates)
        })
        .collect()
}

fn lat_lon(coordinates: &[Value]) -> Result<LatLon, String> {
    let lon = coordinates
        .first()
        .and_then(Value::as_f64)
        .ok_or_else(|| "position missing longitude".to_owned())?;
    let lat = coordinates
        .get(1)
        .and_then(Value::as_f64)
        .ok_or_else(|| "position missing latitude".to_owned())?;
    LatLon::new(lat, lon).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(value: f64, unit: u8, datum: Option<u8>) -> RawMeasurement {
        RawMeasurement {
            value,
            unit,
            reference_datum: datum,
        }
    }

    #[test]
    fn vertical_limit_flight_level() {
        let limit = vertical_limit(raw(100.0, UNIT_FLIGHT_LEVEL, Some(DATUM_STD))).unwrap();
        assert_eq!(limit.reference, VerticalReference::Fl(100));
    }

    #[test]
    fn vertical_limit_feet_msl() {
        let limit = vertical_limit(raw(4500.0, UNIT_FEET, Some(DATUM_MSL))).unwrap();
        assert_eq!(
            limit.reference,
            VerticalReference::Amsl(MetersAmsl::from_feet(4500.0))
        );
    }

    #[test]
    fn vertical_limit_meters_msl() {
        let limit = vertical_limit(raw(1500.0, UNIT_METER, Some(DATUM_MSL))).unwrap();
        assert_eq!(limit.reference, VerticalReference::Amsl(MetersAmsl(1500.0)));
    }

    #[test]
    fn vertical_limit_zero_gnd_is_ground() {
        let limit = vertical_limit(raw(0.0, UNIT_FEET, Some(DATUM_GND))).unwrap();
        assert_eq!(limit.reference, VerticalReference::Gnd);
    }

    #[test]
    fn vertical_limit_nonzero_gnd_is_agl() {
        let limit = vertical_limit(raw(1000.0, UNIT_FEET, Some(DATUM_GND))).unwrap();
        assert_eq!(
            limit.reference,
            VerticalReference::Agl(MetersAgl::from_feet(1000.0))
        );
    }

    #[test]
    fn vertical_limit_rejects_unknown_encodings() {
        assert!(vertical_limit(raw(100.0, 9, Some(DATUM_MSL))).is_err());
        assert!(vertical_limit(raw(100.0, UNIT_FEET, Some(7))).is_err());
        // Feet with the standard-atmosphere datum is contradictory.
        assert!(vertical_limit(raw(100.0, UNIT_FEET, Some(DATUM_STD))).is_err());
        assert!(vertical_limit(raw(100.0, UNIT_FEET, None)).is_err());
        // Fractional or out-of-range flight levels.
        assert!(vertical_limit(raw(100.5, UNIT_FLIGHT_LEVEL, Some(DATUM_STD))).is_err());
        assert!(vertical_limit(raw(-10.0, UNIT_FLIGHT_LEVEL, Some(DATUM_STD))).is_err());
    }

    #[test]
    fn elevation_conversions() {
        assert_eq!(
            elevation_amsl(raw(207.0, UNIT_METER, Some(DATUM_MSL))).unwrap(),
            MetersAmsl(207.0)
        );
        assert_eq!(
            elevation_amsl(raw(1000.0, UNIT_FEET, None)).unwrap(),
            MetersAmsl::from_feet(1000.0)
        );
        assert!(elevation_amsl(raw(207.0, UNIT_METER, Some(DATUM_GND))).is_err());
        assert!(elevation_amsl(raw(207.0, UNIT_FLIGHT_LEVEL, Some(DATUM_MSL))).is_err());
    }

    #[test]
    fn height_conversions() {
        assert_eq!(
            height_agl(raw(100.0, UNIT_METER, Some(DATUM_GND))).unwrap(),
            MetersAgl(100.0)
        );
        assert!(height_agl(raw(100.0, UNIT_METER, Some(DATUM_MSL))).is_err());
    }

    #[test]
    fn radio_frequency_units() {
        assert_eq!(
            radio_frequency("122.880", FREQ_UNIT_MHZ).unwrap(),
            RadioFrequency::from_mhz(122.88)
        );
        assert_eq!(
            radio_frequency("341.000", FREQ_UNIT_KHZ).unwrap(),
            RadioFrequency::from_khz(341.0)
        );
        assert!(radio_frequency("abc", FREQ_UNIT_MHZ).is_err());
        assert!(radio_frequency("122.880", 3).is_err());
    }

    #[test]
    fn point_position_parses_lon_lat_order() {
        let geometry = json!({ "type": "Point", "coordinates": [6.044, 50.776] });
        let position = point_position(&geometry).unwrap();
        assert!((position.lon() - 6.044).abs() < 1e-9);
        assert!((position.lat() - 50.776).abs() < 1e-9);
    }

    #[test]
    fn point_position_rejects_other_geometry() {
        let geometry = json!({ "type": "Polygon", "coordinates": [] });
        assert!(point_position(&geometry).is_err());
    }

    #[test]
    fn polygon_geometry_parses_rings() {
        let geometry = json!({
            "type": "Polygon",
            "coordinates": [
                [[9.0, 48.0], [10.0, 48.0], [10.0, 49.0], [9.0, 48.0]],
                [[9.4, 48.2], [9.6, 48.2], [9.5, 48.4], [9.4, 48.2]],
            ],
        });
        let polygon = polygon_geometry(&geometry).unwrap();
        assert_eq!(polygon.exterior().len(), 3);
        assert_eq!(polygon.holes().len(), 1);
    }

    #[test]
    fn polygon_geometry_rejects_garbage() {
        assert!(polygon_geometry(&json!({ "type": "Polygon" })).is_err());
        assert!(polygon_geometry(&json!({ "type": "Polygon", "coordinates": [] })).is_err());
        assert!(polygon_geometry(&json!({ "type": "MultiPolygon", "coordinates": [] })).is_err());
        assert!(polygon_geometry(&serde_json::Value::Null).is_err());
    }
}

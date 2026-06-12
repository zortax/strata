//! openAIP `/obstacles` payload → [`Obstacle`].

use serde::Deserialize;
use serde_json::Value;

use crate::domain::{Obstacle, ObstacleKind};

use super::NormalizationReport;
use super::common::{RawMeasurement, elevation_amsl, height_agl, item_id, point_position};

#[derive(Debug, Deserialize)]
struct RawObstacle {
    name: Option<String>,
    #[serde(rename = "type")]
    kind: u16,
    geometry: Value,
    /// Elevation of the obstacle top, meters MSL.
    elevation: Option<RawMeasurement>,
    /// Structure height, meters GND. Missing on many OSM-imported items.
    height: Option<RawMeasurement>,
}

pub(crate) fn normalize(items: &[Value]) -> (Vec<Obstacle>, NormalizationReport) {
    let mut obstacles = Vec::with_capacity(items.len());
    let mut report = NormalizationReport::new(items.len());
    for item in items {
        match normalize_one(item) {
            Ok(obstacle) => obstacles.push(obstacle),
            Err(reason) => report.skip(item_id(item), reason, "obstacle"),
        }
    }
    (obstacles, report)
}

fn normalize_one(item: &Value) -> Result<Obstacle, String> {
    let raw: RawObstacle =
        serde_json::from_value(item.clone()).map_err(|e| format!("malformed obstacle: {e}"))?;
    let position = point_position(&raw.geometry)?;
    let height =
        height_agl(raw.height.ok_or("missing height")?).map_err(|e| format!("height: {e}"))?;
    let elevation_top = elevation_amsl(raw.elevation.ok_or("missing elevation")?)
        .map_err(|e| format!("elevation: {e}"))?;
    Ok(Obstacle {
        name: raw.name.filter(|n| !n.is_empty()),
        kind: obstacle_kind(raw.kind),
        position,
        height,
        elevation_top,
        // openAIP does not publish obstacle lighting.
        lighted: false,
    })
}

/// openAIP obstacle `type` codes (verified against the obstacle schema):
/// 0 Obstacle (generic), 1 Chimney, 2 Building, 3 Wind Turbine, 4 Tower.
/// The generic code 0 has no dedicated domain variant and stays raw.
fn obstacle_kind(code: u16) -> ObstacleKind {
    match code {
        1 => ObstacleKind::Chimney,
        2 => ObstacleKind::Building,
        3 => ObstacleKind::WindTurbine,
        4 => ObstacleKind::Tower,
        other => ObstacleKind::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MetersAgl, MetersAmsl};

    fn fixture_items() -> Vec<Value> {
        super::super::fixture_items(include_str!(
            "../../../tests/fixtures/openaip/obstacles.json"
        ))
    }

    #[test]
    fn fixture_obstacles_normalize_or_skip_for_missing_height() {
        let items = fixture_items();
        let (obstacles, report) = normalize(&items);
        assert_eq!(report.total, 30);
        // 19 of the 30 fixture items are OSM imports without a height value;
        // the domain type requires one, so they are counted skips.
        assert_eq!(report.skipped.len(), 19);
        assert!(
            report
                .skipped
                .iter()
                .all(|(_, reason)| reason == "missing height"),
            "unexpected skip reasons: {:?}",
            report.skipped
        );
        assert_eq!(obstacles.len(), 11);
        assert_eq!(report.normalized(), 11);
    }

    #[test]
    fn wind_turbine_spot_check() {
        let (obstacles, _) = normalize(&fixture_items());
        let first = &obstacles[0];
        assert_eq!(first.name.as_deref(), Some("#784930"));
        // openAIP type 0 is the generic "Obstacle" code.
        assert_eq!(first.kind, ObstacleKind::Other(0));
        assert_eq!(first.height, MetersAgl(100.0));
        assert_eq!(first.elevation_top, MetersAmsl(305.0));
        assert!(!first.lighted);
        assert!((first.position.lat() - 50.934_119_5).abs() < 1e-9);
        assert!((first.position.lon() - 12.063_344_5).abs() < 1e-9);

        assert!(
            obstacles.iter().any(|o| o.kind == ObstacleKind::WindTurbine),
            "fixture contains type-3 wind turbines"
        );
    }

    #[test]
    fn kind_mapping() {
        assert_eq!(obstacle_kind(1), ObstacleKind::Chimney);
        assert_eq!(obstacle_kind(2), ObstacleKind::Building);
        assert_eq!(obstacle_kind(3), ObstacleKind::WindTurbine);
        assert_eq!(obstacle_kind(4), ObstacleKind::Tower);
        assert_eq!(obstacle_kind(0), ObstacleKind::Other(0));
        assert_eq!(obstacle_kind(77), ObstacleKind::Other(77));
    }
}

//! Hit-testing: everything under a clicked point.

use rusqlite::Connection;

use super::records::{self, FeatureRecord};
use super::{Feature, StoreError};
use crate::domain::{Airport, Airspace, BoundingBox, LatLon, Navaid, Obstacle, ReportingPoint};

/// Airspaces whose polygon contains `point` (R*Tree prefilter, exact
/// point-in-polygon after decode), followed by point features within
/// `tolerance_deg` of `point` ordered by increasing distance.
pub(super) fn feature_at(
    conn: &Connection,
    point: LatLon,
    tolerance_deg: f64,
) -> Result<Vec<Feature>, StoreError> {
    let mut hits = Vec::new();

    let point_box = BoundingBox::around(point, 0.0);
    for airspace in records::query_bbox::<Airspace>(conn, point_box)? {
        if airspace.geometry.contains(point) {
            hits.push(Feature::Airspace(airspace));
        }
    }

    let tolerance_box = BoundingBox::around(point, tolerance_deg);
    let mut nearby: Vec<(f64, Feature)> = Vec::new();
    collect_within::<Airport>(conn, point, tolerance_deg, tolerance_box, &mut nearby)?;
    collect_within::<Navaid>(conn, point, tolerance_deg, tolerance_box, &mut nearby)?;
    collect_within::<ReportingPoint>(conn, point, tolerance_deg, tolerance_box, &mut nearby)?;
    collect_within::<Obstacle>(conn, point, tolerance_deg, tolerance_box, &mut nearby)?;
    nearby.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.extend(nearby.into_iter().map(|(_, feature)| feature));

    Ok(hits)
}

fn collect_within<T: FeatureRecord>(
    conn: &Connection,
    point: LatLon,
    tolerance_deg: f64,
    tolerance_box: BoundingBox,
    out: &mut Vec<(f64, Feature)>,
) -> Result<(), StoreError> {
    for item in records::query_bbox::<T>(conn, tolerance_box)? {
        let Some(position) = item.position() else {
            continue;
        };
        let distance = planar_distance_deg(point, position);
        if distance <= tolerance_deg {
            out.push((distance, item.into_feature()));
        }
    }
    Ok(())
}

/// Euclidean distance on the lat/lon degree plane — consistent with
/// `tolerance_deg` and fine for hit-test radii at Germany's extent.
fn planar_distance_deg(a: LatLon, b: LatLon) -> f64 {
    (a.lat() - b.lat()).hypot(a.lon() - b.lon())
}

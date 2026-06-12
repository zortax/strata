//! App-side implementations of `strata-plan`'s source traits (plan §5.3):
//! terrain, obstacles and airspaces over the SQLite [`Store`], magnetic
//! variation over the WMM, and winds aloft over the prefetched gridded
//! ICON frames.
//!
//! The traits are deliberately not `Send + Sync` (see `strata_plan::sources`)
//! — every compute run constructs its sources **on the compute thread**
//! from `Send` ingredients (`Arc<Store>`, `Arc<WindsAloftFrames>`), so the
//! per-run caches (prefetched elevation tiles, bbox memos) never need
//! cross-thread synchronization and die with the run.
//!
//! [`Store`]: strata_data::store::Store

pub mod elevation;
pub mod features;
pub mod magvar;
pub mod winds;

pub use elevation::StoreElevationSource;
pub use features::{StoreAirspaceSource, StoreObstacleSource};
pub use magvar::WmmMagvarSource;
pub use winds::{
    FreezingLevelSource, GriddedWindsAloftSampler, LevelWinds, WindsAloftFrames, WindsTimeStep,
};

use strata_data::domain::{BoundingBox, LatLon};

/// Approximate meters per degree of latitude (and of longitude at the
/// equator) — for padding query bboxes, not for geometry.
const METERS_PER_DEGREE: f64 = 111_320.0;

/// One bbox covering `points` padded by `margin_meters` on every side —
/// the prefetch envelope for a route's sources (corridor half-width plus
/// slack). Longitude padding widens with latitude (`1/cos`), clamped at
/// ±85° so polar degenerate cases stay finite; the result is clamped into
/// valid coordinate ranges. `None` for an empty point set.
pub fn points_prefetch_bbox(
    points: impl IntoIterator<Item = LatLon>,
    margin_meters: f64,
) -> Option<BoundingBox> {
    let mut bounds: Option<(f64, f64, f64, f64)> = None; // west south east north
    for p in points {
        bounds = Some(match bounds {
            None => (p.lon(), p.lat(), p.lon(), p.lat()),
            Some((w, s, e, n)) => (
                w.min(p.lon()),
                s.min(p.lat()),
                e.max(p.lon()),
                n.max(p.lat()),
            ),
        });
    }
    let (west, south, east, north) = bounds?;
    let lat_pad = margin_meters / METERS_PER_DEGREE;
    let widest_lat = south.abs().max(north.abs()).min(85.0);
    let lon_pad = lat_pad / widest_lat.to_radians().cos();
    BoundingBox::new(
        (west - lon_pad).max(-180.0),
        (south - lat_pad).max(-90.0),
        (east + lon_pad).min(180.0),
        (north + lat_pad).min(90.0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    #[test]
    fn prefetch_bbox_pads_route_extent() {
        let bbox = points_prefetch_bbox([p(50.0, 8.0), p(51.0, 10.0)], 11_132.0).unwrap();
        // 11132 m ≈ 0.1° latitude.
        assert!((bbox.south() - 49.9).abs() < 1e-6);
        assert!((bbox.north() - 51.1).abs() < 1e-6);
        // Longitude padding is wider than latitude padding at 51°N.
        let lon_pad = bbox.east() - 10.0;
        assert!(lon_pad > 0.1 && lon_pad < 0.25, "lon pad {lon_pad}");
        assert!((8.0 - bbox.west() - lon_pad).abs() < 1e-9, "symmetric");
    }

    #[test]
    fn prefetch_bbox_handles_single_point_and_empty() {
        assert!(points_prefetch_bbox([], 1000.0).is_none());
        let bbox = points_prefetch_bbox([p(50.0, 8.0)], 1000.0).unwrap();
        assert!(bbox.contains(p(50.0, 8.0)));
        assert!(bbox.north() > bbox.south());
        assert!(bbox.east() > bbox.west());
    }

    #[test]
    fn prefetch_bbox_clamps_to_valid_ranges() {
        let bbox = points_prefetch_bbox([p(89.9, 179.9)], 100_000.0).unwrap();
        assert!(bbox.north() <= 90.0);
        assert!(bbox.east() <= 180.0);
        let bbox = points_prefetch_bbox([p(-89.9, -179.9)], 100_000.0).unwrap();
        assert!(bbox.south() >= -90.0);
        assert!(bbox.west() >= -180.0);
    }
}

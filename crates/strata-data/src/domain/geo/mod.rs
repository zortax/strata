//! WGS84 geographic primitives. Angles in degrees, lengths in meters.
//!
//! No antimeridian handling: all consumers operate on the Germany region
//! (spec: lat 47..55.2, lon 5.5..15.5).

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod magvar;
mod prepared;

pub use magvar::magvar;
pub use prepared::PreparedPolygon;

/// Errors constructing geographic primitives.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GeoError {
    #[error("latitude {0} outside [-90, 90] or not finite")]
    InvalidLatitude(f64),
    #[error("longitude {0} outside [-180, 180] or not finite")]
    InvalidLongitude(f64),
    #[error("{kind} requires at least {needed} points, got {got}")]
    TooFewPoints {
        kind: &'static str,
        needed: usize,
        got: usize,
    },
    #[error("bounding box edges out of order (requires west <= east and south <= north)")]
    InvalidBounds,
}

/// An angle in degrees. Semantics (bearing, declination, …) are carried by
/// the producing API; magnetic declination is east-positive.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Degrees(pub f64);

/// Horizontal length in meters (runway dimensions, distances).
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Meters(pub f64);

impl Meters {
    pub fn as_feet(self) -> f64 {
        self.0 * crate::domain::vertical::FEET_PER_METER
    }
}

/// A validated WGS84 position in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawLatLon")]
pub struct LatLon {
    lat: f64,
    lon: f64,
}

#[derive(Deserialize)]
struct RawLatLon {
    lat: f64,
    lon: f64,
}

impl TryFrom<RawLatLon> for LatLon {
    type Error = GeoError;

    fn try_from(raw: RawLatLon) -> Result<Self, Self::Error> {
        LatLon::new(raw.lat, raw.lon)
    }
}

impl LatLon {
    pub fn new(lat: f64, lon: f64) -> Result<Self, GeoError> {
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err(GeoError::InvalidLatitude(lat));
        }
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return Err(GeoError::InvalidLongitude(lon));
        }
        Ok(Self { lat, lon })
    }

    pub fn lat(&self) -> f64 {
        self.lat
    }

    pub fn lon(&self) -> f64 {
        self.lon
    }
}

impl fmt::Display for LatLon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ns = if self.lat >= 0.0 { 'N' } else { 'S' };
        let ew = if self.lon >= 0.0 { 'E' } else { 'W' };
        write!(
            f,
            "{:.5}°{ns} {:.5}°{ew}",
            self.lat.abs(),
            self.lon.abs()
        )
    }
}

/// An axis-aligned geographic rectangle in degrees. Edges are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawBoundingBox")]
pub struct BoundingBox {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

#[derive(Deserialize)]
struct RawBoundingBox {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

impl TryFrom<RawBoundingBox> for BoundingBox {
    type Error = GeoError;

    fn try_from(raw: RawBoundingBox) -> Result<Self, Self::Error> {
        BoundingBox::new(raw.west, raw.south, raw.east, raw.north)
    }
}

impl BoundingBox {
    pub fn new(west: f64, south: f64, east: f64, north: f64) -> Result<Self, GeoError> {
        let sw = LatLon::new(south, west)?;
        let ne = LatLon::new(north, east)?;
        Self::from_corners(sw, ne)
    }

    pub fn from_corners(south_west: LatLon, north_east: LatLon) -> Result<Self, GeoError> {
        if south_west.lon() > north_east.lon() || south_west.lat() > north_east.lat() {
            return Err(GeoError::InvalidBounds);
        }
        Ok(Self {
            west: south_west.lon(),
            south: south_west.lat(),
            east: north_east.lon(),
            north: north_east.lat(),
        })
    }

    /// Caller guarantees ordering and range validity (compile-time constants).
    pub(crate) const fn new_unchecked(west: f64, south: f64, east: f64, north: f64) -> Self {
        Self {
            west,
            south,
            east,
            north,
        }
    }

    /// Smallest box covering all `points`; `None` for an empty iterator.
    pub fn from_points<I: IntoIterator<Item = LatLon>>(points: I) -> Option<Self> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let mut bbox = Self {
            west: first.lon(),
            south: first.lat(),
            east: first.lon(),
            north: first.lat(),
        };
        for p in iter {
            bbox.west = bbox.west.min(p.lon());
            bbox.south = bbox.south.min(p.lat());
            bbox.east = bbox.east.max(p.lon());
            bbox.north = bbox.north.max(p.lat());
        }
        Some(bbox)
    }

    /// Box of half-width/half-height `half_extent_deg` around `center`,
    /// clamped to valid coordinate ranges. Used for tolerance hit-testing.
    pub fn around(center: LatLon, half_extent_deg: f64) -> Self {
        Self {
            west: (center.lon() - half_extent_deg).max(-180.0),
            south: (center.lat() - half_extent_deg).max(-90.0),
            east: (center.lon() + half_extent_deg).min(180.0),
            north: (center.lat() + half_extent_deg).min(90.0),
        }
    }

    pub fn west(&self) -> f64 {
        self.west
    }

    pub fn south(&self) -> f64 {
        self.south
    }

    pub fn east(&self) -> f64 {
        self.east
    }

    pub fn north(&self) -> f64 {
        self.north
    }

    pub fn south_west(&self) -> LatLon {
        LatLon {
            lat: self.south,
            lon: self.west,
        }
    }

    pub fn north_east(&self) -> LatLon {
        LatLon {
            lat: self.north,
            lon: self.east,
        }
    }

    pub fn center(&self) -> LatLon {
        LatLon {
            lat: (self.south + self.north) / 2.0,
            lon: (self.west + self.east) / 2.0,
        }
    }

    pub fn contains(&self, p: LatLon) -> bool {
        p.lon() >= self.west && p.lon() <= self.east && p.lat() >= self.south && p.lat() <= self.north
    }

    pub fn intersects(&self, other: &BoundingBox) -> bool {
        self.west <= other.east
            && other.west <= self.east
            && self.south <= other.north
            && other.south <= self.north
    }

    /// Whether `other` lies entirely within this box (inclusive edges).
    pub fn contains_bbox(&self, other: &BoundingBox) -> bool {
        self.west <= other.west
            && self.south <= other.south
            && other.east <= self.east
            && other.north <= self.north
    }
}

/// An open line string with at least two points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawPolyline")]
pub struct Polyline {
    points: Vec<LatLon>,
}

#[derive(Deserialize)]
struct RawPolyline {
    points: Vec<LatLon>,
}

impl TryFrom<RawPolyline> for Polyline {
    type Error = GeoError;

    fn try_from(raw: RawPolyline) -> Result<Self, Self::Error> {
        Polyline::new(raw.points)
    }
}

impl Polyline {
    pub fn new(points: Vec<LatLon>) -> Result<Self, GeoError> {
        if points.len() < 2 {
            return Err(GeoError::TooFewPoints {
                kind: "polyline",
                needed: 2,
                got: points.len(),
            });
        }
        Ok(Self { points })
    }

    pub fn points(&self) -> &[LatLon] {
        &self.points
    }

    pub fn bounding_box(&self) -> BoundingBox {
        // Invariant: at least two points.
        BoundingBox::from_points(self.points.iter().copied())
            .unwrap_or(BoundingBox::new_unchecked(0.0, 0.0, 0.0, 0.0))
    }
}

/// A polygon with one exterior ring and zero or more holes.
///
/// Rings are stored *unclosed* (no duplicated first point); constructors
/// normalize explicitly closed input. Ring orientation is not significant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawPolygon")]
pub struct Polygon {
    exterior: Vec<LatLon>,
    holes: Vec<Vec<LatLon>>,
}

#[derive(Deserialize)]
struct RawPolygon {
    exterior: Vec<LatLon>,
    holes: Vec<Vec<LatLon>>,
}

impl TryFrom<RawPolygon> for Polygon {
    type Error = GeoError;

    fn try_from(raw: RawPolygon) -> Result<Self, Self::Error> {
        Polygon::new(raw.exterior, raw.holes)
    }
}

impl Polygon {
    pub fn new(exterior: Vec<LatLon>, holes: Vec<Vec<LatLon>>) -> Result<Self, GeoError> {
        let exterior = normalize_ring(exterior)?;
        let holes = holes
            .into_iter()
            .map(normalize_ring)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { exterior, holes })
    }

    pub fn exterior(&self) -> &[LatLon] {
        &self.exterior
    }

    pub fn holes(&self) -> &[Vec<LatLon>] {
        &self.holes
    }

    pub fn bounding_box(&self) -> BoundingBox {
        // Invariant: exterior has at least three points.
        BoundingBox::from_points(self.exterior.iter().copied())
            .unwrap_or(BoundingBox::new_unchecked(0.0, 0.0, 0.0, 0.0))
    }

    /// Even-odd (ray casting) point-in-polygon test on the lat/lon plane,
    /// honoring holes. Behavior for points exactly on an edge is unspecified.
    /// Planar math is fine at Germany's extent for hit-testing purposes.
    pub fn contains(&self, p: LatLon) -> bool {
        ring_contains(&self.exterior, p) && !self.holes.iter().any(|h| ring_contains(h, p))
    }

    /// Total vertex count across the exterior ring and all holes — the
    /// cheap size signal consumers use to decide whether [`Self::prepared`]
    /// pays off.
    pub fn vertex_count(&self) -> usize {
        self.exterior.len() + self.holes.iter().map(Vec::len).sum::<usize>()
    }

    /// This polygon preprocessed for fast repeated containment tests (see
    /// [`PreparedPolygon`]); identical `contains` semantics.
    pub fn prepared(&self) -> PreparedPolygon<'_> {
        PreparedPolygon::new(self)
    }
}

/// Drops an explicit closing point, then requires >= 3 distinct vertices.
fn normalize_ring(mut ring: Vec<LatLon>) -> Result<Vec<LatLon>, GeoError> {
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 {
        return Err(GeoError::TooFewPoints {
            kind: "polygon ring",
            needed: 3,
            got: ring.len(),
        });
    }
    Ok(ring)
}

fn ring_contains(ring: &[LatLon], p: LatLon) -> bool {
    let (px, py) = (p.lon(), p.lat());
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, yi) = (ring[i].lon(), ring[i].lat());
        let (xj, yj) = (ring[j].lon(), ring[j].lat());
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    #[test]
    fn latlon_validation() {
        assert!(LatLon::new(50.0, 8.0).is_ok());
        assert_eq!(
            LatLon::new(90.1, 0.0),
            Err(GeoError::InvalidLatitude(90.1))
        );
        assert_eq!(
            LatLon::new(0.0, -180.5),
            Err(GeoError::InvalidLongitude(-180.5))
        );
        assert!(matches!(
            LatLon::new(f64::NAN, 0.0),
            Err(GeoError::InvalidLatitude(_))
        ));
    }

    #[test]
    fn latlon_display() {
        assert_eq!(ll(50.775_67, 6.044_39).to_string(), "50.77567°N 6.04439°E");
        assert_eq!(ll(-12.5, -45.25).to_string(), "12.50000°S 45.25000°W");
    }

    #[test]
    fn bbox_contains_and_intersects() {
        let de = BoundingBox::new(5.5, 47.0, 15.5, 55.2).unwrap();
        assert!(de.contains(ll(50.0, 10.0)));
        assert!(de.contains(ll(47.0, 5.5))); // inclusive edge
        assert!(!de.contains(ll(46.9, 10.0)));

        let overlapping = BoundingBox::new(14.0, 54.0, 20.0, 60.0).unwrap();
        let disjoint = BoundingBox::new(-10.0, 30.0, 0.0, 40.0).unwrap();
        let touching = BoundingBox::new(15.5, 47.0, 20.0, 55.2).unwrap();
        assert!(de.intersects(&overlapping));
        assert!(overlapping.intersects(&de));
        assert!(!de.intersects(&disjoint));
        assert!(de.intersects(&touching)); // inclusive touch
    }

    #[test]
    fn bbox_contains_bbox_is_inclusive() {
        let outer = BoundingBox::new(8.0, 48.0, 12.0, 52.0).unwrap();
        let inner = BoundingBox::new(8.0, 49.0, 11.0, 52.0).unwrap();
        assert!(outer.contains_bbox(&inner));
        assert!(outer.contains_bbox(&outer), "a box contains itself");
        assert!(!inner.contains_bbox(&outer));
        let nudged_out = BoundingBox::new(7.9, 49.0, 11.0, 52.0).unwrap();
        assert!(!outer.contains_bbox(&nudged_out));
    }

    #[test]
    fn bbox_rejects_reversed_edges() {
        assert_eq!(
            BoundingBox::new(10.0, 50.0, 5.0, 55.0),
            Err(GeoError::InvalidBounds)
        );
    }

    #[test]
    fn bbox_around_clamps() {
        let b = BoundingBox::around(ll(89.5, 179.5), 1.0);
        assert_eq!(b.north(), 90.0);
        assert_eq!(b.east(), 180.0);
    }

    #[test]
    fn polyline_needs_two_points() {
        assert!(matches!(
            Polyline::new(vec![ll(50.0, 8.0)]),
            Err(GeoError::TooFewPoints { needed: 2, .. })
        ));
        let line = Polyline::new(vec![ll(50.0, 8.0), ll(51.0, 9.0)]).unwrap();
        let bbox = line.bounding_box();
        assert_eq!(bbox.south(), 50.0);
        assert_eq!(bbox.east(), 9.0);
    }

    #[test]
    fn polygon_normalizes_closed_rings() {
        let square = Polygon::new(
            vec![ll(0.0, 0.0), ll(0.0, 4.0), ll(4.0, 4.0), ll(4.0, 0.0), ll(0.0, 0.0)],
            vec![],
        )
        .unwrap();
        assert_eq!(square.exterior().len(), 4);
    }

    #[test]
    fn polygon_contains_with_hole() {
        let square = Polygon::new(
            vec![ll(0.0, 0.0), ll(0.0, 4.0), ll(4.0, 4.0), ll(4.0, 0.0)],
            vec![vec![ll(1.0, 1.0), ll(1.0, 3.0), ll(3.0, 3.0), ll(3.0, 1.0)]],
        )
        .unwrap();
        assert!(square.contains(ll(0.5, 0.5)));
        assert!(square.contains(ll(3.5, 2.0)));
        assert!(!square.contains(ll(2.0, 2.0))); // inside the hole
        assert!(!square.contains(ll(5.0, 5.0))); // outside
    }

    #[test]
    fn polygon_postcard_round_trip() {
        // The store serializes geometry as postcard blobs; the validated
        // try_from-deserialization must survive the non-self-describing format.
        let polygon = Polygon::new(
            vec![ll(48.0, 9.0), ll(49.0, 10.0), ll(48.5, 11.0)],
            vec![vec![ll(48.4, 9.8), ll(48.6, 10.0), ll(48.4, 10.2)]],
        )
        .unwrap();
        let blob = postcard::to_stdvec(&polygon).unwrap();
        let back: Polygon = postcard::from_bytes(&blob).unwrap();
        assert_eq!(polygon, back);
    }

    #[test]
    fn polygon_bbox() {
        let tri = Polygon::new(vec![ll(48.0, 9.0), ll(49.0, 10.0), ll(48.5, 11.0)], vec![]).unwrap();
        let bbox = tri.bounding_box();
        assert_eq!(bbox.west(), 9.0);
        assert_eq!(bbox.north(), 49.0);
    }
}

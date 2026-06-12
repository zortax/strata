//! Latitude-banded point-in-polygon acceleration.
//!
//! The even-odd crossing predicate `(yi > py) != (yj > py)` can only be
//! satisfied by edges whose latitude span straddles the query latitude, so
//! [`PreparedPolygon`] buckets every ring's edges into uniform latitude
//! bands once and `contains` visits only the query band — `O(edges in
//! band)` instead of `O(V)` per test. Semantics are identical to
//! [`Polygon::contains`] by construction: the band always holds a superset
//! of the edges that can toggle, and extra edges evaluate the exact same
//! predicate (toggling nothing).
//!
//! Built by hot repeated-containment consumers (the flight corridor's
//! airspace pass tests hundreds of stations against polygons with 50–500
//! vertices) for high-vertex rings; small rings stay on the plain linear
//! test, which beats the bucketing overhead.

use super::{LatLon, Polygon};

/// A [`Polygon`] preprocessed for fast repeated [`contains`] tests.
///
/// [`contains`]: Self::contains
pub struct PreparedPolygon<'a> {
    exterior: PreparedRing<'a>,
    holes: Vec<PreparedRing<'a>>,
}

impl<'a> PreparedPolygon<'a> {
    pub(super) fn new(polygon: &'a Polygon) -> Self {
        Self {
            exterior: PreparedRing::new(polygon.exterior()),
            holes: polygon
                .holes()
                .iter()
                .map(|hole| PreparedRing::new(hole))
                .collect(),
        }
    }

    /// Identical semantics to [`Polygon::contains`].
    pub fn contains(&self, p: LatLon) -> bool {
        self.exterior.contains(p) && !self.holes.iter().any(|hole| hole.contains(p))
    }
}

/// One ring's edges bucketed into uniform latitude bands. Edge `i` is the
/// segment from vertex `i-1` (wrapping) to vertex `i`, matching the plain
/// `ring_contains` iteration.
struct PreparedRing<'a> {
    ring: &'a [LatLon],
    min_lat: f64,
    max_lat: f64,
    /// Bands per degree of latitude (0 for a degenerate flat ring — every
    /// edge then lives in band 0).
    scale: f64,
    /// Edge indices per band; an edge spanning several bands appears in
    /// each of them.
    bands: Vec<Vec<u32>>,
}

impl<'a> PreparedRing<'a> {
    fn new(ring: &'a [LatLon]) -> Self {
        let n = ring.len();
        let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
        for vertex in ring {
            min_lat = min_lat.min(vertex.lat());
            max_lat = max_lat.max(vertex.lat());
        }
        // ~4 edges per band on typical (locally short-edged) rings; capped
        // so a pathological ring of long edges stays bounded in memory.
        let band_count = (n / 4).clamp(1, 256);
        let scale = if max_lat > min_lat {
            band_count as f64 / (max_lat - min_lat)
        } else {
            0.0
        };
        let band_of =
            |lat: f64| -> usize { (((lat - min_lat) * scale) as usize).min(band_count - 1) };
        let mut bands = vec![Vec::new(); band_count];
        let mut j = n - 1;
        for (i, vertex) in ring.iter().enumerate() {
            let (lo, hi) = if vertex.lat() <= ring[j].lat() {
                (vertex.lat(), ring[j].lat())
            } else {
                (ring[j].lat(), vertex.lat())
            };
            for band in &mut bands[band_of(lo)..=band_of(hi)] {
                band.push(i as u32);
            }
            j = i;
        }
        Self {
            ring,
            min_lat,
            max_lat,
            scale,
            bands,
        }
    }

    fn contains(&self, p: LatLon) -> bool {
        let py = p.lat();
        // Outside the ring's latitude range no edge can straddle `py`
        // (`py == min/max` stays inside the banded path — edges with one
        // endpoint exactly at `py` can still toggle there).
        if py < self.min_lat || py > self.max_lat {
            return false;
        }
        let band = (((py - self.min_lat) * self.scale) as usize).min(self.bands.len() - 1);
        let (px, n) = (p.lon(), self.ring.len());
        let mut inside = false;
        for &edge in &self.bands[band] {
            let i = edge as usize;
            let j = if i == 0 { n - 1 } else { i - 1 };
            let (xi, yi) = (self.ring[i].lon(), self.ring[i].lat());
            let (xj, yj) = (self.ring[j].lon(), self.ring[j].lat());
            if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        inside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    /// A gnarly star-shaped ring (concave, edges crossing many latitude
    /// bands) with a square hole.
    fn star_with_hole() -> Polygon {
        let star: Vec<LatLon> = (0..30)
            .map(|k| {
                let theta = k as f64 / 30.0 * std::f64::consts::TAU;
                let r = if k % 2 == 0 { 2.0 } else { 0.7 };
                ll(50.0 + r * theta.sin(), 10.0 + r * theta.cos())
            })
            .collect();
        let hole = vec![ll(49.8, 9.8), ll(49.8, 10.2), ll(50.2, 10.2), ll(50.2, 9.8)];
        Polygon::new(star, vec![hole]).unwrap()
    }

    #[test]
    fn prepared_matches_the_plain_test_over_a_dense_grid() {
        let polygon = star_with_hole();
        let prepared = polygon.prepared();
        let mut inside = 0;
        for i in 0..=60 {
            for j in 0..=60 {
                let p = ll(47.9 + 4.2 * i as f64 / 60.0, 7.9 + 4.2 * j as f64 / 60.0);
                assert_eq!(prepared.contains(p), polygon.contains(p), "diverged at {p}");
                inside += usize::from(polygon.contains(p));
            }
        }
        // Sanity: the grid actually saw inside, outside and hole points.
        assert!(inside > 100, "grid covers the polygon ({inside} inside)");
    }

    #[test]
    fn prepared_handles_degenerate_flat_and_tiny_rings() {
        // Almost-flat sliver: all vertices within a hair of one latitude.
        let sliver = Polygon::new(
            vec![ll(50.0, 10.0), ll(50.0, 11.0), ll(50.000001, 10.5)],
            vec![],
        )
        .unwrap();
        let prepared = sliver.prepared();
        for p in [
            ll(50.0000005, 10.5),
            ll(50.1, 10.5),
            ll(49.9, 10.5),
            ll(50.0, 9.0),
        ] {
            assert_eq!(prepared.contains(p), sliver.contains(p), "at {p}");
        }
    }

    #[test]
    fn vertex_count_totals_exterior_and_holes() {
        assert_eq!(star_with_hole().vertex_count(), 30 + 4);
    }
}

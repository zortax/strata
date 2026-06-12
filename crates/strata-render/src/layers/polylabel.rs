//! Pole of inaccessibility: the interior point of a polygon (with holes)
//! farthest from any edge — the visual center used to place airspace and
//! SIGMET labels. Quadtree refinement after Mapbox's `polylabel`; pure
//! function, no allocation beyond the cell queue.

use glam::DVec2;

use std::collections::BinaryHeap;

/// Refinement never expands more cells than this, whatever the precision —
/// degenerate geometry must not stall a worker.
const MAX_CELLS: usize = 20_000;

/// Best interior point of `exterior` minus `holes`, refined until improving
/// by less than `precision` (same units as the input) is impossible.
///
/// Rings may or may not repeat their closing vertex. Returns `None` for
/// degenerate input (fewer than 3 distinct exterior vertices or an empty
/// bounding box).
pub fn pole_of_inaccessibility(
    exterior: &[DVec2],
    holes: &[Vec<DVec2>],
    precision: f64,
) -> Option<DVec2> {
    if exterior.len() < 3 {
        return None;
    }
    let (min, max) = bounding_box(exterior)?;
    let size = max - min;
    if size.x <= 0.0 || size.y <= 0.0 || !size.is_finite() {
        return None;
    }
    // Cap the initial grid for extremely elongated polygons; subdivision
    // recovers the detail where it matters.
    let cell_size = size.x.min(size.y).max(size.x.max(size.y) / 256.0);
    let precision = if precision > 0.0 && precision.is_finite() {
        precision
    } else {
        cell_size / 100.0
    };

    let mut queue = BinaryHeap::new();
    let half = cell_size / 2.0;
    // Initial grid covering the bbox.
    let mut x = min.x;
    while x < max.x {
        let mut y = min.y;
        while y < max.y {
            queue.push(Cell::new(
                DVec2::new(x + half, y + half),
                half,
                exterior,
                holes,
            ));
            y += cell_size;
        }
        x += cell_size;
    }

    // Seed with the centroid and the bbox center so thin polygons start well.
    let mut best = Cell::new(centroid(exterior), 0.0, exterior, holes);
    let bbox_center = Cell::new(min + size / 2.0, 0.0, exterior, holes);
    if bbox_center.dist > best.dist {
        best = bbox_center;
    }

    let mut expanded = 0usize;
    while let Some(cell) = queue.pop() {
        if cell.dist > best.dist {
            best = cell.clone();
        }
        // The heap is ordered by potential, so nothing left can beat best.
        if cell.potential - best.dist <= precision {
            break;
        }
        expanded += 1;
        if expanded > MAX_CELLS {
            break;
        }
        let h = cell.half / 2.0;
        for (dx, dy) in [(-h, -h), (h, -h), (-h, h), (h, h)] {
            queue.push(Cell::new(
                cell.center + DVec2::new(dx, dy),
                h,
                exterior,
                holes,
            ));
        }
    }
    Some(best.center)
}

/// Signed distance from `point` to the polygon boundary: positive inside,
/// negative outside (even-odd rule over all rings).
pub fn signed_distance(point: DVec2, exterior: &[DVec2], holes: &[Vec<DVec2>]) -> f64 {
    let mut inside = false;
    let mut min_dist_sq = f64::INFINITY;
    let rings = std::iter::once(exterior).chain(holes.iter().map(Vec::as_slice));
    for ring in rings {
        let n = ring.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            if (a.y > point.y) != (b.y > point.y) {
                let t = (point.y - a.y) / (b.y - a.y);
                if point.x < a.x + t * (b.x - a.x) {
                    inside = !inside;
                }
            }
            min_dist_sq = min_dist_sq.min(segment_distance_sq(point, a, b));
        }
    }
    let dist = min_dist_sq.sqrt();
    if inside { dist } else { -dist }
}

fn segment_distance_sq(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    let t = if len_sq > 0.0 {
        ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length_squared()
}

fn bounding_box(points: &[DVec2]) -> Option<(DVec2, DVec2)> {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in points {
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        min = min.min(*p);
        max = max.max(*p);
    }
    (min.x <= max.x && min.y <= max.y).then_some((min, max))
}

fn centroid(ring: &[DVec2]) -> DVec2 {
    let n = ring.len();
    let mut area = 0.0;
    let mut acc = DVec2::ZERO;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let cross = a.x * b.y - b.x * a.y;
        area += cross;
        acc += (a + b) * cross;
    }
    if area.abs() < f64::EPSILON {
        // Degenerate ring: fall back to the vertex mean.
        return ring.iter().copied().sum::<DVec2>() / n.max(1) as f64;
    }
    acc / (3.0 * area)
}

#[derive(Clone)]
struct Cell {
    center: DVec2,
    half: f64,
    /// Signed distance from the cell center to the polygon boundary.
    dist: f64,
    /// Upper bound of the distance anywhere inside the cell.
    potential: f64,
}

impl Cell {
    fn new(center: DVec2, half: f64, exterior: &[DVec2], holes: &[Vec<DVec2>]) -> Self {
        let dist = signed_distance(center, exterior, holes);
        Self {
            center,
            half,
            dist,
            potential: dist + half * std::f64::consts::SQRT_2,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.potential == other.potential
    }
}

impl Eq for Cell {}

impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cell {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.potential.total_cmp(&other.potential)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l_shape() -> Vec<DVec2> {
        // Unit-wide L: vertical arm x∈[0,1], horizontal arm y∈[0,1].
        [
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ]
        .map(|(x, y)| DVec2::new(x, y))
        .to_vec()
    }

    #[test]
    fn l_shape_label_stays_inside() {
        let exterior = l_shape();
        let p = pole_of_inaccessibility(&exterior, &[], 0.01).expect("non-degenerate");
        let dist = signed_distance(p, &exterior, &[]);
        assert!(dist > 0.0, "label point {p:?} is outside the L");
        // The widest interior disc of a unit-wide L has radius 0.5.
        assert!(dist > 0.4, "expected near-optimal distance, got {dist}");
        // Note: the centroid of this L (≈ (1.36, 1.36)) lies OUTSIDE.
        let c = centroid(&exterior);
        assert!(signed_distance(c, &exterior, &[]) < 0.0);
    }

    #[test]
    fn donut_label_avoids_the_hole() {
        let exterior: Vec<DVec2> = [(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (0.0, 6.0)]
            .map(|(x, y)| DVec2::new(x, y))
            .to_vec();
        let hole: Vec<DVec2> = [(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0)]
            .map(|(x, y)| DVec2::new(x, y))
            .to_vec();
        let holes = vec![hole];
        let p = pole_of_inaccessibility(&exterior, &holes, 0.01).expect("non-degenerate");
        let dist = signed_distance(p, &exterior, &holes);
        assert!(dist > 0.9, "expected ~1.0 in the ring, got {dist} at {p:?}");
    }

    #[test]
    fn convex_polygon_label_is_near_center() {
        let exterior: Vec<DVec2> = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]
            .map(|(x, y)| DVec2::new(x, y))
            .to_vec();
        let p = pole_of_inaccessibility(&exterior, &[], 0.001).expect("non-degenerate");
        assert!((p - DVec2::new(1.0, 1.0)).length() < 0.05);
    }

    #[test]
    fn closed_ring_with_repeated_vertex_works() {
        let exterior: Vec<DVec2> = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)]
            .map(|(x, y)| DVec2::new(x, y))
            .to_vec();
        let p = pole_of_inaccessibility(&exterior, &[], 0.001).expect("non-degenerate");
        assert!(signed_distance(p, &exterior, &[]) > 0.9);
    }

    #[test]
    fn degenerate_inputs_yield_none() {
        assert!(pole_of_inaccessibility(&[], &[], 0.01).is_none());
        let line: Vec<DVec2> = vec![DVec2::ZERO, DVec2::new(1.0, 0.0)];
        assert!(pole_of_inaccessibility(&line, &[], 0.01).is_none());
        // Zero-area "polygon".
        let flat: Vec<DVec2> = vec![DVec2::ZERO, DVec2::new(1.0, 0.0), DVec2::new(2.0, 0.0)];
        assert!(pole_of_inaccessibility(&flat, &[], 0.01).is_none());
    }
}

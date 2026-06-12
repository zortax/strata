//! Pure geometry of the CG-envelope editor plot: data ↔ pixel mapping,
//! axis ticks and vertex/segment hit-testing. No painting and no entities —
//! the drag math is unit-tested here and consumed by [`super::envelope`].
//!
//! Data space is the envelope's own (arm m, mass kg) plane; pixel space is
//! the plot area in window coordinates with the y axis pointing down (so
//! mass grows upward on screen).

use gpui::{Bounds, Pixels, Point, Size, point, px};
use strata_data::domain::Meters;
use strata_plan::aircraft::EnvelopePoint;
use strata_plan::units::Kilograms;

/// Grab distance for a vertex, in px.
pub const VERTEX_HIT_RADIUS_PX: f32 = 9.0;
/// Grab distance for a segment (double-click insert), in px.
pub const SEGMENT_HIT_RADIUS_PX: f32 = 6.0;

/// Margins between the plot bounds and the mapped data area — room for the
/// mass labels (left) and arm labels (bottom).
const MARGIN_LEFT_PX: f32 = 48.0;
const MARGIN_RIGHT_PX: f32 = 14.0;
const MARGIN_TOP_PX: f32 = 10.0;
const MARGIN_BOTTOM_PX: f32 = 22.0;

/// Fraction of the data span added as padding on each side, so vertices
/// never sit on the plot edge and there is room to drag outward.
const RANGE_PADDING: f64 = 0.18;
/// Minimum data spans — a degenerate polygon (all points equal) still maps.
const MIN_ARM_SPAN_M: f64 = 0.2;
const MIN_MASS_SPAN_KG: f64 = 100.0;

/// The data window the plot shows, padded around the polygon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataRange {
    pub arm_min: f64,
    pub arm_max: f64,
    pub mass_min: f64,
    pub mass_max: f64,
}

impl DataRange {
    /// The padded window around `points` (empty input gets a unit window).
    pub fn around(points: &[EnvelopePoint]) -> Self {
        let mut arm_min = f64::INFINITY;
        let mut arm_max = f64::NEG_INFINITY;
        let mut mass_min = f64::INFINITY;
        let mut mass_max = f64::NEG_INFINITY;
        for p in points {
            arm_min = arm_min.min(p.arm.0);
            arm_max = arm_max.max(p.arm.0);
            mass_min = mass_min.min(p.mass.0);
            mass_max = mass_max.max(p.mass.0);
        }
        if points.is_empty() {
            (arm_min, arm_max, mass_min, mass_max) = (0.0, 1.0, 0.0, 1.0);
        }
        let arm_pad = ((arm_max - arm_min).max(MIN_ARM_SPAN_M)) * RANGE_PADDING;
        let mass_pad = ((mass_max - mass_min).max(MIN_MASS_SPAN_KG)) * RANGE_PADDING;
        Self {
            arm_min: arm_min - arm_pad,
            arm_max: arm_max + arm_pad,
            mass_min: (mass_min - mass_pad).max(0.0),
            mass_max: mass_max + mass_pad,
        }
    }
}

/// The inner plot area inside `bounds`, leaving the axis-label margins.
pub fn plot_area(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(
            bounds.origin.x + px(MARGIN_LEFT_PX),
            bounds.origin.y + px(MARGIN_TOP_PX),
        ),
        size: Size {
            width: (bounds.size.width - px(MARGIN_LEFT_PX + MARGIN_RIGHT_PX)).max(px(1.)),
            height: (bounds.size.height - px(MARGIN_TOP_PX + MARGIN_BOTTOM_PX)).max(px(1.)),
        },
    }
}

/// Data ↔ pixel mapping over a plot area. Pixel positions are in the same
/// coordinate space as `area` (window coordinates in practice).
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeMapping {
    pub area: Bounds<Pixels>,
    pub range: DataRange,
}

impl EnvelopeMapping {
    pub fn new(area: Bounds<Pixels>, range: DataRange) -> Self {
        Self { area, range }
    }

    pub fn to_px(self, p: EnvelopePoint) -> Point<Pixels> {
        let arm_span = self.range.arm_max - self.range.arm_min;
        let mass_span = self.range.mass_max - self.range.mass_min;
        let fx = ((p.arm.0 - self.range.arm_min) / arm_span) as f32;
        let fy = ((p.mass.0 - self.range.mass_min) / mass_span) as f32;
        point(
            self.area.origin.x + self.area.size.width * fx,
            // Mass grows upward: invert y.
            self.area.origin.y + self.area.size.height * (1.0 - fy),
        )
    }

    /// Pixel → data, clamped into the mapping's data window (a drag cannot
    /// leave the plot).
    pub fn to_data(self, pos: Point<Pixels>) -> EnvelopePoint {
        let fx = f64::from((pos.x - self.area.origin.x) / self.area.size.width).clamp(0.0, 1.0);
        let fy = f64::from((pos.y - self.area.origin.y) / self.area.size.height).clamp(0.0, 1.0);
        EnvelopePoint {
            arm: Meters(self.range.arm_min + fx * (self.range.arm_max - self.range.arm_min)),
            mass: Kilograms(
                self.range.mass_min + (1.0 - fy) * (self.range.mass_max - self.range.mass_min),
            ),
        }
    }

    /// The vertex within [`VERTEX_HIT_RADIUS_PX`] of `pos`, nearest first.
    pub fn hit_vertex(&self, pos: Point<Pixels>, points: &[EnvelopePoint]) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in points.iter().enumerate() {
            let d = distance(self.to_px(*p), pos);
            if d <= VERTEX_HIT_RADIUS_PX && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// The closed-polygon segment within [`SEGMENT_HIT_RADIUS_PX`] of
    /// `pos`, as `(insert_index, projected data point)` — inserting the
    /// returned point at `insert_index` splits that segment in place.
    pub fn hit_segment(
        &self,
        pos: Point<Pixels>,
        points: &[EnvelopePoint],
    ) -> Option<(usize, EnvelopePoint)> {
        if points.len() < 2 {
            return None;
        }
        let mut best: Option<(usize, Point<Pixels>, f32)> = None;
        for i in 0..points.len() {
            let a = self.to_px(points[i]);
            let b = self.to_px(points[(i + 1) % points.len()]);
            let projected = project_on_segment(pos, a, b);
            let d = distance(projected, pos);
            if d <= SEGMENT_HIT_RADIUS_PX && best.is_none_or(|(.., bd)| d < bd) {
                best = Some((i + 1, projected, d));
            }
        }
        best.map(|(insert, projected, _)| (insert, self.to_data(projected)))
    }
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let dx = f32::from(a.x - b.x);
    let dy = f32::from(a.y - b.y);
    dx.hypot(dy)
}

/// The nearest point to `pos` on the segment `a`–`b`.
fn project_on_segment(pos: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> Point<Pixels> {
    let (ax, ay) = (f32::from(a.x), f32::from(a.y));
    let (bx, by) = (f32::from(b.x), f32::from(b.y));
    let (px_, py_) = (f32::from(pos.x), f32::from(pos.y));
    let (dx, dy) = (bx - ax, by - ay);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq <= f32::EPSILON {
        0.0
    } else {
        (((px_ - ax) * dx + (py_ - ay) * dy) / len_sq).clamp(0.0, 1.0)
    };
    point(px(ax + t * dx), px(ay + t * dy))
}

/// "Nice" axis tick values covering `min..max` (step 1/2/2.5/5 × 10ⁿ,
/// aiming at ~`target` ticks).
pub fn ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    let span = max - min;
    if !(span.is_finite()) || span <= 0.0 || target == 0 {
        return Vec::new();
    }
    let raw_step = span / target as f64;
    let magnitude = 10f64.powf(raw_step.log10().floor());
    let normalized = raw_step / magnitude;
    let nice = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 2.5 {
        2.5
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = nice * magnitude;
    let first = (min / step).ceil() * step;
    let mut out = Vec::new();
    let mut value = first;
    while value <= max + step * 1e-9 {
        // Snap floating-point drift onto the lattice for clean labels.
        out.push((value / step).round() * step);
        value += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    fn p(arm: f64, mass: f64) -> EnvelopePoint {
        EnvelopePoint {
            arm: Meters(arm),
            mass: Kilograms(mass),
        }
    }

    /// The C172-class example envelope from the bundled profiles.
    fn envelope() -> Vec<EnvelopePoint> {
        vec![
            p(0.89, 680.0),
            p(0.89, 885.0),
            p(1.04, 1157.0),
            p(1.20, 1157.0),
            p(1.20, 680.0),
        ]
    }

    fn mapping() -> EnvelopeMapping {
        // A 400×200 plot area at (100, 50).
        let area = Bounds {
            origin: point(px(100.), px(50.)),
            size: size(px(400.), px(200.)),
        };
        EnvelopeMapping::new(area, DataRange::around(&envelope()))
    }

    #[test]
    fn range_pads_the_polygon_and_keeps_mass_non_negative() {
        let range = DataRange::around(&envelope());
        assert!(range.arm_min < 0.89 && range.arm_max > 1.20);
        assert!(range.mass_min < 680.0 && range.mass_max > 1157.0);
        assert!(range.mass_min >= 0.0);

        // Degenerate polygon: the minimum spans keep the window usable.
        let range = DataRange::around(&[p(1.0, 800.0)]);
        assert!(range.arm_max - range.arm_min >= MIN_ARM_SPAN_M * 2.0 * RANGE_PADDING);
        assert!(range.mass_max - range.mass_min >= MIN_MASS_SPAN_KG * 2.0 * RANGE_PADDING);
    }

    #[test]
    fn px_data_round_trip_is_exact_within_float_noise() {
        let mapping = mapping();
        for v in envelope() {
            let back = mapping.to_data(mapping.to_px(v));
            assert!(
                (back.arm.0 - v.arm.0).abs() < 1e-3,
                "arm {} vs {}",
                back.arm.0,
                v.arm.0
            );
            assert!(
                (back.mass.0 - v.mass.0).abs() < 1.0,
                "mass {} vs {}",
                back.mass.0,
                v.mass.0
            );
        }
    }

    #[test]
    fn mapping_orientation_mass_grows_upward() {
        let mapping = mapping();
        let low = mapping.to_px(p(1.0, 700.0));
        let high = mapping.to_px(p(1.0, 1100.0));
        assert!(high.y < low.y, "heavier point paints higher (smaller y)");
        let aft = mapping.to_px(p(1.2, 700.0));
        assert!(aft.x > low.x, "aft arm paints to the right");
    }

    #[test]
    fn corners_of_the_data_window_map_to_the_plot_area_corners() {
        let mapping = mapping();
        let range = mapping.range;
        let top_left = mapping.to_px(p(range.arm_min, range.mass_max));
        assert!((f32::from(top_left.x) - 100.0).abs() < 0.51, "{top_left:?}");
        assert!((f32::from(top_left.y) - 50.0).abs() < 0.51, "{top_left:?}");
        let bottom_right = mapping.to_px(p(range.arm_max, range.mass_min));
        assert!(
            (f32::from(bottom_right.x) - 500.0).abs() < 0.51,
            "{bottom_right:?}"
        );
        assert!(
            (f32::from(bottom_right.y) - 250.0).abs() < 0.51,
            "{bottom_right:?}"
        );
    }

    #[test]
    fn dragging_to_a_pixel_clamps_into_the_data_window() {
        let mapping = mapping();
        let dragged = mapping.to_data(point(px(0.), px(0.)));
        assert!((dragged.arm.0 - mapping.range.arm_min).abs() < 1e-9);
        assert!((dragged.mass.0 - mapping.range.mass_max).abs() < 1e-9);
        let dragged = mapping.to_data(point(px(10_000.), px(10_000.)));
        assert!((dragged.arm.0 - mapping.range.arm_max).abs() < 1e-9);
        assert!((dragged.mass.0 - mapping.range.mass_min).abs() < 1e-9);
    }

    #[test]
    fn vertex_hit_testing_picks_the_nearest_within_radius() {
        let mapping = mapping();
        let points = envelope();
        let at = mapping.to_px(points[2]);
        assert_eq!(mapping.hit_vertex(at, &points), Some(2));
        // A few px off still grabs it…
        let off = point(at.x + px(5.), at.y - px(5.));
        assert_eq!(mapping.hit_vertex(off, &points), Some(2));
        // …far away does not.
        let far = point(at.x + px(40.), at.y + px(40.));
        assert_eq!(mapping.hit_vertex(far, &points), None);
    }

    #[test]
    fn segment_hit_returns_the_insert_index_and_projected_point() {
        let mapping = mapping();
        let points = envelope();
        // Midpoint of the closing segment points[4] → points[0] (the
        // minimum-mass bottom edge at constant 680 kg): insert index = len.
        let a = mapping.to_px(points[4]);
        let b = mapping.to_px(points[0]);
        let mid = point((a.x + b.x) / 2., (a.y + b.y) / 2.);
        let (insert, projected) = mapping.hit_segment(mid, &points).expect("on the segment");
        assert_eq!(insert, points.len());
        assert!((projected.mass.0 - 680.0).abs() < 1.0, "{projected:?}");
        let mid_arm = (points[4].arm.0 + points[0].arm.0) / 2.0;
        assert!((projected.arm.0 - mid_arm).abs() < 0.01, "{projected:?}");

        // Midpoint of segment 1 → 2: insert at 2.
        let a = mapping.to_px(points[1]);
        let b = mapping.to_px(points[2]);
        let mid = point((a.x + b.x) / 2., (a.y + b.y) / 2.);
        let (insert, _) = mapping.hit_segment(mid, &points).expect("on the segment");
        assert_eq!(insert, 2);

        // The polygon center hits nothing.
        let center = mapping.to_px(p(1.045, 920.0));
        assert_eq!(mapping.hit_segment(center, &points), None);

        // A vertex grab beats the segment only by the caller's ordering —
        // but the segment test alone must not panic near vertices.
        let _ = mapping.hit_segment(mapping.to_px(points[0]), &points);
    }

    #[test]
    fn ticks_are_nice_and_cover_the_range() {
        let t = ticks(660.0, 1240.0, 5);
        assert!(!t.is_empty());
        assert!(t.first().copied().unwrap() >= 660.0);
        assert!(t.last().copied().unwrap() <= 1240.0);
        // 580/5 = 116 → nice step 200? raw 116 → magnitude 100, normalized
        // 1.16 → step 200. Values land on the 200 lattice.
        for v in &t {
            assert!((v / 200.0 - (v / 200.0).round()).abs() < 1e-9, "{v}");
        }

        assert!(ticks(1.0, 1.0, 5).is_empty());
        let t = ticks(0.83, 1.27, 5);
        assert!(t.iter().all(|v| (0.83..=1.27).contains(v)));
    }
}

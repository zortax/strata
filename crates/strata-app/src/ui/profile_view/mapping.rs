//! World ↔ pixel mapping of the profile chart (design §3.3): along-track
//! meters on X, altitude meters AMSL on Y (labelled in NM / ft), plus the
//! axis-tick and quantization helpers. Pure — unit-tested without gpui.

use strata_plan::units::METERS_PER_NAUTICAL_MILE;

/// Feet per meter (the inverse of `strata_data`'s conversion, kept local
/// so the mapping stays dependency-free).
pub(crate) const FEET_PER_METER: f64 = 1.0 / 0.3048;

/// Altitude-drag quantum (design §3.3 "snap to 100 ft").
pub(crate) const ALTITUDE_SNAP_FEET: f64 = 100.0;

/// Maps `(along-track m, altitude m AMSL)` into a pixel rectangle: distance
/// grows rightward, altitude grows **upward** (pixel y is inverted).
/// Construction sanitizes degenerate ranges so the mapping is always
/// finite and invertible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChartMapping {
    origin: (f32, f32),
    size: (f32, f32),
    total_m: f64,
    floor_m: f64,
    ceil_m: f64,
}

impl ChartMapping {
    /// `origin`/`size` describe the plot rectangle in window pixels;
    /// `total_m` the route length; `floor_m..ceil_m` the altitude range.
    pub fn new(
        origin: (f32, f32),
        size: (f32, f32),
        total_m: f64,
        floor_m: f64,
        ceil_m: f64,
    ) -> Self {
        let total_m = if total_m.is_finite() && total_m > 1.0 {
            total_m
        } else {
            1.0
        };
        let (floor_m, ceil_m) = if floor_m.is_finite() && ceil_m.is_finite() && ceil_m > floor_m {
            (floor_m, ceil_m)
        } else {
            (0.0, 1.0)
        };
        Self {
            origin,
            size: (size.0.max(1.0), size.1.max(1.0)),
            total_m,
            floor_m,
            ceil_m,
        }
    }

    pub fn origin(&self) -> (f32, f32) {
        self.origin
    }

    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    pub fn total_m(&self) -> f64 {
        self.total_m
    }

    pub fn floor_m(&self) -> f64 {
        self.floor_m
    }

    pub fn ceil_m(&self) -> f64 {
        self.ceil_m
    }

    /// Pixel x of an along-track distance.
    pub fn x_at(&self, along_m: f64) -> f32 {
        let f = (along_m / self.total_m) as f32;
        self.origin.0 + f * self.size.0
    }

    /// Pixel y of an altitude; out-of-range altitudes map outside the
    /// rectangle (callers clamp world values where overdraw matters).
    pub fn y_at(&self, alt_m: f64) -> f32 {
        let f = ((alt_m - self.floor_m) / (self.ceil_m - self.floor_m)) as f32;
        // Inverted: higher altitude is higher on screen (smaller y).
        self.origin.1 + (1.0 - f) * self.size.1
    }

    /// Along-track meters under pixel x, clamped into the route.
    pub fn along_at(&self, x: f32) -> f64 {
        let f = f64::from((x - self.origin.0) / self.size.0);
        (f * self.total_m).clamp(0.0, self.total_m)
    }

    /// Altitude meters under pixel y, clamped into the chart range.
    pub fn alt_at(&self, y: f32) -> f64 {
        let f = f64::from(1.0 - (y - self.origin.1) / self.size.1);
        (self.floor_m + f * (self.ceil_m - self.floor_m)).clamp(self.floor_m, self.ceil_m)
    }

    /// Altitude clamped into the drawable range — for series that may run
    /// off the chart (freezing level below ground, UNL ceilings).
    pub fn clamp_alt(&self, alt_m: f64) -> f64 {
        alt_m.clamp(self.floor_m, self.ceil_m)
    }

    /// Vertical scale: pixels per meter of altitude (drives the px gates —
    /// band thinness culling, label fit — on world-space thicknesses).
    pub fn px_per_alt_m(&self) -> f64 {
        f64::from(self.size.1) / (self.ceil_m - self.floor_m)
    }

    /// Vertical exaggeration of the current scales: how many times steeper
    /// a slope is drawn than it is — `(px per meter up) / (px per meter
    /// along)`. The design's "×12" indicator.
    pub fn exaggeration(&self) -> f64 {
        let px_per_m_y = f64::from(self.size.1) / (self.ceil_m - self.floor_m);
        let px_per_m_x = f64::from(self.size.0) / self.total_m;
        px_per_m_y / px_per_m_x
    }

    /// Whether a window pixel lies inside the plot rectangle.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.origin.0
            && x <= self.origin.0 + self.size.0
            && y >= self.origin.1
            && y <= self.origin.1 + self.size.1
    }
}

/// "Nice" tick values covering `[min, max]` with roughly `target` steps:
/// the classic 1/2/5 ladder, ticks aligned to multiples of the step.
pub(crate) fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() || max <= min || target == 0 {
        return Vec::new();
    }
    let raw_step = (max - min) / target as f64;
    let magnitude = 10f64.powf(raw_step.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|s| *s >= raw_step)
        .unwrap_or(10.0 * magnitude);
    let mut ticks = Vec::new();
    let mut value = (min / step).ceil() * step;
    while value <= max + step * 1e-9 {
        // Normalize "-0.0" and float drift on the zero tick.
        ticks.push(if value.abs() < step * 1e-9 {
            0.0
        } else {
            value
        });
        value += step;
    }
    ticks
}

/// Snaps an altitude in meters to the nearest 100 ft, returned in **feet**
/// (the drag commit's unit; never negative).
pub(crate) fn snap_altitude_feet(alt_m: f64) -> f64 {
    let feet = alt_m * FEET_PER_METER;
    ((feet / ALTITUDE_SNAP_FEET).round() * ALTITUDE_SNAP_FEET).max(0.0)
}

/// The route leg containing `along_m`, given cumulative leg-end distances
/// (a station exactly on a boundary belongs to the **earlier** leg —
/// `render_route`'s convention; past the end rounds into the final leg).
/// `None` only for an empty route.
pub(crate) fn leg_at(leg_ends_m: &[f64], along_m: f64) -> Option<usize> {
    if leg_ends_m.is_empty() {
        return None;
    }
    for (index, end) in leg_ends_m.iter().enumerate() {
        if along_m <= *end {
            return Some(index);
        }
    }
    Some(leg_ends_m.len() - 1)
}

/// Distance from point `p` to the segment `a..b` (pixel space; the planned
/// line's hover/drag hit test).
pub(crate) fn point_segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq <= f32::EPSILON {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Meters → nautical miles (display helper).
pub(crate) fn meters_to_nm(m: f64) -> f64 {
    m / METERS_PER_NAUTICAL_MILE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> ChartMapping {
        // 100 km route, 0–3000 m altitude in a 500×300 px rect at (50, 20).
        ChartMapping::new((50.0, 20.0), (500.0, 300.0), 100_000.0, 0.0, 3000.0)
    }

    #[test]
    fn distance_maps_left_to_right_and_inverts() {
        let m = mapping();
        assert_eq!(m.x_at(0.0), 50.0);
        assert_eq!(m.x_at(100_000.0), 550.0);
        assert_eq!(m.x_at(50_000.0), 300.0);

        assert!((m.along_at(300.0) - 50_000.0).abs() < 1.0);
        // Clamped beyond the rect.
        assert_eq!(m.along_at(0.0), 0.0);
        assert_eq!(m.along_at(900.0), 100_000.0);
    }

    #[test]
    fn altitude_maps_bottom_to_top_and_inverts() {
        let m = mapping();
        assert_eq!(m.y_at(0.0), 320.0, "floor sits on the bottom edge");
        assert_eq!(m.y_at(3000.0), 20.0, "ceiling on the top edge");
        assert_eq!(m.y_at(1500.0), 170.0);

        assert!((m.alt_at(170.0) - 1500.0).abs() < 1.0);
        // Clamped into the chart range.
        assert_eq!(m.alt_at(1000.0), 0.0);
        assert_eq!(m.alt_at(-50.0), 3000.0);
        assert_eq!(m.clamp_alt(-400.0), 0.0);
        assert_eq!(m.clamp_alt(9999.0), 3000.0);
    }

    #[test]
    fn px_per_alt_m_is_the_vertical_scale() {
        // 300 px over 3000 m → 0.1 px/m.
        assert!((mapping().px_per_alt_m() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn exaggeration_is_the_scale_ratio() {
        // px/m vertically: 300 / 3000 = 0.1; horizontally: 500 / 100000 =
        // 0.005 → ×20.
        assert!((mapping().exaggeration() - 20.0).abs() < 1e-9);
        // A mapping drawn to true scale reads ×1.
        let true_scale = ChartMapping::new((0.0, 0.0), (1000.0, 100.0), 10_000.0, 0.0, 1000.0);
        assert!((true_scale.exaggeration() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_inputs_are_sanitized() {
        let m = ChartMapping::new((0.0, 0.0), (0.0, -5.0), 0.0, 100.0, 100.0);
        assert!(m.x_at(0.5).is_finite());
        assert!(m.y_at(0.5).is_finite());
        assert!(m.exaggeration().is_finite());
        let m = ChartMapping::new((0.0, 0.0), (100.0, 100.0), f64::NAN, 0.0, f64::NAN);
        assert!(m.along_at(50.0).is_finite());
        assert!(m.alt_at(50.0).is_finite());
    }

    #[test]
    fn nice_ticks_use_the_1_2_5_ladder() {
        // 0–54 NM, ~6 ticks → step 10.
        assert_eq!(
            nice_ticks(0.0, 54.0, 6),
            vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0]
        );
        // 0–4500 ft, ~5 ticks → step 1000.
        assert_eq!(
            nice_ticks(0.0, 4500.0, 5),
            vec![0.0, 1000.0, 2000.0, 3000.0, 4000.0]
        );
        // Small ranges step on the 2-ladder.
        assert_eq!(nice_ticks(0.0, 9.0, 5), vec![0.0, 2.0, 4.0, 6.0, 8.0]);
        // Non-zero minimum: ticks stay aligned to step multiples.
        assert_eq!(nice_ticks(330.0, 1700.0, 4), vec![500.0, 1000.0, 1500.0]);
        assert!(nice_ticks(5.0, 5.0, 4).is_empty());
        assert!(nice_ticks(0.0, 10.0, 0).is_empty());
    }

    #[test]
    fn altitude_snaps_to_100_ft() {
        // 1000 m = 3280.8 ft → 3300 ft.
        assert_eq!(snap_altitude_feet(1000.0), 3300.0);
        // 914.4 m = exactly 3000 ft.
        assert_eq!(snap_altitude_feet(914.4), 3000.0);
        // Clearly below / above the 3250 midpoint (the exact half-way
        // case is float-round-trip unstable and not part of the contract).
        assert_eq!(snap_altitude_feet(3249.0 * 0.3048), 3200.0);
        assert_eq!(snap_altitude_feet(3251.0 * 0.3048), 3300.0);
        // Below zero clamps to 0 (no negative cruise altitudes).
        assert_eq!(snap_altitude_feet(-100.0), 0.0);
    }

    #[test]
    fn leg_lookup_follows_the_boundary_convention() {
        let ends = [10_000.0, 30_000.0, 60_000.0];
        assert_eq!(leg_at(&ends, 0.0), Some(0));
        assert_eq!(leg_at(&ends, 9_999.0), Some(0));
        // A boundary belongs to the earlier leg (matches render_route).
        assert_eq!(leg_at(&ends, 10_000.0), Some(0));
        assert_eq!(leg_at(&ends, 10_001.0), Some(1));
        assert_eq!(leg_at(&ends, 45_000.0), Some(2));
        // Past the end (float noise) rounds into the final leg.
        assert_eq!(leg_at(&ends, 60_001.0), Some(2));
        assert_eq!(leg_at(&[], 5.0), None);
    }

    #[test]
    fn point_segment_distance_handles_ends_and_degenerate_segments() {
        let a = (0.0, 0.0);
        let b = (10.0, 0.0);
        assert_eq!(point_segment_distance((5.0, 3.0), a, b), 3.0);
        assert_eq!(point_segment_distance((-4.0, 0.0), a, b), 4.0);
        assert_eq!(point_segment_distance((13.0, 4.0), a, b), 5.0);
        // Zero-length segment: plain point distance.
        assert_eq!(point_segment_distance((3.0, 4.0), a, a), 5.0);
    }
}

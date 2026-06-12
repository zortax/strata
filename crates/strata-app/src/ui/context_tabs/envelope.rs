//! The CG envelope plot (design §3.4 "Loading"): the profile's (arm, mass)
//! envelope polygon, the takeoff / zero-fuel / landing CG points and the
//! fuel-burn track, custom-painted with gpui's `PathBuilder` (the plan's
//! §5.2 choice — chart widgets can't draw polygon + track + points in one
//! coordinate space).
//!
//! The data→pixel mapping is pure ([`PlotRanges`], [`PlotMapping`]) and
//! unit-tested; the paint closure only walks it.

use gpui::{
    AnyElement, App, Bounds, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels,
    Styled as _, canvas, div, point, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use strata_plan::aircraft::EnvelopePoint;
use strata_plan::wb::{WbReport, WbState, WbStateKind};

/// Height of the painted plot area.
const PLOT_HEIGHT_PX: f32 = 190.;
/// Inner padding between the plot frame and the mapped data extent.
const PLOT_INSET_PX: f32 = 10.;
/// Radius of a CG state point.
const POINT_RADIUS_PX: f32 = 4.;
/// Fraction the data ranges are padded by on each side so envelope edges
/// don't hug the frame.
const RANGE_PAD_FRACTION: f64 = 0.08;
/// Span substituted for a degenerate (single-value) axis so the mapping
/// stays finite: ±0.05 m arm / ±10 kg mass around the value.
const MIN_ARM_SPAN: f64 = 0.1;
const MIN_MASS_SPAN: f64 = 20.0;

// --- pure mapping -------------------------------------------------------------

/// Padded data extent of everything the plot shows, in (arm m, mass kg).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlotRanges {
    pub arm: (f64, f64),
    pub mass: (f64, f64),
}

impl PlotRanges {
    /// Extent of `points` padded by [`RANGE_PAD_FRACTION`] per side;
    /// degenerate axes are widened to a minimum span. `None` when `points`
    /// is empty or contains non-finite values.
    pub fn covering(points: impl IntoIterator<Item = (f64, f64)>) -> Option<Self> {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for (arm, mass) in points {
            if !arm.is_finite() || !mass.is_finite() {
                return None;
            }
            bounds = Some(match bounds {
                None => (arm, mass, arm, mass),
                Some((a0, m0, a1, m1)) => (a0.min(arm), m0.min(mass), a1.max(arm), m1.max(mass)),
            });
        }
        let (a0, m0, a1, m1) = bounds?;
        let (a0, a1) = pad_span(a0, a1, MIN_ARM_SPAN);
        let (m0, m1) = pad_span(m0, m1, MIN_MASS_SPAN);
        Some(Self {
            arm: (a0, a1),
            mass: (m0, m1),
        })
    }
}

/// Pads `[lo, hi]` by [`RANGE_PAD_FRACTION`] per side, widening degenerate
/// spans to `min_span` first.
fn pad_span(lo: f64, hi: f64, min_span: f64) -> (f64, f64) {
    let (lo, hi) = if hi - lo < min_span {
        let mid = (lo + hi) / 2.0;
        (mid - min_span / 2.0, mid + min_span / 2.0)
    } else {
        (lo, hi)
    };
    let pad = (hi - lo) * RANGE_PAD_FRACTION;
    (lo - pad, hi + pad)
}

/// Maps (arm, mass) data coordinates into a pixel rectangle: arm grows
/// rightward, mass grows **upward** (pixel y is inverted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlotMapping {
    ranges: PlotRanges,
    /// Top-left of the drawable rectangle, in window pixels.
    origin: (f32, f32),
    size: (f32, f32),
}

impl PlotMapping {
    pub fn new(ranges: PlotRanges, origin: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            ranges,
            origin,
            size,
        }
    }

    /// Pixel position of a data point (clamping is the caller's choice —
    /// out-of-range data maps outside the rectangle, which is exactly what
    /// an out-of-envelope point should do visually).
    pub fn to_px(self, arm: f64, mass: f64) -> (f32, f32) {
        let (a0, a1) = self.ranges.arm;
        let (m0, m1) = self.ranges.mass;
        let fx = ((arm - a0) / (a1 - a0)) as f32;
        let fy = ((mass - m0) / (m1 - m0)) as f32;
        (
            self.origin.0 + fx * self.size.0,
            // Inverted: larger mass is higher on screen (smaller y).
            self.origin.1 + (1.0 - fy) * self.size.1,
        )
    }
}

// --- the plotted element --------------------------------------------------------

/// One CG point to plot.
#[derive(Debug, Clone, Copy)]
struct PlotPoint {
    arm: f64,
    mass: f64,
    color: Hsla,
}

/// Everything the paint closure needs, resolved up front (theme colors are
/// captured values — the closure has no `cx`).
struct PlotPaint {
    ranges: PlotRanges,
    envelope: Vec<(f64, f64)>,
    burn_track: Vec<(f64, f64)>,
    points: Vec<PlotPoint>,
    envelope_fill: Hsla,
    envelope_stroke: Hsla,
    track_color: Hsla,
}

/// Short label + theme color for a plotted W&B state. Ramp is skipped —
/// the design plots takeoff / zero-fuel / landing; ramp differs from
/// takeoff by taxi fuel only and would overplot it.
fn state_style(kind: WbStateKind, cx: &App) -> Option<(&'static str, Hsla)> {
    match kind {
        WbStateKind::Ramp => None,
        WbStateKind::Takeoff => Some(("T/O", cx.theme().primary)),
        WbStateKind::ZeroFuel => Some(("ZFW", cx.theme().info)),
        WbStateKind::Landing => Some(("LDG", cx.theme().success)),
    }
}

/// The envelope plot card body: painted canvas + axis labels + legend.
/// `report` is the latest computed W&B; without one only the envelope
/// polygon is drawn.
pub(crate) fn envelope_plot(
    envelope: &[EnvelopePoint],
    report: Option<&WbReport>,
    cx: &App,
) -> AnyElement {
    let envelope_points: Vec<(f64, f64)> = envelope.iter().map(|p| (p.arm.0, p.mass.0)).collect();
    let burn_track: Vec<(f64, f64)> = report
        .map(|r| r.burn_track.iter().map(|p| (p.arm.0, p.mass.0)).collect())
        .unwrap_or_default();
    let states: Vec<WbState> = report.map(|r| r.states.clone()).unwrap_or_default();

    let plotted_states: Vec<(WbState, &'static str, Hsla)> = states
        .iter()
        .filter_map(|s| state_style(s.kind, cx).map(|(label, color)| (*s, label, color)))
        .collect();
    let points: Vec<PlotPoint> = plotted_states
        .iter()
        .map(|(s, _, color)| PlotPoint {
            arm: s.arm.0,
            mass: s.mass.0,
            color: if s.within_envelope {
                *color
            } else {
                cx.theme().danger
            },
        })
        .collect();

    let all_points = envelope_points
        .iter()
        .chain(burn_track.iter())
        .copied()
        .chain(points.iter().map(|p| (p.arm, p.mass)));
    let Some(ranges) = PlotRanges::covering(all_points) else {
        return div()
            .h(px(PLOT_HEIGHT_PX))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("No CG envelope in the aircraft profile")
            .into_any_element();
    };

    let paint_data = PlotPaint {
        ranges,
        envelope: envelope_points,
        burn_track,
        points,
        envelope_fill: cx.theme().primary.opacity(0.08),
        envelope_stroke: cx.theme().primary.opacity(0.55),
        track_color: cx.theme().muted_foreground.opacity(0.8),
    };

    // Corner axis annotations (the mapped extent), placed as overlay text
    // so no glyph painting happens inside the canvas.
    let arm_label = |v: f64| format!("{v:.2} m");
    let mass_label = |v: f64| format!("{v:.0} kg");

    let legend = h_flex()
        .gap_3()
        .flex_wrap()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .children(plotted_states.iter().map(|(state, label, color)| {
            let color = if state.within_envelope {
                *color
            } else {
                cx.theme().danger
            };
            h_flex()
                .gap_1()
                .items_center()
                .child(div().size_2().rounded_full().bg(color))
                .child(format!(
                    "{label} {:.0} kg @ {:.2} m{}",
                    state.mass.0,
                    state.arm.0,
                    if state.within_envelope {
                        ""
                    } else {
                        " — out"
                    }
                ))
        }));

    v_flex()
        .gap_1p5()
        .child(
            div()
                .relative()
                .w_full()
                .h(px(PLOT_HEIGHT_PX))
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background.opacity(0.4))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, (), window, _| paint_plot(&paint_data, bounds, window),
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .top_1()
                        .left_1p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(mass_label(ranges.mass.1)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_1()
                        .left_1p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {}",
                            mass_label(ranges.mass.0),
                            arm_label(ranges.arm.0)
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_1()
                        .right_1p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(arm_label(ranges.arm.1)),
                ),
        )
        .child(legend)
        .into_any_element()
}

/// Paints polygon, burn track and CG points into `bounds` (paint phase —
/// pure pixel pushing, every decision was made in [`envelope_plot`]).
fn paint_plot(data: &PlotPaint, bounds: Bounds<Pixels>, window: &mut gpui::Window) {
    let origin = (
        f32::from(bounds.origin.x) + PLOT_INSET_PX,
        f32::from(bounds.origin.y) + PLOT_INSET_PX,
    );
    let size = (
        (f32::from(bounds.size.width) - 2. * PLOT_INSET_PX).max(1.),
        (f32::from(bounds.size.height) - 2. * PLOT_INSET_PX).max(1.),
    );
    let mapping = PlotMapping::new(data.ranges, origin, size);

    // Envelope polygon: translucent fill + outline (closed ring).
    if data.envelope.len() >= 3 {
        if let Some(path) = polygon_path(&data.envelope, &mapping, PathStyle::Fill) {
            window.paint_path(path, data.envelope_fill);
        }
        if let Some(path) = polygon_path(&data.envelope, &mapping, PathStyle::Stroke(px(1.5))) {
            window.paint_path(path, data.envelope_stroke);
        }
    }

    // Fuel-burn CG track (takeoff → zero fuel; landing lies on it).
    if data.burn_track.len() >= 2 {
        let mut builder = PathBuilder::stroke(px(1.));
        let mut points = data.burn_track.iter();
        if let Some(&(arm, mass)) = points.next() {
            let (x, y) = mapping.to_px(arm, mass);
            builder.move_to(point(px(x), px(y)));
            for &(arm, mass) in points {
                let (x, y) = mapping.to_px(arm, mass);
                builder.line_to(point(px(x), px(y)));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, data.track_color);
            }
        }
    }

    // CG state points, painted last so they sit on top.
    for p in &data.points {
        let (x, y) = mapping.to_px(p.arm, p.mass);
        let r = POINT_RADIUS_PX;
        window.paint_quad(
            gpui::fill(
                Bounds::new(
                    point(px(x - r), px(y - r)),
                    gpui::size(px(2. * r), px(2. * r)),
                ),
                p.color,
            )
            .corner_radii(px(r)),
        );
    }
}

enum PathStyle {
    Fill,
    Stroke(Pixels),
}

/// A closed polygon path over `vertices` (unclosed ring, the profile's
/// convention) mapped to pixels. `None` for degenerate input or an empty
/// path build.
fn polygon_path(
    vertices: &[(f64, f64)],
    mapping: &PlotMapping,
    style: PathStyle,
) -> Option<gpui::Path<Pixels>> {
    if vertices.len() < 3 {
        return None;
    }
    let mut builder = match style {
        PathStyle::Fill => PathBuilder::fill(),
        PathStyle::Stroke(width) => PathBuilder::stroke(width),
    };
    let (x, y) = mapping.to_px(vertices[0].0, vertices[0].1);
    builder.move_to(point(px(x), px(y)));
    for &(arm, mass) in &vertices[1..] {
        let (x, y) = mapping.to_px(arm, mass);
        builder.line_to(point(px(x), px(y)));
    }
    // Close the ring back to the first vertex.
    let (x, y) = mapping.to_px(vertices[0].0, vertices[0].1);
    builder.line_to(point(px(x), px(y)));
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_and_pad_the_data() {
        let ranges =
            PlotRanges::covering([(1.0, 700.0), (1.2, 1100.0), (0.9, 900.0)]).expect("ranges");
        // Raw extent: arm 0.9–1.2, mass 700–1100; padded by 8 % per side.
        assert!(
            (ranges.arm.0 - (0.9 - 0.3 * 0.08)).abs() < 1e-9,
            "{ranges:?}"
        );
        assert!(
            (ranges.arm.1 - (1.2 + 0.3 * 0.08)).abs() < 1e-9,
            "{ranges:?}"
        );
        assert!((ranges.mass.0 - (700.0 - 400.0 * 0.08)).abs() < 1e-9);
        assert!((ranges.mass.1 - (1100.0 + 400.0 * 0.08)).abs() < 1e-9);
    }

    #[test]
    fn empty_or_non_finite_data_yields_no_ranges() {
        assert_eq!(PlotRanges::covering([]), None);
        assert_eq!(PlotRanges::covering([(f64::NAN, 1.0)]), None);
        assert_eq!(PlotRanges::covering([(1.0, f64::INFINITY)]), None);
    }

    #[test]
    fn degenerate_spans_widen_to_a_minimum() {
        let ranges = PlotRanges::covering([(1.0, 1000.0)]).expect("single point maps");
        assert!(ranges.arm.1 - ranges.arm.0 >= MIN_ARM_SPAN, "{ranges:?}");
        assert!(ranges.mass.1 - ranges.mass.0 >= MIN_MASS_SPAN, "{ranges:?}");
        // Centered on the value.
        assert!(((ranges.arm.0 + ranges.arm.1) / 2.0 - 1.0).abs() < 1e-9);
        assert!(((ranges.mass.0 + ranges.mass.1) / 2.0 - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn mapping_puts_min_bottom_left_and_max_top_right() {
        let ranges = PlotRanges {
            arm: (1.0, 2.0),
            mass: (500.0, 1000.0),
        };
        let mapping = PlotMapping::new(ranges, (10.0, 20.0), (100.0, 200.0));

        // Min arm / min mass → left edge, bottom edge (y inverted).
        let (x, y) = mapping.to_px(1.0, 500.0);
        assert!((x - 10.0).abs() < 1e-4);
        assert!((y - 220.0).abs() < 1e-4);

        // Max arm / max mass → right edge, top edge.
        let (x, y) = mapping.to_px(2.0, 1000.0);
        assert!((x - 110.0).abs() < 1e-4);
        assert!((y - 20.0).abs() < 1e-4);

        // Midpoint maps to the rectangle center.
        let (x, y) = mapping.to_px(1.5, 750.0);
        assert!((x - 60.0).abs() < 1e-4);
        assert!((y - 120.0).abs() < 1e-4);
    }

    #[test]
    fn out_of_range_data_maps_outside_the_rectangle() {
        let ranges = PlotRanges {
            arm: (1.0, 2.0),
            mass: (500.0, 1000.0),
        };
        let mapping = PlotMapping::new(ranges, (0.0, 0.0), (100.0, 100.0));
        // Heavier than the range top → above the rectangle (y < 0); that is
        // exactly how an out-of-envelope point escapes the polygon visually.
        let (_, y) = mapping.to_px(1.5, 1200.0);
        assert!(y < 0.0, "y = {y}");
        let (x, _) = mapping.to_px(2.5, 750.0);
        assert!(x > 100.0, "x = {x}");
    }
}

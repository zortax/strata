//! The interactive CG-envelope polygon editor (design §3.5): the envelope
//! drawn as a live plot (PathBuilder) with draggable vertices, double-click
//! (or the + button) to add a point on a segment, right-click (or the ✕
//! button) to delete one. Deletion below three points is the one blocked
//! mutation — everything else warns, never blocks.
//!
//! The editor is its own entity owned by the profile editor; the manager
//! subscribes to [`EnvelopeEvent`]: `Changed` fires live during a drag
//! (in-memory draft update only), `Committed` on release / structural
//! edits (save-to-disk + library broadcast).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Bounds, Context, EventEmitter, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, Styled as _,
    TextAlign, Window, canvas, div, point, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::plot::label::{PlotLabel, Text as PlotText};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use strata_plan::aircraft::EnvelopePoint;

use crate::assets::IconName;

use super::fields::format_num;
use super::plot::{DataRange, EnvelopeMapping, plot_area, ticks};

/// Height of the plot canvas inside the W&B section.
const PLOT_HEIGHT_PX: f32 = 240.;
/// Painted vertex radius (hit radius is larger; see [`super::plot`]).
const VERTEX_RADIUS_PX: f32 = 4.5;

/// Events the manager view subscribes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EnvelopeEvent {
    /// A vertex moved (drag in progress) — update the draft in memory.
    Changed,
    /// A mutation finished (drag released, vertex added/removed) — persist.
    Committed,
}

struct Drag {
    index: usize,
    /// Data window frozen at drag start, so dragging a boundary vertex
    /// does not rescale the plot under the cursor.
    range: DataRange,
}

/// The envelope editor entity. Owns a working copy of the polygon; the
/// parent reads it back on events.
pub(super) struct EnvelopeEditor {
    points: Vec<EnvelopePoint>,
    selected: Option<usize>,
    hovered: Option<usize>,
    drag: Option<Drag>,
    /// Canvas bounds captured at prepaint (window coordinates) — the
    /// mouse handlers hit-test against these.
    bounds: Bounds<Pixels>,
}

impl EventEmitter<EnvelopeEvent> for EnvelopeEditor {}

impl EnvelopeEditor {
    pub fn new(points: Vec<EnvelopePoint>) -> Self {
        Self {
            points,
            selected: None,
            hovered: None,
            drag: None,
            bounds: Bounds::default(),
        }
    }

    pub fn points(&self) -> &[EnvelopePoint] {
        &self.points
    }

    /// The mapping for the current frame: frozen range while dragging.
    fn mapping(&self) -> EnvelopeMapping {
        let range = match &self.drag {
            Some(drag) => drag.range,
            None => DataRange::around(&self.points),
        };
        EnvelopeMapping::new(plot_area(self.bounds), range)
    }

    // --- mutations -----------------------------------------------------------

    fn insert_point(&mut self, index: usize, point: EnvelopePoint, cx: &mut Context<Self>) {
        let index = index.min(self.points.len());
        self.points.insert(index, point);
        self.selected = Some(index);
        cx.emit(EnvelopeEvent::Committed);
        cx.notify();
    }

    /// Deletes vertex `index`; refused (with a toast) at three points —
    /// the envelope must stay a polygon.
    fn delete_point(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.points.len() <= 3 {
            window.push_notification(
                (
                    gpui_component::notification::NotificationType::Warning,
                    "The CG envelope needs at least three points.",
                ),
                cx,
            );
            return;
        }
        if index >= self.points.len() {
            return;
        }
        self.points.remove(index);
        self.selected = None;
        self.hovered = None;
        cx.emit(EnvelopeEvent::Committed);
        cx.notify();
    }

    /// The + button: split the segment *after* the selected vertex (or the
    /// longest segment when nothing is selected) at its midpoint.
    fn add_midpoint(&mut self, cx: &mut Context<Self>) {
        let n = self.points.len();
        if n < 2 {
            return;
        }
        let segment = match self.selected {
            Some(i) if i < n => i,
            _ => {
                // Longest segment in data space (the plot is roughly
                // proportional; exactness doesn't matter for a seed point).
                (0..n)
                    .max_by(|&a, &b| {
                        let len = |i: usize| {
                            let p = self.points[i];
                            let q = self.points[(i + 1) % n];
                            let da = p.arm.0 - q.arm.0;
                            // Normalize by typical spans so arm meters and
                            // mass kilograms weigh comparably.
                            let dm = (p.mass.0 - q.mass.0) / 1000.0;
                            da * da + dm * dm
                        };
                        len(a).total_cmp(&len(b))
                    })
                    .unwrap_or(0)
            }
        };
        let p = self.points[segment];
        let q = self.points[(segment + 1) % n];
        let midpoint = EnvelopePoint {
            arm: strata_data::domain::Meters((p.arm.0 + q.arm.0) / 2.0),
            mass: strata_plan::units::Kilograms((p.mass.0 + q.mass.0) / 2.0),
        };
        self.insert_point(segment + 1, midpoint, cx);
    }

    // --- mouse ---------------------------------------------------------------

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mapping = self.mapping();
        if let Some(index) = mapping.hit_vertex(event.position, &self.points) {
            self.selected = Some(index);
            self.drag = Some(Drag {
                index,
                range: mapping.range,
            });
            cx.notify();
            return;
        }
        if event.click_count >= 2
            && let Some((insert, point)) = mapping.hit_segment(event.position, &self.points)
        {
            // The fresh vertex is immediately draggable — the press that
            // created it keeps holding it.
            self.insert_point(insert, point, cx);
            self.drag = Some(Drag {
                index: insert,
                range: mapping.range,
            });
            return;
        }
        if self.selected.take().is_some() {
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = &self.drag {
            if event.pressed_button == Some(MouseButton::Left) {
                let index = drag.index;
                let mapping = self.mapping();
                if index < self.points.len() {
                    let target = mapping.to_data(event.position);
                    if self.points[index] != target {
                        self.points[index] = target;
                        cx.emit(EnvelopeEvent::Changed);
                        cx.notify();
                    }
                }
            } else {
                // The button was released outside the plot (mouse-up never
                // reached us): treat re-entry as the release.
                self.drag = None;
                cx.emit(EnvelopeEvent::Committed);
                cx.notify();
            }
            return;
        }
        let hovered = self.mapping().hit_vertex(event.position, &self.points);
        if hovered != self.hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.emit(EnvelopeEvent::Committed);
            cx.notify();
        }
    }

    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.mapping().hit_vertex(event.position, &self.points) {
            self.delete_point(index, window, cx);
        }
    }

    // --- painting ------------------------------------------------------------

    /// Paints grid, axis labels, the polygon and its vertices. Everything
    /// is derived from the captured `bounds` + the same mapping the mouse
    /// handlers use, so picture and hit tests cannot drift apart.
    fn paint_plot(&self, window: &mut Window, cx: &mut gpui::App) {
        let theme = cx.theme();
        let grid_color = theme.border.opacity(0.5);
        let label_color = theme.muted_foreground;
        let accent = theme.primary;

        let mapping = self.mapping();
        let area = mapping.area;
        let range = mapping.range;

        // Grid + tick labels.
        let mut labels: Vec<PlotText> = Vec::new();
        for arm in ticks(range.arm_min, range.arm_max, 6) {
            let x = mapping
                .to_px(EnvelopePoint {
                    arm: strata_data::domain::Meters(arm),
                    mass: strata_plan::units::Kilograms(range.mass_min),
                })
                .x;
            window.paint_quad(gpui::fill(
                Bounds::new(point(x, area.origin.y), size(px(1.), area.size.height)),
                grid_color,
            ));
            labels.push(
                PlotText::new(
                    format!("{} m", format_num(arm, 2)),
                    point(
                        (x - self.bounds.origin.x).floor(),
                        (area.origin.y + area.size.height + px(6.) - self.bounds.origin.y).floor(),
                    ),
                    label_color,
                )
                .align(TextAlign::Center),
            );
        }
        for mass in ticks(range.mass_min, range.mass_max, 5) {
            let y = mapping
                .to_px(EnvelopePoint {
                    arm: strata_data::domain::Meters(range.arm_min),
                    mass: strata_plan::units::Kilograms(mass),
                })
                .y;
            window.paint_quad(gpui::fill(
                Bounds::new(point(area.origin.x, y), size(area.size.width, px(1.))),
                grid_color,
            ));
            labels.push(
                PlotText::new(
                    format_num(mass, 0),
                    point(
                        (area.origin.x - px(8.) - self.bounds.origin.x).floor(),
                        (y - px(6.) - self.bounds.origin.y).floor(),
                    ),
                    label_color,
                )
                .align(TextAlign::Right),
            );
        }

        // The polygon: translucent fill + stroke.
        if self.points.len() >= 2 {
            let px_points: Vec<_> = self.points.iter().map(|p| mapping.to_px(*p)).collect();
            if self.points.len() >= 3 {
                let mut fill = gpui::PathBuilder::fill();
                fill.add_polygon(&px_points, true);
                if let Ok(path) = fill.build() {
                    window.paint_path(path, accent.opacity(0.12));
                }
            }
            let mut stroke = gpui::PathBuilder::stroke(px(1.5));
            stroke.add_polygon(&px_points, true);
            if let Ok(path) = stroke.build() {
                window.paint_path(path, accent.opacity(0.9));
            }
        }

        // Vertices (selected ring > hovered > plain).
        for (i, p) in self.points.iter().enumerate() {
            let center = mapping.to_px(*p);
            let radius = if Some(i) == self.selected || Some(i) == self.hovered {
                VERTEX_RADIUS_PX + 1.5
            } else {
                VERTEX_RADIUS_PX
            };
            let bounds = Bounds::new(
                point(center.x - px(radius), center.y - px(radius)),
                size(px(radius * 2.), px(radius * 2.)),
            );
            let (fill_color, ring) = if Some(i) == self.selected {
                (theme.background, accent)
            } else {
                (accent, theme.background)
            };
            window.paint_quad(
                gpui::fill(bounds, fill_color)
                    .corner_radii(px(radius))
                    .border_widths(px(1.5))
                    .border_color(ring),
            );
        }

        PlotLabel::new(labels).paint(&self.bounds, window, cx);
    }
}

impl Render for EnvelopeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected.and_then(|i| self.points.get(i).copied());
        let selected_label = selected.map(|p| {
            format!(
                "{} m · {} kg",
                format_num(p.arm.0, 3),
                format_num(p.mass.0, 0)
            )
        });
        let delete_disabled = self.selected.is_none() || self.points.len() <= 3;

        let prepaint_view = cx.entity().downgrade();
        let paint_view = cx.entity();
        let plot_canvas = canvas(
            move |bounds, _window, cx| {
                prepaint_view
                    .update(cx, |this, _| this.bounds = bounds)
                    .ok();
            },
            move |_bounds, (), window, cx| {
                paint_view.update(cx, |this, cx| this.paint_plot(window, cx));
            },
        )
        .size_full();

        v_flex()
            .gap_2()
            .w_full()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                "Drag vertices · double-click a segment to add a point · \
                                 right-click a vertex to delete",
                            ),
                    )
                    .children(selected_label.map(|text| {
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.foreground)
                            .child(text)
                    }))
                    .child(
                        Button::new("envelope-add")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Add a point on the selected (or longest) segment")
                            .on_click(cx.listener(|this, _, _, cx| this.add_midpoint(cx))),
                    )
                    .child(
                        Button::new("envelope-delete")
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .disabled(delete_disabled)
                            .tooltip("Delete the selected point")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(index) = this.selected {
                                    this.delete_point(index, window, cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("envelope-plot")
                    .w_full()
                    .h(px(PLOT_HEIGHT_PX))
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted.opacity(0.15))
                    .when(self.drag.is_some(), |el| el.cursor_grabbing())
                    .when(self.drag.is_none() && self.hovered.is_some(), |el| {
                        el.cursor_grab()
                    })
                    .child(plot_canvas)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up)),
            )
    }
}

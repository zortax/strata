//! The drawer's Nav Log tab (design §3.3 "Nav Log"): the PLOG as a proper
//! wide table over [`ComputedFlight::navlog`] — WPT | TT | MT | Wind |
//! WCA | MH | TAS | GS | Dist | ETE | ETA | Alt | Fuel leg/Σ/rem | Freq |
//! Notes. Monospaced numerics, TOC/TOD pseudo-rows styled subtly, the
//! totals row pinned below the scrolling body, horizontal scroll at
//! narrow widths, em-dashes wherever a value is not computable.
//!
//! Notes are the one editable column: they live on the **document**
//! ([`RouteWaypoint::notes`]) and route through
//! [`AppState::set_waypoint_notes`]'s notes-only fast path — dirty
//! tracking flows like any edit, but no recompute is scheduled (notes
//! feed no computed output; the inputs here render from the document, so
//! nothing ever waits on a compute). [`NotesInputs`] keeps one input
//! entity per route waypoint in lockstep with the route.
//!
//! [`ComputedFlight::navlog`]: strata_plan::compute::ComputedFlight
//! [`RouteWaypoint::notes`]: strata_plan::flight::RouteWaypoint::notes
//! [`AppState::set_waypoint_notes`]: crate::state::AppState::set_waypoint_notes

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use strata_data::domain::Frequency;
use strata_plan::flight::{PlannedAltitude, RouteWaypoint};
use strata_plan::navlog::{NavLog, NavLogRow, NavLogRowKind};
use strata_plan::sources::WindsAloft;
use strata_plan::units::{DegreesTrue, Knots, Liters, Minutes, NauticalMiles};

use crate::ui::flight_panel::model;

use super::ProfileDrawer;

/// The shared "not computable" placeholder.
pub const EM_DASH: &str = "—";

// --- column layout -----------------------------------------------------------------

/// One table column: header label and fixed width (`None` = the flexible
/// Notes column).
struct Column {
    label: &'static str,
    width_px: Option<f32>,
    numeric: bool,
}

const fn col(label: &'static str, width_px: f32) -> Column {
    Column {
        label,
        width_px: Some(width_px),
        numeric: true,
    }
}

/// Column order and widths (logical px). "Fuel" is the leg burn, "Σ" the
/// cumulative burn, "Rem" the fuel remaining on board.
const COLUMNS: &[Column] = &[
    Column {
        label: "WPT",
        width_px: Some(108.),
        numeric: false,
    },
    col("TT", 48.),
    col("MT", 48.),
    col("Wind", 64.),
    col("WCA", 48.),
    col("MH", 48.),
    col("TAS", 44.),
    col("GS", 44.),
    col("Dist", 52.),
    col("ETE", 52.),
    col("ETA", 58.),
    col("Alt", 72.),
    col("Fuel", 48.),
    col("Σ", 48.),
    col("Rem", 52.),
    Column {
        label: "Freq",
        width_px: Some(104.),
        numeric: false,
    },
    Column {
        label: "Notes",
        width_px: None,
        numeric: false,
    },
];

/// Minimum width of the flexible Notes column.
const NOTES_MIN_WIDTH_PX: f32 = 180.;
/// Horizontal cell padding.
const CELL_PX: f32 = 6.;

/// Minimum table width before the horizontal scrollbar engages.
fn min_table_width_px() -> f32 {
    COLUMNS
        .iter()
        .map(|c| c.width_px.unwrap_or(NOTES_MIN_WIDTH_PX) + 2. * CELL_PX)
        .sum()
}

// --- pure formatting (unit-tested) ------------------------------------------------

/// Three-digit true track (`083°`), em-dash when absent.
pub fn fmt_true_track(track: Option<DegreesTrue>) -> String {
    track.map_or_else(
        || EM_DASH.to_owned(),
        |t| {
            let deg = (t.0.round() as i64).rem_euclid(360);
            let deg = if deg == 0 { 360 } else { deg };
            format!("{deg:03}°")
        },
    )
}

/// `ddd/ss` wind (`240/15`), the 360-for-north convention.
pub fn fmt_wind(wind: Option<&WindsAloft>) -> String {
    wind.map_or_else(
        || EM_DASH.to_owned(),
        |w| {
            let dir = (w.direction.0.round() as i64).rem_euclid(360);
            let dir = if dir == 0 { 360 } else { dir };
            format!("{dir:03}/{:02.0}", w.speed.0.max(0.0))
        },
    )
}

/// Signed wind-correction angle (`+4°` / `-3°`; positive = right).
pub fn fmt_wca(wca: Option<f64>) -> String {
    wca.map_or_else(
        || EM_DASH.to_owned(),
        |deg| {
            let rounded = deg.round() as i64;
            match rounded {
                0 => "0°".to_owned(),
                d if d > 0 => format!("+{d}°"),
                d => format!("{d}°"),
            }
        },
    )
}

/// Whole knots, em-dash when absent.
pub fn fmt_speed(speed: Option<Knots>) -> String {
    speed.map_or_else(|| EM_DASH.to_owned(), model::fmt_knots)
}

/// Distance in NM (the panel's compact convention), em-dash when absent.
pub fn fmt_distance(distance: Option<NauticalMiles>) -> String {
    distance.map_or_else(|| EM_DASH.to_owned(), model::fmt_nm)
}

/// `H:MM` ETE, em-dash when absent.
pub fn fmt_ete(ete: Option<Minutes>) -> String {
    ete.map_or_else(|| EM_DASH.to_owned(), model::fmt_minutes)
}

/// `14:25Z` ETA, em-dash without a departure time.
pub fn fmt_eta(eta: Option<chrono::DateTime<chrono::Utc>>) -> String {
    eta.map_or_else(|| EM_DASH.to_owned(), model::fmt_eta)
}

/// `3500 ft` / `FL95`, em-dash when unplanned.
pub fn fmt_altitude(altitude: Option<PlannedAltitude>) -> String {
    altitude.map_or_else(|| EM_DASH.to_owned(), model::altitude_label)
}

/// Fuel in liters, one decimal (the unit lives in the column header).
pub fn fmt_fuel(fuel: Option<Liters>) -> String {
    fuel.map_or_else(|| EM_DASH.to_owned(), |l| format!("{:.1}", l.0))
}

/// The suggested frequency (`119.905 MHz`), em-dash when none applies.
pub fn fmt_frequency(frequency: Option<&Frequency>) -> String {
    frequency.map_or_else(|| EM_DASH.to_owned(), |f| f.frequency.to_string())
}

/// Maps nav-log rows onto route indices for the Notes column: the n-th
/// waypoint row edits `doc.route[n]`'s notes. When the waypoint-row count
/// does not match the route (a stale compute during the debounce window),
/// every row maps to `None` and notes render read-only — never against
/// the wrong waypoint.
pub fn waypoint_row_indices(rows: &[NavLogRow], route_len: usize) -> Vec<Option<usize>> {
    let waypoint_rows = rows
        .iter()
        .filter(|r| r.kind == NavLogRowKind::Waypoint)
        .count();
    if waypoint_rows != route_len {
        return vec![None; rows.len()];
    }
    let mut next = 0usize;
    rows.iter()
        .map(|row| match row.kind {
            NavLogRowKind::Waypoint => {
                let index = next;
                next += 1;
                Some(index)
            }
            NavLogRowKind::TopOfClimb | NavLogRowKind::TopOfDescent => None,
        })
        .collect()
}

// --- notes inputs --------------------------------------------------------------------

/// One notes input per route waypoint, kept in lockstep with the open
/// document (rebuilt on length changes, text-synced otherwise — focused
/// inputs are never overwritten, the user is typing).
pub(super) struct NotesInputs {
    inputs: Vec<Entity<InputState>>,
    subscriptions: Vec<Subscription>,
}

impl NotesInputs {
    pub(super) fn new() -> Self {
        Self {
            inputs: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    pub(super) fn input(&self, route_index: usize) -> Option<&Entity<InputState>> {
        self.inputs.get(route_index)
    }

    /// Brings the inputs in line with `route` (called on every flight
    /// event while the drawer is mounted).
    pub(super) fn sync(
        &mut self,
        route: &[RouteWaypoint],
        window: &mut Window,
        cx: &mut Context<ProfileDrawer>,
    ) {
        use gpui::AppContext as _;
        if self.inputs.len() != route.len() {
            self.subscriptions.clear();
            self.inputs.clear();
            for index in 0..route.len() {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("…"));
                self.subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this: &mut ProfileDrawer, input, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            let notes = input.read(cx).value().to_string();
                            this.app_state.update(cx, |state, cx| {
                                state.set_waypoint_notes(index, notes, cx);
                            });
                        }
                    },
                ));
                self.inputs.push(input);
            }
        }
        for (input, waypoint) in self.inputs.iter().zip(route) {
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue; // the user is typing; the doc already follows
            }
            if input.read(cx).value().as_ref() != waypoint.notes {
                let text = waypoint.notes.clone();
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    /// Drops every entity once the drawer unmounted.
    pub(super) fn clear(&mut self) {
        self.subscriptions.clear();
        self.inputs.clear();
    }
}

// --- rendering -----------------------------------------------------------------------

/// The tab body: the PLOG table, or a hint card while nothing is
/// computed.
pub(super) fn render_navlog_tab(
    drawer: &ProfileDrawer,
    cx: &mut Context<ProfileDrawer>,
) -> AnyElement {
    let state = drawer.app_state.read(cx);
    let Some(flight) = &state.flight else {
        return div().into_any_element(); // exit animation: empty frame
    };
    let route_len = flight.doc.route.len();
    let Some(computed) = flight.computed.clone() else {
        let hint = match &flight.compute_state {
            crate::state::ComputeState::Pending => "Computing…".to_owned(),
            crate::state::ComputeState::Computed => String::new(),
            crate::state::ComputeState::NotComputable(reason) => format!("{reason}."),
            crate::state::ComputeState::Failed(error) => format!("Compute failed: {error}"),
        };
        return v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("The nav log appears once the flight computes."),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                    .child(hint),
            )
            .into_any_element();
    };

    let navlog = computed.navlog.clone();
    let indices = waypoint_row_indices(&navlog.rows, route_len);
    let mono = cx.theme().mono_font_family.clone();

    let body_rows: Vec<AnyElement> = navlog
        .rows
        .iter()
        .zip(&indices)
        .map(|(row, route_index)| render_row(drawer, row, *route_index, cx))
        .collect();

    div()
        .id("navlog-hscroll")
        .size_full()
        .overflow_x_scroll()
        .child(
            v_flex()
                .h_full()
                .w_full()
                .min_w(px(min_table_width_px()))
                .font_family(mono)
                .text_xs()
                .child(render_header(cx))
                .child(
                    div()
                        .id("navlog-vscroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(v_flex().children(body_rows)),
                )
                .child(render_totals(&navlog, cx)),
        )
        .into_any_element()
}

/// A fixed-width cell; numeric cells right-align.
fn cell(content: impl IntoElement, width_px: f32, numeric: bool) -> gpui::Div {
    let cell = h_flex()
        .w(px(width_px + 2. * CELL_PX))
        .flex_shrink_0()
        .px(px(CELL_PX))
        .overflow_hidden();
    if numeric {
        cell.justify_end().child(content)
    } else {
        cell.child(content)
    }
}

/// The flexible trailing Notes cell.
fn notes_cell(content: impl IntoElement) -> gpui::Div {
    h_flex()
        .flex_1()
        .min_w(px(NOTES_MIN_WIDTH_PX + 2. * CELL_PX))
        .px(px(CELL_PX))
        .child(content)
}

fn render_header(cx: &Context<ProfileDrawer>) -> AnyElement {
    let mut header = h_flex()
        .flex_shrink_0()
        .items_center()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().border)
        .text_color(cx.theme().muted_foreground)
        .font_weight(FontWeight::SEMIBOLD);
    for column in COLUMNS {
        header = match column.width_px {
            Some(width) => header.child(cell(column.label, width, column.numeric)),
            None => header.child(notes_cell(column.label)),
        };
    }
    header.into_any_element()
}

fn render_row(
    drawer: &ProfileDrawer,
    row: &NavLogRow,
    route_index: Option<usize>,
    cx: &Context<ProfileDrawer>,
) -> AnyElement {
    let pseudo = row.kind != NavLogRowKind::Waypoint;

    let values = [
        fmt_true_track(row.true_track),
        // MT/MH share TT's three-digit convention via the panel helper.
        row.magnetic_track
            .map_or_else(|| EM_DASH.to_owned(), model::fmt_heading),
        fmt_wind(row.wind.as_ref()),
        fmt_wca(row.wind_correction_angle_deg),
        row.magnetic_heading
            .map_or_else(|| EM_DASH.to_owned(), model::fmt_heading),
        fmt_speed(row.tas),
        fmt_speed(row.ground_speed),
        fmt_distance(row.distance),
        fmt_ete(row.ete),
        fmt_eta(row.eta),
        fmt_altitude(row.altitude),
        fmt_fuel(row.leg_fuel),
        fmt_fuel(row.cumulative_fuel),
        fmt_fuel(row.remaining_fuel),
    ];

    let mut element = h_flex()
        .items_center()
        .py_0p5()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.4))
        .when(pseudo, |el| {
            // TOC/TOD pseudo-rows: present but deliberately subdued.
            el.text_color(cx.theme().muted_foreground).italic()
        })
        .child(cell(
            div()
                .when(!pseudo, |el| el.font_weight(FontWeight::SEMIBOLD))
                .truncate()
                .child(row.label.clone()),
            108.,
            false,
        ));
    for (value, column) in values.into_iter().zip(&COLUMNS[1..15]) {
        element = element.child(cell(
            value,
            column.width_px.unwrap_or(NOTES_MIN_WIDTH_PX),
            column.numeric,
        ));
    }
    element = element.child(cell(
        div()
            .truncate()
            .child(fmt_frequency(row.frequency.as_ref())),
        104.,
        false,
    ));

    // Notes: editable for waypoint rows (persisted on the doc per leg);
    // TOC/TOD rows and stale computes show the row's copy read-only.
    let notes: AnyElement = match route_index.and_then(|i| drawer.notes.input(i)) {
        Some(input) => Input::new(input)
            .xsmall()
            .appearance(false)
            .into_any_element(),
        None => div()
            .text_color(cx.theme().muted_foreground)
            .truncate()
            .child(if row.notes.is_empty() {
                if pseudo {
                    EM_DASH.to_owned()
                } else {
                    String::new()
                }
            } else {
                row.notes.clone()
            })
            .into_any_element(),
    };
    element.child(notes_cell(notes)).into_any_element()
}

/// The pinned totals row (outside the vertical scroller, inside the
/// horizontal one, so columns stay aligned at every scroll offset).
fn render_totals(navlog: &NavLog, cx: &Context<ProfileDrawer>) -> AnyElement {
    let blank = |width: f32, numeric: bool| cell(String::new(), width, numeric);
    h_flex()
        .flex_shrink_0()
        .items_center()
        .py_1()
        .border_t_1()
        .border_color(cx.theme().border)
        .font_weight(FontWeight::SEMIBOLD)
        .child(cell("Totals", 108., false))
        .children(
            COLUMNS[1..8]
                .iter()
                .map(|c| blank(c.width_px.unwrap_or(NOTES_MIN_WIDTH_PX), c.numeric)),
        )
        .child(cell(model::fmt_nm(navlog.totals.distance), 52., true))
        .child(cell(model::fmt_minutes(navlog.totals.ete), 52., true))
        .child(blank(58., true)) // ETA
        .child(blank(72., true)) // Alt
        .child(cell(format!("{:.1}", navlog.totals.fuel.0), 48., true))
        .child(blank(48., true)) // Σ
        .child(blank(52., true)) // Rem
        .child(blank(104., false)) // Freq
        .child(notes_cell(String::new()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use strata_data::domain::MetersAmsl;
    use strata_plan::units::Celsius;

    use super::*;

    #[test]
    fn headings_and_tracks_use_the_three_digit_360_convention() {
        assert_eq!(fmt_true_track(Some(DegreesTrue::new(83.4))), "083°");
        assert_eq!(fmt_true_track(Some(DegreesTrue::new(0.2))), "360°");
        assert_eq!(fmt_true_track(Some(DegreesTrue::new(359.7))), "360°");
        assert_eq!(fmt_true_track(None), EM_DASH);
    }

    #[test]
    fn wind_formats_as_direction_over_speed() {
        let wind = WindsAloft {
            direction: DegreesTrue::new(240.0),
            speed: Knots(15.4),
            temperature: Celsius(2.0),
            temperature_provenance: strata_plan::sources::Provenance::Real,
        };
        assert_eq!(fmt_wind(Some(&wind)), "240/15");
        let north = WindsAloft {
            direction: DegreesTrue::new(0.0),
            speed: Knots(4.0),
            temperature: Celsius(2.0),
            temperature_provenance: strata_plan::sources::Provenance::Isa,
        };
        assert_eq!(fmt_wind(Some(&north)), "360/04");
        assert_eq!(fmt_wind(None), EM_DASH);
    }

    #[test]
    fn wca_is_signed_and_zero_is_unsigned() {
        assert_eq!(fmt_wca(Some(4.4)), "+4°");
        assert_eq!(fmt_wca(Some(-2.6)), "-3°");
        assert_eq!(fmt_wca(Some(0.2)), "0°");
        assert_eq!(fmt_wca(None), EM_DASH);
    }

    #[test]
    fn ete_and_eta_use_the_panel_conventions() {
        assert_eq!(fmt_ete(Some(Minutes(7.4))), "0:07");
        assert_eq!(fmt_ete(Some(Minutes(95.0))), "1:35");
        assert_eq!(fmt_ete(None), EM_DASH);

        let eta = Utc.with_ymd_and_hms(2026, 6, 14, 14, 25, 31).unwrap();
        assert_eq!(fmt_eta(Some(eta)), "14:25Z");
        assert_eq!(fmt_eta(None), EM_DASH);
    }

    #[test]
    fn not_computable_values_are_em_dashes_throughout() {
        assert_eq!(fmt_speed(None), EM_DASH);
        assert_eq!(fmt_distance(None), EM_DASH);
        assert_eq!(fmt_altitude(None), EM_DASH);
        assert_eq!(fmt_fuel(None), EM_DASH);
        assert_eq!(fmt_frequency(None), EM_DASH);

        assert_eq!(fmt_speed(Some(Knots(95.4))), "95");
        assert_eq!(fmt_distance(Some(NauticalMiles(38.62))), "38.6");
        assert_eq!(
            fmt_altitude(Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(3500.0)))),
            "3500 ft"
        );
        assert_eq!(fmt_altitude(Some(PlannedAltitude::FlightLevel(95))), "FL95");
        assert_eq!(fmt_fuel(Some(Liters(12.34))), "12.3");
    }

    fn row(kind: NavLogRowKind) -> NavLogRow {
        NavLogRow {
            kind,
            label: "x".into(),
            altitude: None,
            true_track: None,
            magnetic_track: None,
            wind: None,
            wind_correction_angle_deg: None,
            magnetic_heading: None,
            tas: None,
            ground_speed: None,
            distance: None,
            ete: None,
            eta: None,
            leg_fuel: None,
            cumulative_fuel: None,
            remaining_fuel: None,
            frequency: None,
            notes: String::new(),
        }
    }

    #[test]
    fn waypoint_rows_map_onto_route_indices_in_order() {
        use NavLogRowKind::*;
        let rows = [
            row(Waypoint),
            row(TopOfClimb),
            row(Waypoint),
            row(TopOfDescent),
            row(Waypoint),
        ];
        assert_eq!(
            waypoint_row_indices(&rows, 3),
            vec![Some(0), None, Some(1), None, Some(2)]
        );
    }

    #[test]
    fn stale_computes_disable_the_notes_mapping_entirely() {
        use NavLogRowKind::*;
        let rows = [row(Waypoint), row(Waypoint), row(Waypoint)];
        // The doc gained a waypoint the navlog has not seen yet: no row may
        // edit a waypoint it might mis-address.
        assert_eq!(waypoint_row_indices(&rows, 4), vec![None, None, None]);
        assert_eq!(waypoint_row_indices(&rows, 2), vec![None, None, None]);
        assert_eq!(waypoint_row_indices(&[], 0), Vec::<Option<usize>>::new());
    }

    #[test]
    fn fixed_columns_line_up_with_the_row_renderer() {
        // `render_row` zips 14 formatted values against COLUMNS[1..15] and
        // hardcodes the WPT/Freq/Notes widths; keep the table honest.
        assert_eq!(COLUMNS.len(), 17);
        assert_eq!(COLUMNS[0].label, "WPT");
        assert_eq!(COLUMNS[0].width_px, Some(108.));
        assert_eq!(COLUMNS[15].label, "Freq");
        assert_eq!(COLUMNS[15].width_px, Some(104.));
        assert_eq!(COLUMNS[16].label, "Notes");
        assert_eq!(COLUMNS[16].width_px, None);
        assert!(min_table_width_px() > 1000.);
    }
}

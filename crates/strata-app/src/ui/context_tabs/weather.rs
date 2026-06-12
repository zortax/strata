//! The Weather tab (design §3.4 "Weather"): METAR/TAF cards for the
//! route's named airports (departure / destination / alternates), the
//! per-leg winds-aloft summary from the computed [`LegWind`] data, the
//! freezing-level readout, and SIGMETs whose area intersects the route
//! corridor bbox. Snapshot timestamp + refresh ride the existing
//! live-weather path (`AppState::refresh_weather`).
//!
//! # Freezing-level fallback chain (documented, labelled)
//!
//! [`freezing_level_readout`] prefers, in order: the fetched ICON
//! `hzerocl` 0 °C-isotherm grid; the 0 °C crossing interpolated from the
//! fetched per-level temperatures (both via
//! [`WindsAloftFrames::freezing_level`]); the standard-lapse extrapolation
//! from the legs' sampled OATs ([`estimated_freezing_level_feet`]), which
//! is "from forecast temperatures" when those OATs are real and the
//! honest "ISA estimate — no forecast data" otherwise. Every rung carries
//! its label; nothing renders as more authoritative than it is.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, ParentElement as _, Styled as _, div,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use strata_data::domain::{BoundingBox, IcaoCode, Sigmet};
use strata_plan::compute::ComputedFlight;
use strata_plan::flight::{FlightDoc, NamedPointKind, RoutePoint};
use strata_plan::perf::planned_altitude_amsl;
use strata_plan::sources::Provenance;
use strata_plan::units::METERS_PER_NAUTICAL_MILE;
use strata_plan::wind::LegWindOrigin;
use strata_render::MapTheme;

use crate::sources::{FreezingLevelSource, WindsAloftFrames, points_prefetch_bbox};
use crate::ui::info_panel::{badge, card, kv, metar_rows, section, taf_rows};

use super::{ContextPanel, FlightView};

/// Lateral margin of the SIGMET relevance bbox around the route — the
/// default profile corridor half-width (±5 NM), the same "what the plan
/// considered" notion the corridor engine uses.
const SIGMET_CORRIDOR_MARGIN_M: f64 = 5.0 * METERS_PER_NAUTICAL_MILE;

/// ICAO standard lapse rate, meters of altitude per °C.
const METERS_PER_DEGREE_C: f64 = 1000.0 / 6.5;

// --- pure helpers ---------------------------------------------------------------

/// Role of a weather station relative to the flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StationRole {
    Departure,
    Destination,
    Alternate,
}

impl StationRole {
    pub fn label(self) -> &'static str {
        match self {
            StationRole::Departure => "Departure",
            StationRole::Destination => "Destination",
            StationRole::Alternate => "Alternate",
        }
    }
}

/// A weather station resolved from the route.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WeatherStation {
    pub role: StationRole,
    pub icao: String,
    pub name: String,
}

/// The route's named airports in briefing order: departure (first
/// waypoint), destination (last waypoint, two-plus-point routes), then the
/// alternates. Free points and non-airport features carry no station —
/// they are skipped, not guessed.
pub(crate) fn route_weather_stations(doc: &FlightDoc) -> Vec<WeatherStation> {
    let mut stations = Vec::new();
    let mut push = |role: StationRole, point: &RoutePoint| {
        if let RoutePoint::Named(named) = point
            && named.kind == NamedPointKind::Airport
        {
            stations.push(WeatherStation {
                role,
                icao: named.id.clone(),
                name: named.name.clone(),
            });
        }
    };
    if let Some(first) = doc.route.first() {
        push(StationRole::Departure, &first.point);
    }
    if doc.route.len() >= 2
        && let Some(last) = doc.route.last()
    {
        push(StationRole::Destination, &last.point);
    }
    for alternate in &doc.alternates {
        push(StationRole::Alternate, alternate);
    }
    stations
}

/// Freezing level estimated from the computed legs' winds-aloft
/// temperatures: per leg, the 0 °C altitude extrapolated from the sampled
/// OAT at planned altitude with the ICAO standard lapse rate, averaged
/// across legs. `None` without winds data. An estimate by construction —
/// the last rung of the labelled chain in [`freezing_level_readout`].
pub(crate) fn estimated_freezing_level_feet(
    winds: &[strata_plan::wind::LegWind],
    doc: &FlightDoc,
) -> Option<f64> {
    let mut sum_m = 0.0;
    let mut count = 0usize;
    for wind in winds {
        let altitude = doc
            .route
            .get(wind.leg_index)
            .and_then(|w| w.leg_altitude)
            .or(doc.cruise_altitude)?;
        let altitude_m = planned_altitude_amsl(altitude).0;
        sum_m += altitude_m + wind.wind.temperature.0 * METERS_PER_DEGREE_C;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let mean_m = (sum_m / count as f64).max(0.0);
    Some(mean_m * strata_data::domain::FEET_PER_METER)
}

/// A freezing-level readout: the value plus the honest label of the chain
/// rung that produced it (rendered as the caption in the Weather tab and
/// baked into the briefing PDF string).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FreezingLevelReadout {
    pub feet: f64,
    /// Lower-case source phrase, e.g. `"ISA estimate — no forecast data"`.
    pub source_label: &'static str,
}

/// The documented freezing-level fallback chain (module docs): `hzerocl`
/// grid → real-temperature interpolation → labelled lapse-rate estimate
/// from the leg OATs (which is itself an ISA estimate when no temperature
/// data backed those OATs). `None` only when nothing freezing-shaped can
/// be derived at all (no winds computed).
pub(crate) fn freezing_level_readout(
    frames: &WindsAloftFrames,
    doc: &FlightDoc,
    computed: &ComputedFlight,
) -> Option<FreezingLevelReadout> {
    // Rungs 1+2 sample the frames at the legs' midpoints, mid-flight (the
    // nearest-step rule makes finer per-leg timing pointless).
    if let Some(departure) = doc.departure_time {
        let mid_flight = departure
            + chrono::Duration::milliseconds(
                (computed.phases.total_duration.0 * 60_000.0 / 2.0) as i64,
            );
        let samples: Vec<(f64, FreezingLevelSource)> = computed
            .legs
            .iter()
            .filter_map(|leg| frames.freezing_level(leg.midpoint, mid_flight))
            .map(|(level, source)| (level.0, source))
            .collect();
        if !samples.is_empty() {
            let mean_m = samples.iter().map(|(m, _)| m).sum::<f64>() / samples.len() as f64;
            // A mix of rungs is labelled by the weaker one — "forecast"
            // never overstates a partially interpolated value.
            let all_forecast = samples
                .iter()
                .all(|(_, source)| *source == FreezingLevelSource::Forecast);
            return Some(FreezingLevelReadout {
                feet: mean_m * strata_data::domain::FEET_PER_METER,
                source_label: if all_forecast {
                    "ICON-D2 0 °C isotherm forecast"
                } else {
                    "interpolated from ICON-D2 forecast temperatures"
                },
            });
        }
    }
    // Rung 3: lapse-rate extrapolation from the legs' sampled OATs — real
    // forecast temperatures where they were, the honest ISA estimate
    // otherwise.
    let feet = estimated_freezing_level_feet(&computed.winds, doc)?;
    let any_real_oat = computed
        .winds
        .iter()
        .any(|w| w.wind.temperature_provenance == Provenance::Real);
    Some(FreezingLevelReadout {
        feet,
        source_label: if any_real_oat {
            "estimated from forecast leg temperatures (standard lapse rate)"
        } else {
            "ISA estimate — no forecast data"
        },
    })
}

/// The route's SIGMET relevance bbox: route + alternates padded by the
/// corridor margin. `None` for an empty route.
pub(crate) fn route_corridor_bbox(doc: &FlightDoc) -> Option<BoundingBox> {
    let points = doc
        .route
        .iter()
        .map(|w| w.position())
        .chain(doc.alternates.iter().map(|p| p.position()));
    points_prefetch_bbox(points, SIGMET_CORRIDOR_MARGIN_M)
}

/// Indices of the SIGMETs whose area bbox intersects the route corridor
/// bbox (bbox-level relevance — honest about being a prefilter, exact
/// polygon overlap is briefing-phase work).
pub(crate) fn sigmets_intersecting(sigmets: &[Sigmet], corridor: BoundingBox) -> Vec<usize> {
    sigmets
        .iter()
        .enumerate()
        .filter(|(_, sigmet)| sigmet.geometry.bounding_box().intersects(&corridor))
        .map(|(index, _)| index)
        .collect()
}

/// "just now" / "3 min ago" / "2 h ago" for the snapshot row.
pub(crate) fn age_label(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else {
        format!("{} h {} min ago", secs / 3600, (secs % 3600) / 60)
    }
}

// --- rendering ------------------------------------------------------------------

pub(super) fn render_weather_tab(
    panel: &ContextPanel,
    flight: &FlightView,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let state = panel.app_state.read(cx);
    let map_theme = MapTheme::by_id(state.map_theme_id).unwrap_or_default();

    let stations = route_weather_stations(&flight.doc);
    let fetched_ago = state.weather.last_fetched_at.map(|at| at.elapsed());
    let fetching = state.weather.fetching;
    let fetch_error = state.weather.last_error.clone();

    // Station cards need the METAR/TAF clones up front (no AppState borrow
    // across listener construction).
    type StationWx = (
        WeatherStation,
        Option<strata_data::domain::Metar>,
        Option<strata_data::domain::Taf>,
    );
    let station_wx: Vec<StationWx> = stations
        .into_iter()
        .map(|station| {
            let wx = IcaoCode::new(&station.icao).ok().map(|icao| {
                (
                    state.metar_for(&icao).cloned(),
                    state.taf_for(&icao).cloned(),
                )
            });
            let (metar, taf) = wx.unwrap_or((None, None));
            (station, metar, taf)
        })
        .collect();

    let sigmet_rows: Vec<Sigmet> = route_corridor_bbox(&flight.doc)
        .map(|corridor| {
            sigmets_intersecting(&state.weather.sigmets, corridor)
                .into_iter()
                .map(|i| state.weather.sigmets[i].clone())
                .collect()
        })
        .unwrap_or_default();

    // The freezing-level chain reads the prefetched frames (owned snapshot
    // — taken while the AppState borrow is live, like the clones above).
    let freezing = flight.computed.as_ref().and_then(|computed| {
        freezing_level_readout(&state.flight_winds_frames(), &flight.doc, computed)
    });

    let mut content = v_flex()
        .gap_3()
        .child(snapshot_row(fetched_ago, fetching, fetch_error, cx));

    if station_wx.is_empty() {
        content = content.child(
            card(cx).child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No named airports on the route yet — weather cards follow the departure / destination / alternates."),
            ),
        );
    }
    for (station, metar, taf) in &station_wx {
        content = content.child(station_card(
            panel,
            station,
            metar.as_ref(),
            taf.as_ref(),
            &map_theme,
            cx,
        ));
    }

    content = content.child(winds_aloft_card(flight, freezing, cx));

    if !sigmet_rows.is_empty() {
        let mut sigmet_card = card(cx).child(section("SIGMET", cx));
        for sigmet in &sigmet_rows {
            sigmet_card = sigmet_card.child(sigmet_row(sigmet, cx));
        }
        content = content.child(sigmet_card);
    }

    content.into_any_element()
}

/// Snapshot timestamp + manual refresh (the existing live-weather path).
fn snapshot_row(
    fetched_ago: Option<Duration>,
    fetching: bool,
    fetch_error: Option<String>,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let label = if fetching {
        "Updating…".to_owned()
    } else {
        match fetched_ago {
            Some(age) => format!("Snapshot {}", age_label(age)),
            None => "No weather data yet".to_owned(),
        }
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    Button::new("wx-refresh")
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::IconName::RefreshCw)
                        .tooltip("Refresh weather")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.app_state
                                .update(cx, |state, cx| state.refresh_weather(cx));
                        })),
                ),
        )
        .children(fetch_error.map(|err| {
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(format!("Last fetch failed: {err}"))
        }))
        .into_any_element()
}

/// One station's METAR/TAF card, reusing the airport card's weather
/// sections.
fn station_card(
    panel: &ContextPanel,
    station: &WeatherStation,
    metar: Option<&strata_data::domain::Metar>,
    taf: Option<&strata_data::domain::Taf>,
    map_theme: &MapTheme,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let mut el = card(cx).child(
        h_flex()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(station.role.label()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(station.name.clone()),
                    ),
            )
            .child(badge(station.icao.clone(), cx)),
    );

    match metar {
        Some(metar) => el = el.children(metar_rows(metar, map_theme, cx)),
        None => {
            el = el.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No METAR for this station."),
            );
        }
    }

    if let Some(taf) = taf {
        let icao = station.icao.clone();
        let expanded = panel.expanded_tafs.contains(&icao);
        let view = cx.entity().downgrade();
        let on_toggle: crate::ui::info_panel::CardCallback = Rc::new(move |_, cx| {
            let icao = icao.clone();
            view.update(cx, |this, cx| {
                if !this.expanded_tafs.remove(&icao) {
                    this.expanded_tafs.insert(icao);
                }
                cx.notify();
            })
            .ok();
        });
        el = el.children(taf_rows(
            taf,
            gpui::SharedString::from(format!("wx-taf-{}", station.icao)),
            expanded,
            on_toggle,
            cx,
        ));
    }

    el.into_any_element()
}

/// Winds aloft per leg from the computed [`LegWind`] data, plus the
/// labelled freezing-level readout.
fn winds_aloft_card(
    flight: &FlightView,
    freezing: Option<FreezingLevelReadout>,
    cx: &mut Context<ContextPanel>,
) -> AnyElement {
    let mut el = card(cx).child(section("Winds aloft", cx));
    let Some(computed) = &flight.computed else {
        return el
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        flight.compute_hint.clone().unwrap_or_else(|| {
                            "Per-leg winds appear once the route computes.".into()
                        }),
                    ),
            )
            .into_any_element();
    };

    for (leg, wind) in computed.legs.iter().zip(computed.winds.iter()) {
        let altitude = flight
            .doc
            .route
            .get(wind.leg_index)
            .and_then(|w| w.leg_altitude)
            .or(flight.doc.cruise_altitude)
            .map(|a| format!("{:.0} ft", planned_altitude_amsl(a).as_feet()))
            .unwrap_or_else(|| "—".to_owned());
        el = el.child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("{} → {}", leg.from, leg.to)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(cx.theme().muted_foreground)
                        .child(altitude),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family("monospace")
                        .child(format!(
                            "{:03.0}°/{:.0} kt {:+.0} °C",
                            wind.wind.direction.0, wind.wind.speed.0, wind.wind.temperature.0
                        )),
                )
                .children(matches!(wind.origin, LegWindOrigin::Manual).then(|| badge("manual", cx)))
                // The calm-ISA fallback is a labelled assumption, never
                // passed off as a sampled wind.
                .children(
                    matches!(wind.origin, LegWindOrigin::IsaFallback).then(|| badge("ISA", cx)),
                ),
        );
    }

    if let Some(readout) = freezing {
        let mut caption: String = readout.source_label.to_owned();
        if let Some(first) = caption.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        caption.push('.');
        el = el
            .child(kv("Freezing lvl", format!("≈ {:.0} ft", readout.feet), cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(caption),
            );
    }
    el.into_any_element()
}

fn sigmet_row(sigmet: &Sigmet, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(badge(format!("{:?}", sigmet.hazard), cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {}–{}Z",
                            sigmet.fir,
                            sigmet.valid_from.format("%H:%M"),
                            sigmet.valid_to.format("%H:%M")
                        )),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_xs()
                .font_family("monospace")
                .text_color(cx.theme().muted_foreground)
                .child(sigmet.raw.clone()),
        )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use strata_data::domain::{LatLon, MetersAmsl, Polygon, SigmetHazard};
    use strata_plan::flight::{FreePoint, NamedPoint, PlannedAltitude, RouteWaypoint};
    use strata_plan::sources::WindsAloft;
    use strata_plan::units::{Celsius, DegreesTrue, Knots};
    use strata_plan::wind::{LegWind, WindTriangle};

    use super::*;

    fn airport(id: &str, name: &str, lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Named(NamedPoint {
            kind: NamedPointKind::Airport,
            id: id.to_owned(),
            name: name.to_owned(),
            position: LatLon::new(lat, lon).unwrap(),
        })
    }

    fn free(lat: f64, lon: f64) -> RoutePoint {
        RoutePoint::Free(FreePoint {
            name: None,
            position: LatLon::new(lat, lon).unwrap(),
        })
    }

    #[test]
    fn stations_resolve_departure_destination_and_alternates() {
        let mut doc = FlightDoc::new("t");
        doc.route = vec![
            RouteWaypoint::new(airport("EDFE", "Egelsbach", 49.96, 8.64)),
            RouteWaypoint::new(free(49.5, 9.0)),
            RouteWaypoint::new(airport("EDQN", "Neustadt/Aisch", 49.58, 10.58)),
        ];
        doc.alternates = vec![airport("EDQF", "Fürth-Seckendorf", 49.5, 10.9)];

        let stations = route_weather_stations(&doc);
        assert_eq!(stations.len(), 3);
        assert_eq!(stations[0].role, StationRole::Departure);
        assert_eq!(stations[0].icao, "EDFE");
        assert_eq!(stations[1].role, StationRole::Destination);
        assert_eq!(stations[1].icao, "EDQN");
        assert_eq!(stations[2].role, StationRole::Alternate);
        assert_eq!(stations[2].icao, "EDQF");
    }

    #[test]
    fn free_endpoints_and_short_routes_yield_no_phantom_stations() {
        let mut doc = FlightDoc::new("t");
        doc.route = vec![RouteWaypoint::new(free(49.0, 9.0))];
        assert!(route_weather_stations(&doc).is_empty());

        // A single airport waypoint is the departure, never also the
        // destination.
        doc.route = vec![RouteWaypoint::new(airport(
            "EDFE",
            "Egelsbach",
            49.96,
            8.64,
        ))];
        let stations = route_weather_stations(&doc);
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].role, StationRole::Departure);

        // Navaids/reporting points are not weather stations.
        doc.route = vec![
            RouteWaypoint::new(RoutePoint::Named(NamedPoint {
                kind: NamedPointKind::Navaid,
                id: "FFM".into(),
                name: "Frankfurt VOR".into(),
                position: LatLon::new(50.05, 8.63).unwrap(),
            })),
            RouteWaypoint::new(free(49.5, 9.0)),
        ];
        assert!(route_weather_stations(&doc).is_empty());
    }

    fn leg_wind(leg_index: usize, temperature_c: f64) -> LegWind {
        leg_wind_with(leg_index, temperature_c, Provenance::Real)
    }

    fn leg_wind_with(leg_index: usize, temperature_c: f64, provenance: Provenance) -> LegWind {
        LegWind {
            leg_index,
            wind: WindsAloft {
                direction: DegreesTrue::new(270.0),
                speed: Knots(15.0),
                temperature: Celsius(temperature_c),
                temperature_provenance: provenance,
            },
            origin: if provenance == Provenance::Real {
                LegWindOrigin::Sampled
            } else {
                LegWindOrigin::IsaFallback
            },
            triangle: WindTriangle {
                wind_correction_angle_deg: 0.0,
                true_heading: DegreesTrue::new(90.0),
                ground_speed: Knots(100.0),
            },
        }
    }

    #[test]
    fn freezing_level_extrapolates_with_the_standard_lapse_rate() {
        let mut doc = FlightDoc::new("t");
        doc.route = vec![
            RouteWaypoint::new(free(49.0, 9.0)),
            RouteWaypoint::new(free(49.0, 10.0)),
            RouteWaypoint::new(free(49.0, 11.0)),
        ];
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(1000.0)));

        // +5 °C at 1000 m → freezing at 1000 + 5 × (1000/6.5) m on both legs.
        let winds = vec![leg_wind(0, 5.0), leg_wind(1, 5.0)];
        let feet = estimated_freezing_level_feet(&winds, &doc).expect("winds present");
        let expected_m = 1000.0 + 5.0 * METERS_PER_DEGREE_C;
        let expected_ft = expected_m * strata_data::domain::FEET_PER_METER;
        assert!((feet - expected_ft).abs() < 1e-6, "{feet} vs {expected_ft}");

        // A leg altitude override participates per leg (average of both).
        doc.route[0].leg_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(2000.0)));
        let feet = estimated_freezing_level_feet(&winds, &doc).expect("winds present");
        let leg0 = 2000.0 + 5.0 * METERS_PER_DEGREE_C;
        let leg1 = 1000.0 + 5.0 * METERS_PER_DEGREE_C;
        let expected_ft = (leg0 + leg1) / 2.0 * strata_data::domain::FEET_PER_METER;
        assert!((feet - expected_ft).abs() < 1e-6, "{feet} vs {expected_ft}");

        // Sub-zero OAT puts the estimate *below* the cruise altitude.
        doc.route[0].leg_altitude = None;
        let cold = vec![leg_wind(0, -4.0), leg_wind(1, -4.0)];
        let feet = estimated_freezing_level_feet(&cold, &doc).expect("winds present");
        assert!(feet < 1000.0 * strata_data::domain::FEET_PER_METER);

        // No winds → no readout; missing altitudes → no readout either.
        assert_eq!(estimated_freezing_level_feet(&[], &doc), None);
        doc.cruise_altitude = None;
        assert_eq!(estimated_freezing_level_feet(&winds, &doc), None);
    }

    // --- the labelled freezing-level chain --------------------------------

    fn wx_grid(
        field: strata_data::domain::WeatherField,
        valid: chrono::DateTime<Utc>,
        value: f32,
    ) -> std::sync::Arc<strata_data::domain::WeatherGrid> {
        std::sync::Arc::new(strata_data::domain::WeatherGrid {
            field,
            run_time: valid,
            valid_time: valid,
            grid: strata_data::domain::RegularLatLonGrid::new(
                LatLon::new(46.0, 5.0).unwrap(),
                10.0,
                10.0,
                2,
                2,
                vec![value; 4],
            )
            .unwrap(),
        })
    }

    /// Frames with one step at `valid`: optionally an hzerocl grid,
    /// optionally real per-level temperatures (20 °C at 950 hPa, −10 °C
    /// per level: the 0 °C crossing sits at the 700 hPa ISA altitude).
    fn frames(
        valid: chrono::DateTime<Utc>,
        hzerocl_m: Option<f32>,
        with_temps: bool,
    ) -> WindsAloftFrames {
        use strata_data::domain::{PressureLevel, WeatherField};
        let levels = PressureLevel::ALL
            .into_iter()
            .enumerate()
            .map(|(i, level)| crate::sources::LevelWinds {
                level,
                u: wx_grid(WeatherField::WindU(level), valid, 5.0),
                v: wx_grid(WeatherField::WindV(level), valid, 0.0),
                temperature: with_temps.then(|| {
                    wx_grid(
                        WeatherField::Temperature(level),
                        valid,
                        20.0 - 10.0 * i as f32,
                    )
                }),
            })
            .collect();
        WindsAloftFrames::new(vec![crate::sources::WindsTimeStep {
            valid_time: valid,
            levels,
            freezing_level: hzerocl_m.map(|m| wx_grid(WeatherField::FreezingLevel, valid, m)),
        }])
    }

    /// A minimal computed flight over `doc`: legs at the route midpoints,
    /// the given winds, a 60 min phase plan — everything else empty.
    fn minimal_computed(doc: &FlightDoc, winds: Vec<LegWind>) -> ComputedFlight {
        use strata_data::domain::Meters;
        use strata_plan::compute::ComputedLeg;
        use strata_plan::corridor::{Corridor, CorridorParams};
        use strata_plan::fuel::FuelLadder;
        use strata_plan::navlog::{NavLog, NavLogTotals};
        use strata_plan::perf::PhasePlan;
        use strata_plan::units::{Liters, MagneticVariation, Minutes, NauticalMiles};
        use strata_plan::wb::WbReport;

        let legs = doc
            .route
            .windows(2)
            .enumerate()
            .map(|(index, pair)| ComputedLeg {
                index,
                from: pair[0].point.label(),
                to: pair[1].point.label(),
                distance: Meters(10_000.0),
                true_track: DegreesTrue::new(90.0),
                magnetic_track: DegreesTrue::new(90.0).to_magnetic(MagneticVariation(0.0)),
                midpoint: LatLon::new(
                    (pair[0].position().lat() + pair[1].position().lat()) / 2.0,
                    (pair[0].position().lon() + pair[1].position().lon()) / 2.0,
                )
                .unwrap(),
            })
            .collect();
        ComputedFlight {
            legs,
            corridor: Corridor {
                params: CorridorParams::default(),
                samples: Vec::new(),
                crossings: Vec::new(),
            },
            winds,
            phases: PhasePlan {
                segments: Vec::new(),
                toc: None,
                tod: None,
                total_duration: Minutes(60.0),
                total_fuel: Liters(0.0),
            },
            weight_balance: WbReport {
                states: Vec::new(),
                burn_track: Vec::new(),
            },
            fuel: FuelLadder {
                taxi: Liters(0.0),
                trip: Liters(0.0),
                contingency: Liters(0.0),
                alternate: Liters(0.0),
                final_reserve: Liters(0.0),
                extra: Liters(0.0),
                minimum_required: Liters(0.0),
                loaded: Liters(0.0),
                margin: Liters(0.0),
            },
            conflicts: Vec::new(),
            navlog: NavLog {
                rows: Vec::new(),
                totals: NavLogTotals {
                    distance: NauticalMiles(0.0),
                    ete: Minutes(0.0),
                    fuel: Liters(0.0),
                },
            },
        }
    }

    #[test]
    fn freezing_readout_walks_the_documented_chain() {
        let valid = Utc.with_ymd_and_hms(2026, 6, 14, 9, 30, 0).unwrap();
        let mut doc = FlightDoc::new("t");
        doc.route = vec![
            RouteWaypoint::new(free(49.0, 9.0)),
            RouteWaypoint::new(free(49.0, 10.0)),
        ];
        doc.cruise_altitude = Some(PlannedAltitude::Amsl(MetersAmsl(1000.0)));
        doc.departure_time = Some(valid);
        let computed = minimal_computed(&doc, vec![leg_wind_with(0, 5.0, Provenance::Isa)]);

        // Rung 1: the hzerocl grid wins outright.
        let readout = freezing_level_readout(&frames(valid, Some(2800.0), true), &doc, &computed)
            .expect("forecast");
        assert_eq!(readout.source_label, "ICON-D2 0 °C isotherm forecast");
        let expected_ft = 2800.0 * strata_data::domain::FEET_PER_METER;
        assert!((readout.feet - expected_ft).abs() < 1.0, "{readout:?}");

        // Rung 2: no hzerocl, real temperatures → interpolated crossing at
        // the 700 hPa ISA altitude.
        let readout = freezing_level_readout(&frames(valid, None, true), &doc, &computed)
            .expect("from temps");
        assert_eq!(
            readout.source_label,
            "interpolated from ICON-D2 forecast temperatures"
        );
        let expected_ft = strata_data::domain::PressureLevel::P700.isa_altitude().0
            * strata_data::domain::FEET_PER_METER;
        assert!((readout.feet - expected_ft).abs() < 5.0, "{readout:?}");

        // Rung 3, ISA flavor: no frames at all, fallback leg temps → the
        // honest ISA label with the lapse-rate value.
        let readout = freezing_level_readout(&WindsAloftFrames::default(), &doc, &computed)
            .expect("isa estimate");
        assert_eq!(readout.source_label, "ISA estimate — no forecast data");
        let expected_ft =
            (1000.0 + 5.0 * METERS_PER_DEGREE_C) * strata_data::domain::FEET_PER_METER;
        assert!((readout.feet - expected_ft).abs() < 1e-6, "{readout:?}");

        // Rung 3, real flavor: the legs carried real OATs (frames since
        // pruned) → labelled as forecast-derived, not ISA.
        let real = minimal_computed(&doc, vec![leg_wind_with(0, 5.0, Provenance::Real)]);
        let readout = freezing_level_readout(&WindsAloftFrames::default(), &doc, &real)
            .expect("real estimate");
        assert_eq!(
            readout.source_label,
            "estimated from forecast leg temperatures (standard lapse rate)"
        );

        // Without a departure time the frames are unreachable (no sample
        // time) — the chain still ends at the labelled estimate.
        doc.departure_time = None;
        let readout = freezing_level_readout(&frames(valid, Some(2800.0), true), &doc, &computed)
            .expect("fallback");
        assert_eq!(readout.source_label, "ISA estimate — no forecast data");
    }

    #[test]
    fn sigmet_relevance_is_bbox_intersection_with_the_corridor() {
        let mut doc = FlightDoc::new("t");
        doc.route = vec![
            RouteWaypoint::new(free(50.0, 8.0)),
            RouteWaypoint::new(free(50.0, 9.0)),
        ];
        let corridor = route_corridor_bbox(&doc).expect("route has points");

        let polygon = |lats: [f64; 2], lons: [f64; 2]| {
            Polygon::new(
                vec![
                    LatLon::new(lats[0], lons[0]).unwrap(),
                    LatLon::new(lats[0], lons[1]).unwrap(),
                    LatLon::new(lats[1], lons[1]).unwrap(),
                    LatLon::new(lats[1], lons[0]).unwrap(),
                ],
                vec![],
            )
            .unwrap()
        };
        let sigmet = |geometry: Polygon| Sigmet {
            fir: "EDGG".into(),
            hazard: SigmetHazard::Thunderstorm,
            geometry,
            valid_from: Utc.with_ymd_and_hms(2026, 6, 11, 10, 0, 0).unwrap(),
            valid_to: Utc.with_ymd_and_hms(2026, 6, 11, 14, 0, 0).unwrap(),
            raw: "EDGG SIGMET 2 VALID ...".into(),
        };

        let sigmets = vec![
            sigmet(polygon([49.5, 50.5], [8.2, 8.8])), // over the route
            sigmet(polygon([53.0, 54.0], [9.0, 11.0])), // far north
            sigmet(polygon([49.9, 50.2], [9.05, 9.5])), // touches the 5 NM pad
        ];
        assert_eq!(sigmets_intersecting(&sigmets, corridor), vec![0, 2]);

        // Empty route → no corridor → no relevance question to answer.
        let empty = FlightDoc::new("e");
        assert!(route_corridor_bbox(&empty).is_none());
    }

    #[test]
    fn snapshot_age_labels_read_naturally() {
        assert_eq!(age_label(Duration::from_secs(12)), "just now");
        assert_eq!(age_label(Duration::from_secs(60)), "1 min ago");
        assert_eq!(age_label(Duration::from_secs(59 * 60)), "59 min ago");
        assert_eq!(
            age_label(Duration::from_secs(2 * 3600 + 5 * 60)),
            "2 h 5 min ago"
        );
    }
}

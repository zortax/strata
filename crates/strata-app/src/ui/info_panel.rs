//! Right slide-in info panel: detail cards for the selected feature(s).
//! Stacked airspaces are the norm, so the panel renders one card per hit.
//!
//! The card builders are deliberately view-agnostic (`&App` + injected
//! callbacks): in planning mode the context panel's Inspect tab embeds the
//! exact same cards (`ui::context_tabs`), while this module keeps owning
//! the explorer's floating panel chrome and animation.

use std::rc::Rc;

use strata_data::domain::{
    Airport, Airspace, FlightCategory, Frequency, Metar, MetarDecode, Navaid, Obstacle, Qnh,
    ReportingPoint, Taf, Visibility, WindDirection,
};
use strata_data::store::Feature;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, Window, div, ease_out_quint, px, quadratic,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use strata_render::MapTheme;

use crate::app::RootView;
use crate::app::panel_animation::{
    CONTENT_ENTER_DURATION, PANEL_ENTER_DURATION, PANEL_EXIT_DURATION, PanelVisibility,
};
use crate::assets::IconName;
use crate::convert;
use crate::state::AppState;
use crate::ui::{
    airport_kind_label, airspace_kind_label, chip_color, obstacle_kind_label, runway_surface_label,
};

/// Resting inset of the panel from the window edge (matches `right_3`).
const PANEL_INSET_PX: f32 = 12.;
/// Horizontal travel of the panel's enter/exit animation.
const PANEL_SLIDE_PX: f32 = 20.;
/// Horizontal travel of freshly swapped-in content while the panel is open.
const CONTENT_SLIDE_PX: f32 = 8.;

/// METAR/TAF pair for one card (airport cards only; `(None, None)` for
/// everything else).
pub(crate) type StationWeather = (Option<Metar>, Option<Taf>);

/// Click handler injected into the cards (the TAF expander) — the host
/// view supplies its own state flip.
pub(crate) type CardCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// The live METAR/TAF for each feature's station, in card order.
pub(crate) fn station_weather(state: &AppState, features: &[Feature]) -> Vec<StationWeather> {
    features
        .iter()
        .map(|f| match f {
            Feature::Airport(a) => match &a.ident {
                Some(icao) => (state.metar_for(icao).cloned(), state.taf_for(icao).cloned()),
                None => (None, None),
            },
            _ => (None, None),
        })
        .collect()
}

/// One detail card per feature — the explorer panel's content, shared
/// verbatim with the planning context panel's Inspect tab.
pub(crate) fn selection_cards(
    features: &[Feature],
    weather: &[StationWeather],
    map_theme: &MapTheme,
    taf_expanded: bool,
    on_toggle_taf: &CardCallback,
    cx: &App,
) -> Vec<AnyElement> {
    features
        .iter()
        .zip(weather.iter())
        .map(|(feature, (metar, taf))| match feature {
            Feature::Airport(a) => airport_card(
                a,
                metar.as_ref(),
                taf.as_ref(),
                taf_expanded,
                on_toggle_taf.clone(),
                map_theme,
                cx,
            ),
            Feature::Airspace(a) => airspace_card(a, map_theme, cx),
            Feature::Navaid(n) => navaid_card(n, cx),
            Feature::ReportingPoint(p) => reporting_point_card(p, cx),
            Feature::Obstacle(o) => obstacle_card(o, cx),
        })
        .collect()
}

pub fn render_info_panel(
    root: &RootView,
    cx: &mut Context<RootView>,
) -> Option<impl IntoElement + use<>> {
    let visibility = root.panel_anim.visibility();
    if visibility == PanelVisibility::Closed {
        return None;
    }

    // Render from the RootView snapshot, not AppState: during the exit
    // animation the live selection is already empty. Clone it (plus the
    // weather it references) up front so no AppState borrow is held while
    // building listeners.
    let features: Vec<Feature> = root.panel_selection.clone();
    if features.is_empty() {
        return None;
    }
    let weather = station_weather(root.app_state.read(cx), &features);

    // Chip/badge colors must match what the map currently draws.
    let map_theme = MapTheme::by_id(root.app_state.read(cx).map_theme_id).unwrap_or_default();

    let view = cx.entity().downgrade();
    let on_toggle_taf: CardCallback = Rc::new(move |_, cx| {
        view.update(cx, |this, cx| {
            this.taf_expanded = !this.taf_expanded;
            cx.notify();
        })
        .ok();
    });

    let count = features.len();
    let cards = selection_cards(
        &features,
        &weather,
        &map_theme,
        root.taf_expanded,
        &on_toggle_taf,
        cx,
    );

    let panel = v_flex()
        .occlude()
        .absolute()
        .top_3()
        .right_3()
        .bottom_3()
        .w(px(360.))
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.78))
        .backdrop_blur(px(18.))
        .shadow_lg()
        .overflow_hidden()
        .child(
            h_flex()
                .px_3()
                .py_2()
                .justify_between()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if count == 1 {
                            "Selection".to_string()
                        } else {
                            format!("Selection ({count})")
                        }),
                )
                .child(
                    Button::new("close-info")
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .on_click(cx.listener(|this: &mut RootView, _, _, cx| {
                            this.app_state
                                .update(cx, |state, cx| state.clear_selection(cx));
                        })),
                ),
        )
        .child(
            div()
                .id("info-panel-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                // Keyed by the content generation: a new selection while
                // the panel is open re-keys the animation, replaying a
                // short fade/slide on the content while the panel frame
                // stays put. The relative inset offsets without
                // affecting layout (no scrollbar jitter).
                .child(v_flex().gap_3().children(cards).with_animation(
                    ("info-panel-content", root.panel_anim.content_generation()),
                    Animation::new(CONTENT_ENTER_DURATION).with_easing(ease_out_quint()),
                    |content, delta| {
                        content
                            .relative()
                            .left(px(CONTENT_SLIDE_PX * (1. - delta)))
                            .opacity(delta)
                    },
                )),
        );

    // Enter/exit are separate one-shot animations: re-keying by generation /
    // epoch restarts them (gpui animation state lives under the ElementId).
    // Both only touch this absolutely-positioned panel's own inset+opacity,
    // so the map and the other overlays never reflow, and the panel stays
    // fully interactive while animating.
    Some(match visibility {
        PanelVisibility::Closed => return None, // unreachable: handled above
        PanelVisibility::Open => panel.with_animation(
            ("info-panel-enter", root.panel_anim.open_generation()),
            Animation::new(PANEL_ENTER_DURATION).with_easing(ease_out_quint()),
            |panel, delta| {
                panel
                    .right(px(PANEL_INSET_PX - PANEL_SLIDE_PX * (1. - delta)))
                    .opacity(delta)
            },
        ),
        PanelVisibility::Closing => panel.with_animation(
            ("info-panel-exit", root.panel_anim.close_epoch()),
            Animation::new(PANEL_EXIT_DURATION).with_easing(quadratic),
            |panel, delta| {
                panel
                    .right(px(PANEL_INSET_PX - PANEL_SLIDE_PX * delta))
                    .opacity(1. - delta)
            },
        ),
    })
}

// --- shared bits ------------------------------------------------------------

pub(crate) fn card(cx: &App) -> gpui::Div {
    v_flex()
        .gap_2()
        .p_3()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover.opacity(0.6))
        .text_color(cx.theme().popover_foreground)
}

pub(crate) fn kv(label: &str, value: impl Into<String>, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .text_sm()
        .child(
            div()
                .w(px(92.))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(value.into()))
}

pub(crate) fn section(title: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title.to_uppercase())
}

pub(crate) fn badge(text: impl Into<String>, cx: &App) -> impl IntoElement {
    div()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .rounded(cx.theme().radius)
        .bg(cx.theme().secondary)
        .text_color(cx.theme().secondary_foreground)
        .child(text.into())
}

/// Raw METAR/TAF text: monospace block that always wraps inside the card.
/// `whitespace_normal` wraps at spaces; gpui's line wrapper falls back to
/// breaking inside a token if a single token is wider than the panel.
fn raw_weather_block(raw: String, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_xs()
        .font_family("monospace")
        .text_color(cx.theme().muted_foreground)
        .child(raw)
}

/// Display label for a frequency row: the published station name, falling
/// back to the frequency kind when no name is published.
fn frequency_label(f: &Frequency) -> String {
    if f.name.is_empty() {
        format!("{:?}", f.kind)
    } else {
        f.name.clone()
    }
}

// --- weather sections (shared with the planning weather tab) -----------------

/// The METAR section of an airport card: header, decoded one-liner with the
/// flight-category badge, raw text.
pub(crate) fn metar_rows(metar: &Metar, map_theme: &MapTheme, cx: &App) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = vec![section("METAR", cx).into_any_element()];
    if let Some(decoded) = &metar.decoded {
        // items_start + flex_1/min_w_0 so a long summary wraps below
        // itself instead of pushing past the panel's right edge.
        let mut row = h_flex().gap_2().items_start();
        if let Some(category) = decoded.flight_category() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .pt_0p5()
                    .child(flight_category_badge(category, map_theme)),
            );
        }
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(metar_summary(decoded)),
        );
        rows.push(row.into_any_element());
    }
    rows.push(raw_weather_block(metar.raw.clone(), cx).into_any_element());
    rows
}

/// The collapsible TAF section of an airport card. `id` must be stable and
/// unique within the rendering panel; `on_toggle` flips the host's
/// expansion state.
pub(crate) fn taf_rows(
    taf: &Taf,
    id: impl Into<ElementId>,
    expanded: bool,
    on_toggle: CardCallback,
    cx: &App,
) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = vec![
        h_flex()
            .id(id)
            .gap_1()
            .cursor_pointer()
            .on_click(move |_, window, cx| on_toggle(window, cx))
            .child(section("TAF", cx))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .small()
                .text_color(cx.theme().muted_foreground),
            )
            .into_any_element(),
    ];
    if expanded {
        rows.push(raw_weather_block(taf.raw.clone(), cx).into_any_element());
    }
    rows
}

// --- cards ------------------------------------------------------------------

fn airport_card(
    a: &Airport,
    metar: Option<&Metar>,
    taf: Option<&Taf>,
    taf_expanded: bool,
    on_toggle_taf: CardCallback,
    map_theme: &MapTheme,
    cx: &App,
) -> AnyElement {
    let mut el = card(cx)
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(a.name.clone()),
                )
                .children(a.ident.as_ref().map(|i| badge(i.as_str().to_string(), cx))),
        )
        .child(kv("Kind", airport_kind_label(a.kind), cx))
        .child(kv(
            "Elevation",
            format!("{:.0} ft MSL", a.elevation.as_feet()),
            cx,
        ));

    if !a.runways.is_empty() {
        el = el.child(section("Runways", cx));
        for rwy in &a.runways {
            let dims = match (rwy.length, rwy.width) {
                (Some(l), Some(w)) => format!("{:.0} × {:.0} m", l.0, w.0),
                (Some(l), None) => format!("{:.0} m", l.0),
                _ => "—".to_string(),
            };
            let heading = rwy
                .true_heading_deg
                .map(|h| format!("{h:03}°"))
                .unwrap_or_else(|| "—".to_string());
            el = el.child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .w(px(48.))
                            .flex_shrink_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if rwy.main {
                                format!("{} •", rwy.designator)
                            } else {
                                rwy.designator.clone()
                            }),
                    )
                    .child(div().w(px(44.)).flex_shrink_0().child(heading))
                    .child(div().flex_1().child(dims))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(runway_surface_label(rwy.surface)),
                    ),
            );
        }
    }

    if !a.frequencies.is_empty() {
        el = el.child(section("Frequencies", cx));
        for f in &a.frequencies {
            // Every row carries the same padding + 1px border so the columns
            // stay pixel-aligned; only the primary row's border is visible.
            el = el.child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .text_sm()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().transparent)
                    .when(f.primary, |el| {
                        el.border_color(cx.theme().primary.opacity(0.4))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(frequency_label(f)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(f.frequency.to_string()),
                    ),
            );
        }
    }

    if let Some(metar) = metar {
        el = el.children(metar_rows(metar, map_theme, cx));
    }

    if let Some(taf) = taf {
        el = el.children(taf_rows(taf, "taf-toggle", taf_expanded, on_toggle_taf, cx));
    }

    el.into_any_element()
}

fn airspace_card(a: &Airspace, map_theme: &MapTheme, cx: &App) -> AnyElement {
    let style = strata_render::layers::style::airspace_style(
        &map_theme.airspace,
        convert::airspace_style_key(a.class, &a.kind),
    );
    card(cx)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .size_3()
                        .flex_shrink_0()
                        .rounded_sm()
                        .bg(chip_color(style.border)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(a.name.clone()),
                )
                .child(badge(airspace_kind_label(&a.kind, a.class), cx)),
        )
        .child(kv("Vertical", format!("{} — {}", a.lower, a.upper), cx))
        .children(
            a.airac
                .as_ref()
                .map(|airac| kv("AIRAC", airac.id().to_string(), cx)),
        )
        .into_any_element()
}

fn navaid_card(n: &Navaid, cx: &App) -> AnyElement {
    let mut el = card(cx)
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{} {}", n.ident, n.name)),
                )
                .child(badge(n.kind.to_string(), cx)),
        )
        .child(kv(
            "Elevation",
            format!("{:.0} ft MSL", n.elevation.as_feet()),
            cx,
        ));
    if let Some(freq) = n.frequency {
        el = el.child(kv("Frequency", freq.to_string(), cx));
    }
    if let Some(channel) = &n.channel {
        el = el.child(kv("Channel", channel.clone(), cx));
    }
    el.into_any_element()
}

fn reporting_point_card(p: &ReportingPoint, cx: &App) -> AnyElement {
    let mut el = card(cx).child(
        h_flex()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(p.name.clone()),
            )
            .child(badge(
                if p.mandatory {
                    "Mandatory"
                } else {
                    "Voluntary"
                },
                cx,
            )),
    );
    if !p.airports.is_empty() {
        let list = p
            .airports
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        el = el.child(kv("Airports", list, cx));
    }
    el.into_any_element()
}

fn obstacle_card(o: &Obstacle, cx: &App) -> AnyElement {
    card(cx)
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(o.name.clone().unwrap_or_else(|| "Obstacle".to_string())),
                )
                .child(badge(obstacle_kind_label(&o.kind), cx)),
        )
        .child(kv(
            "Height",
            format!("{:.0} ft AGL", o.height.as_feet()),
            cx,
        ))
        .child(kv(
            "Top",
            format!("{:.0} ft MSL", o.elevation_top.as_feet()),
            cx,
        ))
        .child(kv("Lighted", if o.lighted { "yes" } else { "no" }, cx))
        .into_any_element()
}

// --- weather formatting -------------------------------------------------------

pub(crate) fn flight_category_badge(
    category: FlightCategory,
    map_theme: &MapTheme,
) -> impl IntoElement {
    let color = chip_color(strata_render::layers::style::flight_category_color(
        &map_theme.weather,
        convert::flight_category_color(category),
    ));
    let label = match category {
        FlightCategory::Vfr => "VFR",
        FlightCategory::Mvfr => "MVFR",
        FlightCategory::Ifr => "IFR",
        FlightCategory::Lifr => "LIFR",
    };
    h_flex()
        .gap_1()
        .items_center()
        .child(div().size_2().rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        )
}

/// One-line human summary of a decoded METAR (shared with the briefing
/// PDF's weather section).
pub(crate) fn metar_summary(d: &MetarDecode) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(wind) = &d.wind {
        let dir = match wind.direction {
            WindDirection::Degrees(deg) => format!("{deg:03}°"),
            WindDirection::Variable => "VRB".to_string(),
        };
        let gust = wind.gust_kt.map(|g| format!(" G{g}")).unwrap_or_default();
        parts.push(format!("{dir} {}{gust} kt", wind.speed_kt));
    }
    if let Some(visibility) = d.visibility {
        parts.push(match visibility {
            Visibility::Cavok => "CAVOK".to_string(),
            Visibility::Meters(m) if m >= 9999 => "vis 10 km+".to_string(),
            Visibility::Meters(m) if m >= 1000 => format!("vis {:.0} km", m as f64 / 1000.0),
            Visibility::Meters(m) => format!("vis {m} m"),
        });
    }
    if let Some(ceiling) = d.ceiling_ft_agl() {
        parts.push(format!("ceiling {ceiling} ft"));
    }
    if let (Some(t), Some(td)) = (d.temperature_c, d.dewpoint_c) {
        parts.push(format!("{t}/{td} °C"));
    }
    if let Some(qnh) = d.qnh {
        parts.push(match qnh {
            Qnh::Hpa(h) => format!("Q{h}"),
            Qnh::InHg(i) => format!("{i:.2} inHg"),
        });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_data::decode::decode_metar;
    use strata_data::domain::{FrequencyKind, RadioFrequency};

    #[test]
    fn frequency_label_prefers_published_station_name() {
        let f = Frequency {
            frequency: RadioFrequency::from_mhz(119.15),
            name: "LANGEN INFORMATION".to_string(),
            kind: FrequencyKind::Information,
            primary: false,
        };
        assert_eq!(frequency_label(&f), "LANGEN INFORMATION");
    }

    #[test]
    fn frequency_label_falls_back_to_kind_when_name_is_empty() {
        let f = Frequency {
            frequency: RadioFrequency::from_mhz(118.105),
            name: String::new(),
            kind: FrequencyKind::Tower,
            primary: true,
        };
        assert_eq!(frequency_label(&f), "Tower");
    }

    #[test]
    fn metar_summary_renders_typical_german_metar() {
        let decoded =
            decode_metar("EDDF 101220Z 24012G22KT 9999 BKN025 12/07 Q1013 NOSIG").expect("decodes");
        let summary = metar_summary(&decoded);
        assert!(summary.contains("240°"), "{summary}");
        assert!(summary.contains("G22"), "{summary}");
        assert!(summary.contains("ceiling 2500 ft"), "{summary}");
        assert!(summary.contains("Q1013"), "{summary}");
    }
}

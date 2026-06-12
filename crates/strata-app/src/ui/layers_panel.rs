//! Floating layers panel: a frosted card over the bottom-left of the map
//! with icon-only per-layer visibility toggles and the weather refresh
//! button. Layer names show as tooltips; toggled-on layers render filled
//! (primary), toggled-off ones ghost/muted.

use std::time::Instant;

use gpui::{Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, px};
use gpui_component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    separator::Separator,
};
use strata_render::LayerId;

use crate::app::RootView;
use crate::assets::IconName;

const LAYERS: [(LayerId, IconName, &str); 11] = [
    (LayerId::Terrain, IconName::Mountain, "Terrain"),
    (LayerId::Basemap, IconName::Map, "Basemap"),
    (LayerId::Airspace, IconName::Layers, "Airspace"),
    (LayerId::Airports, IconName::Plane, "Airports"),
    (LayerId::Navaids, IconName::RadioTower, "Navaids"),
    (
        LayerId::ReportingPoints,
        IconName::Waypoints,
        "Reporting points",
    ),
    (LayerId::Obstacles, IconName::TriangleAlert, "Obstacles"),
    (LayerId::Weather, IconName::Cloud, "Weather"),
    // Gridded weather overlays (default off); toggling any of them shows
    // the time slider and starts the DWD fetch scheduler.
    (LayerId::CloudCover, IconName::Cloudy, "Cloud cover"),
    (LayerId::Precipitation, IconName::CloudRain, "Precipitation"),
    (
        LayerId::Thunderstorms,
        IconName::CloudLightning,
        "Thunderstorms",
    ),
];

pub fn render_layers_panel(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let mut row = h_flex().gap_1().items_center();
    for (layer, icon, label) in LAYERS {
        row = row.child(layer_toggle(root, layer, icon, label, cx));
    }
    row = row
        .child(Separator::vertical().h_4().w(px(1.)))
        .child(weather_refresh_button(root, cx));
    // Extension slot: future global controls append to `row` here as
    // further small icon buttons.
    row = row.child(crate::ui::theme::render_map_theme_picker(root, cx));

    div_frosted(cx).child(row)
}

/// The shared frosted-card recipe (matches the search box and info panel).
/// Positioning comes from the bottom-left column in `RootView` — the panel's
/// intrinsic width defines that column's width (the time slider aligns to it).
fn div_frosted(cx: &Context<RootView>) -> gpui::Div {
    gpui::div()
        .occlude()
        .p_1()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.78))
        .backdrop_blur(px(18.))
        .shadow_lg()
}

/// One icon-only layer toggle: filled (primary) when the layer is on,
/// ghost/muted when off. The layer name lives in the tooltip.
fn layer_toggle(
    root: &RootView,
    layer: LayerId,
    icon: IconName,
    label: &'static str,
    cx: &mut Context<RootView>,
) -> Button {
    let enabled = root.map_view.read(cx).layer_enabled(layer);
    let mut button = Button::new(label)
        .small()
        .icon(icon)
        .selected(enabled)
        .tooltip(label)
        .on_click(cx.listener(move |this: &mut RootView, _, _, cx| {
            let on = this.map_view.read(cx).layer_enabled(layer);
            this.map_view
                .update(cx, |map, cx| map.set_layer_enabled(layer, !on, cx));
            // The weather time slider shows while any gridded weather
            // layer is on (cheap no-op for the other layers).
            this.sync_time_slider_visibility(cx);
            cx.notify();
        }));
    button = if enabled {
        button.selected(true)
    } else {
        button.ghost().text_color(cx.theme().muted_foreground)
    };
    button
}

fn weather_refresh_button(root: &RootView, cx: &mut Context<RootView>) -> Button {
    let weather = &root.app_state.read(cx).weather;
    let fetching = weather.fetching;
    let tooltip = if fetching {
        "Refreshing weather…".to_string()
    } else if let Some(err) = &weather.last_error {
        tracing::debug!(%err, "weather error shown in layers panel");
        "Weather refresh failed — click to retry".to_string()
    } else {
        match weather.last_fetched_at.map(age_label) {
            Some(age) => format!("Refresh weather · fetched {age}"),
            None => "Refresh weather".to_string(),
        }
    };

    Button::new("wx-refresh")
        .ghost()
        .small()
        .icon(IconName::RefreshCw)
        .text_color(cx.theme().muted_foreground)
        .loading(fetching)
        .tooltip(tooltip)
        .on_click(cx.listener(|this: &mut RootView, _, _, cx| {
            this.app_state
                .update(cx, |state, cx| state.refresh_weather(cx));
        }))
}

/// "now" / "3m ago" / "2h ago".
fn age_label(at: Instant) -> String {
    let secs = at.elapsed().as_secs();
    match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

//! Bottom status bar: AIRAC cycle + staleness, cursor position, zoom, data
//! attribution, and the persistent "NOT FOR NAVIGATION" badge.

use gpui::{Context, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, h_flex};

use crate::app::RootView;
use crate::assets::IconName;

pub fn render_status_bar(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let state = root.app_state.read(cx);
    let airac = state.airac().map(|a| a.id().to_string());
    let stale = state.airac_stale();
    let has_data = state.has_data();
    let cursor = state.cursor;
    let zoom = state.camera.map(|c| c.zoom);

    let airac_chip = match airac {
        Some(id) => {
            let mut chip = h_flex().gap_1().items_center().child(format!("AIRAC {id}"));
            if stale {
                chip = chip
                    .text_color(cx.theme().warning)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .xsmall()
                            .text_color(cx.theme().warning),
                    )
                    .child("STALE");
            }
            chip
        }
        None if !has_data => h_flex().child("No data ingested"),
        None => h_flex().child("AIRAC —"),
    };

    h_flex()
        .h(px(26.))
        .px_3()
        .gap_4()
        .items_center()
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(airac_chip)
        .child(
            div().child(
                cursor.map_or_else(|| "—".to_string(), |(lat, lon)| format_cursor(lat, lon)),
            ),
        )
        .child(div().child(zoom.map_or_else(|| "z —".to_string(), |z| format!("z {z:.1}"))))
        .child(div().flex_1())
        // Legally required credits: the basemap is OSM-derived via Protomaps
        // (ODbL attribution), all aero data is openAIP (CC BY-NC), and the
        // gridded weather overlays are DWD open data (source credit
        // required).
        .child(div().child("© OpenStreetMap contributors · Protomaps · openAIP CC BY-NC · DWD"))
        .child(
            div()
                .px_2()
                .py_0p5()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().danger.opacity(0.4))
                .bg(cx.theme().danger.opacity(0.15))
                .text_color(cx.theme().danger)
                .font_weight(FontWeight::SEMIBOLD)
                .child("NOT FOR NAVIGATION"),
        )
}

/// `50.7757°N 6.0444°E` with hemisphere letters, 4 decimals.
fn format_cursor(lat: f64, lon: f64) -> String {
    let ns = if lat < 0.0 { 'S' } else { 'N' };
    let ew = if lon < 0.0 { 'W' } else { 'E' };
    format!("{:.4}°{ns} {:.4}°{ew}", lat.abs(), lon.abs())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cursor_formatting() {
        assert_eq!(
            super::format_cursor(50.77567, 6.04439),
            "50.7757°N 6.0444°E"
        );
        assert_eq!(super::format_cursor(-12.5, -33.25), "12.5000°S 33.2500°W");
    }
}

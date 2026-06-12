//! The Countries settings page: per-country switches over the curated
//! Europe list ([`Country::ALL`]), with a search filter and a coarse
//! download-size hint per row.
//!
//! Semantics (stated honestly in the page header): the selection scopes
//! **ingestion only** — which countries' aeronautical data, basemap and
//! terrain are downloaded and kept current. Rendering stays viewport-driven
//! over whatever the store holds, and already-downloaded data stays on disk
//! when a country is disabled.
//!
//! Toggles write through [`AppState::set_enabled_countries`], which
//! persists the config immediately (diff-aware atomic save) and re-scopes
//! the weather fetch / auto-ingest on the spot.

use gpui::{App, Entity, SharedString, Styled as _};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    input::Input,
    label::Label,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage},
};
use strata_data::domain::Country;

use crate::assets::IconName;
use crate::state::AppState;

use super::SettingsView;

/// The page header paragraph — the honest semantics statement.
const SEMANTICS: &str =
    "Controls which countries' aeronautical data, basemap and terrain are \
     downloaded and kept current. Already-downloaded data stays on disk \
     when a country is disabled.";

/// Builds the Countries page: a search row plus one switch row per country
/// matching the current filter (all of [`Country::ALL`] when empty).
pub(super) fn page(this: &SettingsView, cx: &App) -> SettingPage {
    let query = this
        .countries_filter
        .read(cx)
        .value()
        .trim()
        .to_lowercase();
    let app_state = &this.app_state;
    let enabled_count = app_state.read(cx).config.countries.len();

    let filter_input = this.countries_filter.clone();
    let mut group = SettingGroup::new()
        .title(format!(
            "Coverage — {enabled_count} of {} enabled",
            Country::ALL.len()
        ))
        .description(SEMANTICS)
        .item(
            SettingItem::render(move |options, _window, _cx: &mut App| {
                Input::new(&filter_input)
                    .cleanable(true)
                    .with_size(options.size)
            })
            .description("Filter the list by country name or two-letter code."),
        );

    let matching: Vec<Country> = Country::ALL
        .into_iter()
        .filter(|country| matches_query(*country, &query))
        .collect();
    if matching.is_empty() {
        group = group.item(SettingItem::render(move |_options, _window, cx: &mut App| {
            Label::new(format!("No countries match \"{query}\"."))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
        }));
    }
    for country in matching {
        group = group.item(country_item(app_state, country));
    }

    SettingPage::new("Countries")
        .icon(Icon::new(IconName::Globe))
        .group(group)
}

/// One country row: name as the title, a switch as the field, code plus the
/// coarse size hint as the description.
fn country_item(app_state: &Entity<AppState>, country: Country) -> SettingItem {
    let read_state = app_state.clone();
    let write_state = app_state.clone();
    SettingItem::new(
        SharedString::new_static(country.name()),
        SettingField::switch(
            move |cx: &App| read_state.read(cx).config.countries.contains(&country),
            move |enabled: bool, cx: &mut App| {
                write_state.update(cx, |state, cx| {
                    let mut next = state.config.countries.clone();
                    if enabled {
                        next.push(country);
                    } else {
                        next.retain(|c| *c != country);
                    }
                    // Normalizes, persists (save_if_changed), re-scopes
                    // weather + auto-ingest; no-op when nothing changed.
                    state.set_enabled_countries(next, cx);
                });
            },
        )
        .default_value(country == Country::DE),
    )
    .description(format!(
        "{} · full data {}",
        country.code(),
        SizeClass::of(country).hint()
    ))
}

/// Case-insensitive substring match on the country name or alpha-2 code;
/// the empty query matches everything. `query` must already be trimmed and
/// lowercased.
fn matches_query(country: Country, query: &str) -> bool {
    query.is_empty()
        || country.name().to_lowercase().contains(query)
        || country.code().to_lowercase().contains(query)
}

// --- size classes ------------------------------------------------------------

/// Coarse full-data download-size classes for the per-row hint.
///
/// Heuristic: the dominant data volume (basemap tiles, terrain hillshade,
/// elevation) scales with the ingested **bounding-box** area, so countries
/// are classed by `lon_span × lat_span × cos(mid_lat)` in equator-equivalent
/// square degrees, calibrated against the one known real store (Germany,
/// ≈ 51 deg² ≈ 5 GB at the default basemap max zoom of 13):
///
/// | class                  | weighted bbox area | hint           |
/// |------------------------|--------------------|----------------|
/// | [`SizeClass::Small`]   | < 6 deg²           | up to ~0.5 GB  |
/// | [`SizeClass::Medium`]  | 6 – 20 deg²        | ~0.5–2 GB      |
/// | [`SizeClass::Large`]   | 20 – 60 deg²       | ~2–5 GB        |
/// | [`SizeClass::ExtraLarge`] | ≥ 60 deg²       | ~5–10 GB       |
///
/// Deliberately no fake precision: actual size varies with data density
/// (sparse far-north countries run well below their area's suggestion) and
/// with the configured basemap max zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeClass {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl SizeClass {
    /// Classifies a country by its cos-weighted bounding-box area (see the
    /// type docs for the thresholds and calibration).
    fn of(country: Country) -> Self {
        let area = weighted_bbox_area_deg2(country);
        if area < 6.0 {
            Self::Small
        } else if area < 20.0 {
            Self::Medium
        } else if area < 60.0 {
            Self::Large
        } else {
            Self::ExtraLarge
        }
    }

    /// The static, deliberately coarse size-hint string for the row.
    fn hint(self) -> &'static str {
        match self {
            Self::Small => "up to ~0.5 GB",
            Self::Medium => "~0.5–2 GB",
            Self::Large => "~2–5 GB",
            Self::ExtraLarge => "~5–10 GB",
        }
    }
}

/// Bounding-box area in equator-equivalent square degrees:
/// `lon_span × lat_span × cos(mid_lat)` — proportional to the tile count
/// the box ingests, which is what dominates the download size.
fn weighted_bbox_area_deg2(country: Country) -> f64 {
    let b = country.bounding_box();
    let mid_lat = f64::midpoint(b.south(), b.north());
    (b.east() - b.west()) * (b.north() - b.south()) * mid_lat.to_radians().cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn germany_calibration_holds() {
        // The size classes are calibrated on the German box (~51 deg² ≈ the
        // real ~5 GB store). If the box ever changes, re-derive thresholds.
        let area = weighted_bbox_area_deg2(Country::DE);
        assert!((45.0..60.0).contains(&area), "DE weighted area: {area}");
        assert_eq!(SizeClass::of(Country::DE), SizeClass::Large);
    }

    #[test]
    fn every_country_has_a_class_and_a_nonempty_hint() {
        for country in Country::ALL {
            let hint = SizeClass::of(country).hint();
            assert!(!hint.is_empty(), "{country}");
            assert!(hint.contains("GB"), "{country}: {hint}");
        }
    }

    /// Pinned examples per class — catches threshold or bbox drift.
    #[test]
    fn size_class_examples_are_pinned() {
        for (country, class) in [
            (Country::LU, SizeClass::Small),
            (Country::MT, SizeClass::Small),
            (Country::SI, SizeClass::Small),
            (Country::CH, SizeClass::Medium),
            (Country::AT, SizeClass::Medium),
            (Country::NL, SizeClass::Medium),
            (Country::DE, SizeClass::Large),
            (Country::PL, SizeClass::Large),
            (Country::GR, SizeClass::Large),
            (Country::FR, SizeClass::ExtraLarge),
            (Country::IT, SizeClass::ExtraLarge),
            (Country::NO, SizeClass::ExtraLarge),
        ] {
            assert_eq!(SizeClass::of(country), class, "{country}");
        }
    }

    #[test]
    fn hints_grow_with_the_class() {
        // Each class has its own distinct string; spot-check the ordering
        // reads sensibly (small mentions 0.5, extra-large mentions 10).
        assert!(SizeClass::Small.hint().contains("0.5"));
        assert!(SizeClass::ExtraLarge.hint().contains("10"));
        let hints = [
            SizeClass::Small.hint(),
            SizeClass::Medium.hint(),
            SizeClass::Large.hint(),
            SizeClass::ExtraLarge.hint(),
        ];
        let mut deduped = hints.to_vec();
        deduped.dedup();
        assert_eq!(deduped.len(), hints.len(), "hints must be distinct");
    }

    #[test]
    fn query_matches_name_and_code_case_insensitively() {
        // Callers pass trimmed, lowercased queries.
        assert!(matches_query(Country::DE, ""));
        assert!(matches_query(Country::DE, "de"));
        assert!(matches_query(Country::DE, "germ"));
        assert!(matches_query(Country::CZ, "czech"));
        assert!(!matches_query(Country::DE, "austria"));
        // Code substring also matches countries whose *name* contains it.
        assert!(matches_query(Country::SE, "de"), "Sweden contains 'de'");
        // Every country is reachable by its own code and name.
        for country in Country::ALL {
            assert!(matches_query(country, &country.code().to_lowercase()));
            assert!(matches_query(country, &country.name().to_lowercase()));
        }
    }
}

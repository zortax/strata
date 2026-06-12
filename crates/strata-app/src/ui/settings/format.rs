//! Pure helpers for the settings modal: API-key masking, value formatting,
//! and dropdown option building. No gpui state — everything unit-testable.

use gpui::SharedString;

use crate::config::{BASEMAP_MAXZOOM_RANGE, MapTheme};

/// Masked display of the openAIP API key: only the last four characters
/// stay readable ("••••••••3df2"); short keys mask entirely. Empty/blank
/// input yields an empty string (the caller shows "Not set").
pub(crate) fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 4 {
        return "••••".to_owned();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("••••••••{tail}")
}

/// Signed one-decimal label for the basemap detail bias ("−0.5", "+0.3").
pub(crate) fn format_bias(bias: f64) -> String {
    format!("{bias:+.1}")
}

/// Dropdown options for the basemap max zoom (the documented 8..=14 range).
pub(crate) fn maxzoom_options() -> Vec<(SharedString, SharedString)> {
    BASEMAP_MAXZOOM_RANGE
        .map(|z| {
            (
                SharedString::from(z.to_string()),
                SharedString::from(format!("z{z}")),
            )
        })
        .collect()
}

/// Dropdown options for the map (renderer) theme: `auto` first, then every
/// built-in renderer theme by id/name (registry presentation order).
pub(crate) fn map_theme_options() -> Vec<(SharedString, SharedString)> {
    let mut options = vec![(
        SharedString::from("auto"),
        SharedString::from("Auto (follow UI theme)"),
    )];
    for id in strata_render::MapTheme::BUILT_IN_IDS {
        if let Some(theme) = strata_render::MapTheme::by_id(id) {
            options.push((SharedString::from(theme.id), SharedString::from(theme.name)));
        }
    }
    options
}

/// The config map theme as a dropdown value ("auto" or the theme name).
pub(crate) fn map_theme_value(theme: &MapTheme) -> SharedString {
    SharedString::from(String::from(theme.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_masking_keeps_only_the_last_four_chars() {
        assert_eq!(mask_api_key(""), "");
        assert_eq!(mask_api_key("   "), "");
        assert_eq!(mask_api_key("ab"), "••••", "short keys mask entirely");
        assert_eq!(mask_api_key("abcd"), "••••");
        assert_eq!(mask_api_key("abcde"), "••••••••bcde");
        assert_eq!(mask_api_key("0123456789abcdef"), "••••••••cdef");
        // Trims whitespace before masking, and counts chars (not bytes).
        assert_eq!(mask_api_key("  key-täil4  "), "••••••••äil4");
    }

    #[test]
    fn masked_key_never_contains_the_secret_prefix() {
        let key = "secret-prefix-1234";
        let masked = mask_api_key(key);
        assert!(!masked.contains("secret"));
        assert!(masked.ends_with("1234"));
    }

    #[test]
    fn bias_label_is_signed_with_one_decimal() {
        assert_eq!(format_bias(-0.5), "-0.5");
        assert_eq!(format_bias(0.0), "+0.0");
        assert_eq!(format_bias(0.25), "+0.2");
        assert_eq!(format_bias(-1.5), "-1.5");
    }

    #[test]
    fn maxzoom_options_cover_the_documented_range() {
        let options = maxzoom_options();
        assert_eq!(options.first().map(|(v, _)| v.as_ref()), Some("8"));
        assert_eq!(options.last().map(|(v, _)| v.as_ref()), Some("14"));
        assert_eq!(options.len(), 7);
    }

    #[test]
    fn map_theme_options_offer_auto_first_plus_every_builtin() {
        let options = map_theme_options();
        assert_eq!(options[0].0.as_ref(), "auto", "Auto must come first");
        assert_eq!(
            options.len(),
            1 + strata_render::MapTheme::BUILT_IN_IDS.len(),
            "exactly Auto + every built-in"
        );
        let values: Vec<&str> = options.iter().map(|(v, _)| v.as_ref()).collect();
        for id in strata_render::MapTheme::BUILT_IN_IDS {
            assert!(values.contains(&id), "missing built-in {id}");
        }
    }

    #[test]
    fn map_theme_value_round_trips_through_the_config_conversions() {
        assert_eq!(map_theme_value(&MapTheme::Auto).as_ref(), "auto");
        let named = MapTheme::Named("high-contrast".into());
        let value = map_theme_value(&named);
        assert_eq!(MapTheme::from(value.to_string()), named);
        assert_eq!(MapTheme::from("auto".to_string()), MapTheme::Auto);
    }
}

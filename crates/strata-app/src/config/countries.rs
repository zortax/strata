//! The `[countries]` config section: the enabled coverage countries.
//!
//! Country selection scopes **ingestion only** — which countries'
//! aeronautical data, basemap and terrain get downloaded and kept current,
//! plus the live METAR/TAF fetch area. Rendering stays viewport-driven
//! over whatever the store holds, and already-downloaded data stays on
//! disk when a country is disabled.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use strata_data::domain::Country;

/// Sorts into the curated [`Country::ALL`] order and deduplicates. An
/// empty set stays empty: "no countries" is a legal state — nothing gets
/// auto-ingested and the app still runs on whatever data is present. Only
/// a config file with no `[countries]` section at all defaults to
/// [`Country::DEFAULT_ENABLED`].
pub fn normalize_countries(mut countries: Vec<Country>) -> Vec<Country> {
    countries.sort_unstable();
    countries.dedup();
    countries
}

/// serde `with` module for `Config::countries`: the plain `Vec<Country>`
/// field lives in the file as its own table,
///
/// ```toml
/// [countries]
/// enabled = ["DE", "AT"]
/// ```
///
/// Robustness contract (mirrors the module docs in `config`):
/// - codes parse **case-insensitively**, surrounding whitespace ignored;
/// - **unknown codes are dropped with a warning** instead of failing the
///   whole config file (typos, forward compatibility);
/// - duplicates collapse, the result is in [`Country::ALL`] order;
/// - `enabled = []` is preserved as the **empty set** (see
///   [`normalize_countries`]); a `[countries]` section without an
///   `enabled` key defaults to Germany like a missing section.
pub(super) mod section {
    use super::*;

    /// Serialization view: borrows the field.
    #[derive(Serialize)]
    struct SectionRef<'a> {
        enabled: &'a [Country],
    }

    /// Deserialization view: plain strings so one bad code can never
    /// invalidate the whole file; unknown keys inside `[countries]` are
    /// tolerated like everywhere else in the config.
    #[derive(Deserialize)]
    #[serde(default)]
    struct Section {
        enabled: Vec<String>,
    }

    impl Default for Section {
        fn default() -> Self {
            Self {
                enabled: Country::DEFAULT_ENABLED
                    .iter()
                    .map(|c| c.code().to_owned())
                    .collect(),
            }
        }
    }

    pub fn serialize<S>(countries: &[Country], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SectionRef { enabled: countries }.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Country>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let section = Section::deserialize(deserializer)?;
        Ok(parse_enabled_codes(&section.enabled))
    }
}

/// Pure core of the lenient load: trims and parses each code
/// case-insensitively, drops unknown ones with a warning, deduplicates and
/// sorts via [`normalize_countries`].
fn parse_enabled_codes(codes: &[String]) -> Vec<Country> {
    let mut enabled = Vec::with_capacity(codes.len());
    for code in codes {
        match code.trim().parse::<Country>() {
            Ok(country) => enabled.push(country),
            Err(error) => {
                tracing::warn!(%error, "ignoring unknown country code in config [countries] enabled");
            }
        }
    }
    normalize_countries(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_drops_unknown_codes_and_keeps_known_ones() {
        assert_eq!(
            parse_enabled_codes(&codes(&["DE", "XX", "AT", "Atlantis", ""])),
            vec![Country::DE, Country::AT]
        );
        // All-unknown input yields the empty set, not a failure.
        assert_eq!(parse_enabled_codes(&codes(&["??", "ZZ"])), Vec::new());
    }

    #[test]
    fn parse_is_case_insensitive_trimmed_and_deduplicating() {
        assert_eq!(
            parse_enabled_codes(&codes(&["de", " AT ", "De", "at", "ch"])),
            vec![Country::DE, Country::AT, Country::CH]
        );
    }

    #[test]
    fn normalize_sorts_into_curated_order_and_keeps_empty_empty() {
        // Curated declaration order (Germany first), not alphabetical.
        assert_eq!(
            normalize_countries(vec![Country::CH, Country::AT, Country::DE, Country::AT]),
            vec![Country::DE, Country::AT, Country::CH]
        );
        // The empty set is a legal state (nothing auto-ingested) — it must
        // not silently fall back to the default.
        assert_eq!(normalize_countries(Vec::new()), Vec::new());
    }
}

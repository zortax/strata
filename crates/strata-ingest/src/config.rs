//! Run configuration for the ingestion library: enabled countries, data
//! directory, optional credentials and coverage override.

use std::fmt;
use std::path::PathBuf;

use strata_data::domain::{BoundingBox, Country};
use strata_data::paths;

/// Everything an ingest job needs to know about where to read and write.
///
/// Built by the caller — the library reads no environment variables. The CLI
/// resolves flags/env/XDG into this; the GUI app passes its own data dir and
/// settings.
///
/// `countries` is the **ingestion scope**: every dataset entry point loops
/// it (aero fetches per country, basemap/terrain/elevation cover each
/// country's bbox). It never hides anything at render time — the map shows
/// whatever the store holds.
#[derive(Clone)]
pub struct IngestConfig {
    pub data_dir: PathBuf,
    /// Countries to ingest/keep current, in run order. Empty means there
    /// is nothing to do.
    pub countries: Vec<Country>,
    /// openAIP API key for [`Ingestion::aero`](crate::Ingestion::aero) —
    /// injected by the caller (CLI: `.env`/environment; GUI: its settings).
    /// `None` or blank fails `aero` with
    /// [`IngestError::MissingApiKey`](crate::IngestError::MissingApiKey).
    pub openaip_api_key: Option<String>,
    /// Coverage override for cheap basemap/terrain smoke runs; `None`
    /// covers each country's bounding box.
    pub bbox_override: Option<BoundingBox>,
}

impl IngestConfig {
    /// Config with no API key and no bbox override.
    pub fn new(data_dir: impl Into<PathBuf>, countries: Vec<Country>) -> Self {
        Self {
            data_dir: data_dir.into(),
            countries,
            openaip_api_key: None,
            bbox_override: None,
        }
    }

    /// The tile/DEM coverage passes this run executes, in order: one per
    /// country (labelled with it), or a single unlabelled pass when the
    /// bbox override is set (smoke runs shrink coverage; the per-country
    /// boxes do not apply).
    pub fn coverage_passes(&self) -> Vec<(Option<Country>, BoundingBox)> {
        match self.bbox_override {
            Some(bbox) => vec![(None, bbox)],
            None => self
                .countries
                .iter()
                .map(|&c| (Some(c), c.bounding_box()))
                .collect(),
        }
    }

    pub fn bbox_overridden(&self) -> bool {
        self.bbox_override.is_some()
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("store.sqlite")
    }

    /// The shared basemap archive (`basemap.mbtiles`) — all countries'
    /// extracts merge into this one file.
    pub fn basemap_path(&self) -> PathBuf {
        self.data_dir.join(paths::BASEMAP_FILE)
    }

    /// Pre-multi-country archive name, still found on existing installs
    /// until [`paths::migrate_legacy_basemap`] ran.
    pub fn legacy_basemap_path(&self) -> PathBuf {
        self.data_dir.join(paths::LEGACY_BASEMAP_FILE)
    }

    pub fn dem_cache_dir(&self) -> PathBuf {
        self.data_dir.join("dem-cache")
    }
}

/// Manual impl so the API key can never leak into logs.
impl fmt::Debug for IngestConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IngestConfig")
            .field("data_dir", &self.data_dir)
            .field("countries", &self.countries)
            .field(
                "openaip_api_key",
                &self.openaip_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("bbox_override", &self.bbox_override)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn derived_paths_hang_off_the_data_dir() {
        let config = IngestConfig::new("/data", vec![Country::DE]);
        assert_eq!(config.store_path(), p("/data/store.sqlite"));
        assert_eq!(config.basemap_path(), p("/data/basemap.mbtiles"));
        assert_eq!(config.legacy_basemap_path(), p("/data/basemap-de.mbtiles"));
        assert_eq!(config.dem_cache_dir(), p("/data/dem-cache"));
    }

    #[test]
    fn coverage_passes_are_per_country_unless_overridden() {
        let config = IngestConfig::new("/data", vec![Country::DE, Country::AT]);
        assert!(!config.bbox_overridden());
        assert_eq!(
            config.coverage_passes(),
            vec![
                (Some(Country::DE), Country::DE.bounding_box()),
                (Some(Country::AT), Country::AT.bounding_box()),
            ]
        );

        let small = BoundingBox::new(9.5, 49.0, 10.5, 50.0).unwrap();
        let config = IngestConfig {
            bbox_override: Some(small),
            ..config
        };
        assert!(config.bbox_overridden());
        assert_eq!(config.coverage_passes(), vec![(None, small)]);
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let config = IngestConfig {
            openaip_api_key: Some("super-secret".to_string()),
            ..IngestConfig::new("/data", vec![Country::DE])
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret"), "got: {debug}");
        assert!(debug.contains("<redacted>"), "got: {debug}");
    }
}

//! Source traits: everything the planning core reads from the outside
//! world (plan §3). `strata-plan` stays storage-agnostic — the app
//! implements these over its SQLite store / gridded weather caches / WMM,
//! tests implement them synthetically. All traits are object-safe; the
//! [`Sources`] bundle is what [`compute`](crate::compute::compute) takes.
//!
//! Thread-safety is the implementor's choice: compute runs wherever the
//! caller puts it, with sources constructed on (or moved to) that thread.

use std::error::Error;
use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use strata_data::domain::{Airspace, BoundingBox, LatLon, MetersAmsl, Obstacle};

use crate::units::{Celsius, DegreesTrue, Knots, MagneticVariation};

/// Opaque error from a source implementation (store failure, decode error,
/// missing data, …). Carries a message and an optional underlying error.
#[derive(Debug)]
pub struct SourceError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl SourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for SourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn Error + 'static))
    }
}

/// Worst-case terrain elevation (plan §2.1: the store-resident
/// **max-pooled** ~6 arc-second grid; max-pooling is conservative by
/// construction — aggregation can only raise, never hide, a ridge).
pub trait ElevationSource {
    /// Maximum terrain elevation within the grid cell containing `p`;
    /// `Ok(None)` outside coverage.
    fn max_elevation_at(&self, p: LatLon) -> Result<Option<MetersAmsl>, SourceError>;
}

/// Point obstacles (masts, wind turbines, …) for the corridor profile.
pub trait ObstacleSource {
    /// All obstacles intersecting `bbox`.
    fn obstacles_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Obstacle>, SourceError>;
}

/// Airspace volumes for corridor crossing detection.
pub trait AirspaceSource {
    /// All airspaces whose geometry bbox intersects `bbox` (the store's
    /// R*Tree semantics); exact point-in-polygon happens corridor-side.
    fn airspaces_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airspace>, SourceError>;
}

/// Where a planning value came from: real forecast data or the ISA
/// standard-atmosphere assumption. Carried on sampled values so every
/// surface (nav log, weather tab, briefing PDF) can label honestly what
/// is forecast and what is convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// From actual forecast data (e.g. an ICON temperature grid).
    Real,
    /// ISA-derived estimate — no forecast data backed this value.
    Isa,
}

/// One winds-aloft sample: wind vector + outside air temperature.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindsAloft {
    /// True direction the wind blows **from**.
    pub direction: DegreesTrue,
    pub speed: Knots,
    /// Outside air temperature at the sampled altitude.
    pub temperature: Celsius,
    /// Whether [`Self::temperature`] is real forecast data or an ISA
    /// estimate. Defaults to [`Provenance::Isa`] — honest for any sample
    /// produced before temperature grids were wired.
    #[serde(default = "isa_provenance")]
    pub temperature_provenance: Provenance,
}

/// Serde default for [`WindsAloft::temperature_provenance`].
fn isa_provenance() -> Provenance {
    Provenance::Isa
}

/// Winds/temperature aloft (plan §2.2: app-side impl over the gridded
/// ICON-D2 pressure-level cache, interpolating vertically between pressure
/// levels via the standard-atmosphere altitude→pressure mapping — that
/// approximation is documented at the implementation).
pub trait WindsAloftSampler {
    /// Sample at a position, **true altitude** and valid time; `Ok(None)`
    /// where the model grid has no data (outside extent/timeline).
    fn sample(
        &self,
        position: LatLon,
        altitude: MetersAmsl,
        valid_time: DateTime<Utc>,
    ) -> Result<Option<WindsAloft>, SourceError>;
}

/// Magnetic variation (plan §2.3: WMM-backed in strata-data, app-wired).
pub trait MagvarSource {
    /// Variation (declination) at `p` for `date`, **east positive** —
    /// `magnetic = true − variation`.
    fn magvar(&self, p: LatLon, date: NaiveDate) -> Result<MagneticVariation, SourceError>;
}

/// Borrowed bundle of every source [`compute`](crate::compute::compute)
/// needs — the app builds one per compute run.
pub struct Sources<'a> {
    pub elevation: &'a dyn ElevationSource,
    pub obstacles: &'a dyn ObstacleSource,
    pub airspaces: &'a dyn AirspaceSource,
    pub winds: &'a dyn WindsAloftSampler,
    pub magvar: &'a dyn MagvarSource,
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    /// Compile-time proof that every source trait is object-safe (the
    /// [`Sources`] bundle requires it).
    #[allow(dead_code)]
    fn assert_object_safe(
        _: &dyn ElevationSource,
        _: &dyn ObstacleSource,
        _: &dyn AirspaceSource,
        _: &dyn WindsAloftSampler,
        _: &dyn MagvarSource,
    ) {
    }

    /// Analytic synthetic terrain: elevation = lat° × 100 m.
    struct SlopeTerrain;

    impl ElevationSource for SlopeTerrain {
        fn max_elevation_at(&self, p: LatLon) -> Result<Option<MetersAmsl>, SourceError> {
            if p.lat() < 0.0 {
                return Ok(None);
            }
            Ok(Some(MetersAmsl(p.lat() * 100.0)))
        }
    }

    struct Empty;

    impl ObstacleSource for Empty {
        fn obstacles_in_bbox(&self, _: BoundingBox) -> Result<Vec<Obstacle>, SourceError> {
            Ok(Vec::new())
        }
    }

    impl AirspaceSource for Empty {
        fn airspaces_in_bbox(&self, _: BoundingBox) -> Result<Vec<Airspace>, SourceError> {
            Ok(Vec::new())
        }
    }

    /// Constant 270°/20 kt, ISA-ish temperature.
    struct WesterlyWind;

    impl WindsAloftSampler for WesterlyWind {
        fn sample(
            &self,
            _: LatLon,
            _: MetersAmsl,
            _: DateTime<Utc>,
        ) -> Result<Option<WindsAloft>, SourceError> {
            Ok(Some(WindsAloft {
                direction: DegreesTrue::new(270.0),
                speed: Knots(20.0),
                temperature: Celsius(5.0),
                temperature_provenance: Provenance::Real,
            }))
        }
    }

    /// Germany-ish constant variation, 4° east.
    struct FixedVariation;

    impl MagvarSource for FixedVariation {
        fn magvar(&self, _: LatLon, _: NaiveDate) -> Result<MagneticVariation, SourceError> {
            Ok(MagneticVariation(4.0))
        }
    }

    #[test]
    fn synthetic_sources_drive_the_trait_objects() {
        let terrain = SlopeTerrain;
        let empty = Empty;
        let wind = WesterlyWind;
        let magvar = FixedVariation;
        let sources = Sources {
            elevation: &terrain,
            obstacles: &empty,
            airspaces: &empty,
            winds: &wind,
            magvar: &magvar,
        };

        let p = LatLon::new(50.0, 10.0).unwrap();
        let elevation = sources.elevation.max_elevation_at(p).unwrap().unwrap();
        assert_eq!(elevation, MetersAmsl(5000.0));

        let bbox = BoundingBox::new(9.0, 49.0, 11.0, 51.0).unwrap();
        assert!(sources.obstacles.obstacles_in_bbox(bbox).unwrap().is_empty());
        assert!(sources.airspaces.airspaces_in_bbox(bbox).unwrap().is_empty());

        let t = chrono::Utc.with_ymd_and_hms(2026, 6, 14, 12, 0, 0).unwrap();
        let aloft = sources
            .winds
            .sample(p, MetersAmsl::from_feet(4500.0), t)
            .unwrap()
            .unwrap();
        assert_eq!(aloft.speed, Knots(20.0));

        let variation = sources
            .magvar
            .magvar(p, chrono::NaiveDate::from_ymd_opt(2026, 6, 14).unwrap())
            .unwrap();
        // True 100° with 4°E variation -> magnetic 96°.
        assert_eq!(DegreesTrue::new(100.0).to_magnetic(variation).0, 96.0);
    }

    #[test]
    fn source_error_carries_message_and_source() {
        let plain = SourceError::new("store unavailable");
        assert_eq!(plain.to_string(), "store unavailable");
        assert!(Error::source(&plain).is_none());

        let io = std::io::Error::other("disk on fire");
        let wrapped = SourceError::with_source("elevation tile read failed", io);
        assert_eq!(wrapped.to_string(), "elevation tile read failed");
        assert_eq!(Error::source(&wrapped).unwrap().to_string(), "disk on fire");
    }
}

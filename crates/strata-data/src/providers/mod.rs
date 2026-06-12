//! Provider traits — one per data type, async and object-safe. Every data
//! source stays behind a trait so sources remain swappable (licensing).

pub mod autorouter;
pub mod aviationweather;
pub mod copernicus;
pub mod dwd_icon;
pub mod dwd_radar;
pub mod openaip;
pub mod protomaps;

use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::Error;
use crate::domain::{
    Airport, Airspace, BoundingBox, Country, GriddedTimeline, IcaoCode, Metar, Navaid, Notam,
    Obstacle, ReportingPoint, Sigmet, Taf, WeatherField, WeatherGrid,
};

#[async_trait]
pub trait AirportProvider: Send + Sync {
    async fn airports(&self, country: Country) -> Result<Vec<Airport>, Error>;
}

#[async_trait]
pub trait AirspaceProvider: Send + Sync {
    async fn airspaces(&self, country: Country) -> Result<Vec<Airspace>, Error>;
}

#[async_trait]
pub trait NavaidProvider: Send + Sync {
    async fn navaids(&self, country: Country) -> Result<Vec<Navaid>, Error>;
}

#[async_trait]
pub trait ReportingPointProvider: Send + Sync {
    async fn reporting_points(&self, country: Country) -> Result<Vec<ReportingPoint>, Error>;
}

#[async_trait]
pub trait ObstacleProvider: Send + Sync {
    async fn obstacles(&self, country: Country) -> Result<Vec<Obstacle>, Error>;
}

/// Station selection for METAR/TAF fetches.
#[derive(Debug, Clone, PartialEq)]
pub enum WeatherQuery {
    /// All stations within a geographic box (viewport-driven).
    Bbox(BoundingBox),
    /// An explicit station list.
    Stations(Vec<IcaoCode>),
}

#[async_trait]
pub trait WeatherProvider: Send + Sync {
    async fn metars(&self, query: WeatherQuery) -> Result<Vec<Metar>, Error>;
    async fn tafs(&self, query: WeatherQuery) -> Result<Vec<Taf>, Error>;
    /// International SIGMETs intersecting `bbox` (callers pass the union
    /// of the enabled countries' boxes; the upstream feed is global and
    /// filtered client-side, so box size carries no API cost).
    async fn sigmets(&self, bbox: BoundingBox) -> Result<Vec<Sigmet>, Error>;
}

/// A source of gridded weather rasters (forecast model, radar composite).
///
/// Sources differ in cadence and direction of time: a forecast model serves
/// future steps from its latest run, a radar composite a window of observed
/// past frames plus a short nowcast. [`GriddedTimeline`] expresses both —
/// callers drive a time slider off `timeline()` and fetch exactly the
/// advertised steps. Implementations are pure fetch+decode; scheduling and
/// caching belong to the caller.
#[async_trait]
pub trait GriddedWeatherProvider: Send + Sync {
    /// The fields this source can supply.
    fn fields(&self) -> &[WeatherField];

    /// What is currently fetchable for `field` (run plus every valid time,
    /// ascending, each marked observed or forecast).
    async fn timeline(&self, field: WeatherField) -> Result<GriddedTimeline, Error>;

    /// Fetches and decodes the grid for `field` at `valid_time`, which must
    /// be a step advertised by [`Self::timeline`].
    async fn fetch(
        &self,
        field: WeatherField,
        valid_time: DateTime<Utc>,
    ) -> Result<WeatherGrid, Error>;
}

/// Half-open UTC time window `[from, to)` for NOTAM queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// A source of NOTAMs. Both queries return only NOTAMs whose validity
/// window (items B/C) intersects `window`; finer relevance filtering
/// (item D schedules, route geometry) is the consumer's job.
#[async_trait]
pub trait NotamProvider: Send + Sync {
    /// NOTAMs filed against any of `locations` (item A), e.g. aerodromes.
    async fn notams_by_locations(
        &self,
        locations: &[IcaoCode],
        window: TimeWindow,
    ) -> Result<Vec<Notam>, Error>;

    /// NOTAMs filed against the FIR itself (item A = the FIR): en-route,
    /// navigation-warning and area-wide NOTAMs such as ED-R activations or
    /// GNSS degradation. Aerodrome NOTAMs inside the FIR are *not*
    /// included — query their locations explicitly.
    async fn notams_by_fir(&self, fir: &IcaoCode, window: TimeWindow) -> Result<Vec<Notam>, Error>;
}

/// Identifies a 1°×1° DEM tile by the integer degrees of its south-west
/// corner (Copernicus GLO-30 tiling scheme).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DemTileId {
    pub lat_sw: i16,
    pub lon_sw: i16,
}

impl fmt::Display for DemTileId {
    /// Copernicus-style corner label, e.g. `N50E010`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ns, lat) = if self.lat_sw >= 0 {
            ('N', self.lat_sw)
        } else {
            ('S', -self.lat_sw)
        };
        let (ew, lon) = if self.lon_sw >= 0 {
            ('E', self.lon_sw)
        } else {
            ('W', -self.lon_sw)
        };
        write!(f, "{ns}{lat:02}{ew}{lon:03}")
    }
}

/// A decoded DEM raster covering one [`DemTileId`].
#[derive(Debug, Clone, PartialEq)]
pub struct DemTile {
    pub id: DemTileId,
    /// Samples per row.
    pub width: u32,
    /// Number of rows.
    pub height: u32,
    /// Elevations in meters AMSL, row-major starting at the north-west
    /// corner. `width * height` samples. Sea within published tiles is
    /// real 0.0 data; `NaN` marks no-data (voids, or whole substitute
    /// tiles for unpublished all-sea squares), rendered transparent by
    /// the hillshade tiler.
    pub elevations_m: Vec<f32>,
}

/// Minimal DEM access: enumerate the tiles covering a bbox, then fetch them.
#[async_trait]
pub trait TerrainProvider: Send + Sync {
    /// The tile ids whose 1°×1° extent intersects `bbox`.
    fn tiles_for(&self, bbox: BoundingBox) -> Vec<DemTileId>;
    async fn fetch_tile(&self, tile: DemTileId) -> Result<DemTile, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dem_tile_id_display() {
        assert_eq!(
            DemTileId {
                lat_sw: 50,
                lon_sw: 10
            }
            .to_string(),
            "N50E010"
        );
        assert_eq!(
            DemTileId {
                lat_sw: -3,
                lon_sw: -72
            }
            .to_string(),
            "S03W072"
        );
        assert_eq!(
            DemTileId {
                lat_sw: 47,
                lon_sw: 5
            }
            .to_string(),
            "N47E005"
        );
    }
}

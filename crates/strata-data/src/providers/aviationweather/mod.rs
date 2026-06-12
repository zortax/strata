//! aviationweather.gov data API client (`https://aviationweather.gov/api/data/`).
//!
//! Endpoints: `metar`, `taf` (bbox or station list), `isigmet` for
//! international SIGMETs. No API key required. Responses are mapped to the
//! domain record by record — one malformed record is skipped with a warning
//! instead of failing the whole fetch. Raw METAR/TAF text is decoded via
//! [`crate::decode`]; reports whose text fails to decode keep their raw form
//! (the raw report always stands).

mod cache;
mod metar;
mod sigmet;
mod taf;

pub use cache::{CachedWeatherProvider, DEFAULT_METAR_TTL, DEFAULT_SIGMET_TTL, DEFAULT_TAF_TTL};

use async_trait::async_trait;

use std::time::Duration;

use crate::Error;
use crate::domain::{BoundingBox, Metar, Sigmet, Taf};
use crate::providers::{WeatherProvider, WeatherQuery};

/// Provider label used in [`Error::Provider`] and log events.
const PROVIDER: &str = "aviationweather";

pub struct AviationWeatherClient {
    http: reqwest::Client,
    base_url: String,
}

impl AviationWeatherClient {
    pub const DEFAULT_BASE_URL: &'static str = "https://aviationweather.gov/api/data";

    pub fn new() -> Self {
        Self::with_base_url(Self::DEFAULT_BASE_URL)
    }

    /// Override the API root (fixture/local-server tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        // Building the client only fails when the TLS backend cannot be
        // initialized — the same condition `reqwest::Client::default()`
        // would panic on anyway.
        //
        // Timeouts are mandatory: reqwest's default is *no* timeout, so a
        // half-open connection (suspend/resume, NAT expiry) would otherwise
        // wedge the weather subsystem forever — the app's `fetching` guard
        // only resets when the request resolves. 10 s connect / 30 s total
        // is generous for the small JSON responses and well under the
        // 5-minute refresh cadence.
        let http = reqwest::Client::builder()
            .user_agent(concat!("strata-data/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        Self { http, base_url }
    }

    async fn get(
        &self,
        endpoint: &str,
        params: &[(&'static str, String)],
    ) -> Result<String, Error> {
        let url = format!("{}/{endpoint}", self.base_url);
        tracing::debug!(%url, "fetching aviationweather.gov data");
        let response = self
            .http
            .get(&url)
            .query(params)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.text().await?)
    }
}

impl Default for AviationWeatherClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WeatherProvider for AviationWeatherClient {
    async fn metars(&self, query: WeatherQuery) -> Result<Vec<Metar>, Error> {
        if query_is_empty(&query) {
            return Ok(Vec::new());
        }
        let body = self.get("metar", &report_params(&query)).await?;
        let metars = metar::parse_response(&body).map_err(|e| Error::provider(PROVIDER, e))?;
        Ok(metars.into_iter().map(metar::with_decoded).collect())
    }

    async fn tafs(&self, query: WeatherQuery) -> Result<Vec<Taf>, Error> {
        if query_is_empty(&query) {
            return Ok(Vec::new());
        }
        let body = self.get("taf", &report_params(&query)).await?;
        let tafs = taf::parse_response(&body).map_err(|e| Error::provider(PROVIDER, e))?;
        Ok(tafs.into_iter().map(taf::with_decoded).collect())
    }

    async fn sigmets(&self, bbox: BoundingBox) -> Result<Vec<Sigmet>, Error> {
        let body = self
            .get("isigmet", &[("format", "json".to_owned())])
            .await?;
        sigmet::parse_response(&body, &bbox).map_err(|e| Error::provider(PROVIDER, e))
    }
}

/// An explicit empty station list never needs a request (the API would
/// otherwise interpret the missing selector as "everything").
fn query_is_empty(query: &WeatherQuery) -> bool {
    matches!(query, WeatherQuery::Stations(stations) if stations.is_empty())
}

fn report_params(query: &WeatherQuery) -> Vec<(&'static str, String)> {
    let selector = match query {
        WeatherQuery::Bbox(bbox) => ("bbox", bbox_param(bbox)),
        WeatherQuery::Stations(stations) => (
            "ids",
            stations
                .iter()
                .map(|station| station.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
    };
    vec![("format", "json".to_owned()), selector]
}

/// `bbox` query value in the API's `south,west,north,east` order
/// (documented as `lat0,lon0,lat1,lon1`).
fn bbox_param(bbox: &BoundingBox) -> String {
    format!(
        "{},{},{},{}",
        bbox.south(),
        bbox.west(),
        bbox.north(),
        bbox.east()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::IcaoCode;

    fn icao(code: &str) -> IcaoCode {
        IcaoCode::new(code).expect("valid test ICAO code")
    }

    #[test]
    fn bbox_param_is_south_west_north_east() {
        let bbox = crate::domain::Country::DE.bounding_box();
        assert_eq!(bbox_param(&bbox), "47,5.5,55.2,15.5");
    }

    #[test]
    fn report_params_for_bbox_query() {
        let params = report_params(&WeatherQuery::Bbox(
            crate::domain::Country::DE.bounding_box(),
        ));
        assert_eq!(
            params,
            vec![
                ("format", "json".to_owned()),
                ("bbox", "47,5.5,55.2,15.5".to_owned()),
            ]
        );
    }

    #[test]
    fn report_params_join_station_ids() {
        let params = report_params(&WeatherQuery::Stations(vec![icao("EDDF"), icao("EDDM")]));
        assert_eq!(
            params,
            vec![
                ("format", "json".to_owned()),
                ("ids", "EDDF,EDDM".to_owned()),
            ]
        );
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let client = AviationWeatherClient::with_base_url("http://localhost:1/api/data/");
        assert_eq!(client.base_url, "http://localhost:1/api/data");
    }

    #[tokio::test]
    async fn empty_station_list_short_circuits_without_io() {
        // Unroutable base URL: any actual request would error, so an Ok
        // result proves the early return.
        let client = AviationWeatherClient::with_base_url("http://localhost:1");
        let metars = client
            .metars(WeatherQuery::Stations(Vec::new()))
            .await
            .expect("no request issued");
        assert!(metars.is_empty());
        let tafs = client
            .tafs(WeatherQuery::Stations(Vec::new()))
            .await
            .expect("no request issued");
        assert!(tafs.is_empty());
    }
}

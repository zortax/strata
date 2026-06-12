//! HTTP client for the openAIP core API: authenticated, paginated GETs with
//! retry/backoff, plus the per-type fetch + normalize entry points.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use crate::Error;
use crate::domain::{AiracCycle, Airport, Airspace, Country, Navaid, Obstacle, ReportingPoint};
use crate::providers::{
    AirportProvider, AirspaceProvider, NavaidProvider, ObstacleProvider, ReportingPointProvider,
};

use super::common::PROVIDER;
use super::{NormalizationReport, airports, airspaces, navaids, obstacles, reporting_points};

/// Maximum items per page the API allows (and its default).
const PAGE_LIMIT: u32 = 1000;
/// Total tries per page request, including the first.
const MAX_ATTEMPTS: u32 = 4;
/// First retry delay; doubles per attempt.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// One page of an openAIP list response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageEnvelope {
    page: u64,
    total_pages: u64,
    items: Vec<Value>,
}

pub struct OpenAipClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAipClient {
    pub const DEFAULT_BASE_URL: &'static str = "https://api.core.openaip.net/api";

    /// `api_key` comes from `OPENAIP_API_KEY` (loaded by the binaries —
    /// never log it).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, Self::DEFAULT_BASE_URL)
    }

    /// Override the API root (fixture/local-server tests).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("strata/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    /// Fetches and normalizes all airports of `country`, reporting skips.
    pub async fn fetch_airports(
        &self,
        country: Country,
    ) -> Result<(Vec<Airport>, NormalizationReport), Error> {
        let items = self.fetch_paged("airports", country).await?;
        let (airports, report) = airports::normalize(&items);
        report.log_summary("airports");
        Ok((airports, report))
    }

    /// Fetches and normalizes all airspaces of `country`, stamping the
    /// current AIRAC cycle, reporting skips.
    pub async fn fetch_airspaces(
        &self,
        country: Country,
    ) -> Result<(Vec<Airspace>, NormalizationReport), Error> {
        let items = self.fetch_paged("airspaces", country).await?;
        let airac = AiracCycle::current();
        let (airspaces, report) = airspaces::normalize(&items, Some(&airac));
        report.log_summary("airspaces");
        Ok((airspaces, report))
    }

    /// Fetches and normalizes all navaids of `country`, reporting skips.
    pub async fn fetch_navaids(
        &self,
        country: Country,
    ) -> Result<(Vec<Navaid>, NormalizationReport), Error> {
        let items = self.fetch_paged("navaids", country).await?;
        let (navaids, report) = navaids::normalize(&items);
        report.log_summary("navaids");
        Ok((navaids, report))
    }

    /// Fetches and normalizes all reporting points of `country`, reporting
    /// skips. Also fetches the country's airports to resolve the points'
    /// internal airport references to ICAO idents.
    pub async fn fetch_reporting_points(
        &self,
        country: Country,
    ) -> Result<(Vec<ReportingPoint>, NormalizationReport), Error> {
        let airport_items = self.fetch_paged("airports", country).await?;
        let icao_by_id = airports::icao_index(&airport_items);
        let items = self.fetch_paged("reporting-points", country).await?;
        let (points, report) = reporting_points::normalize(&items, &icao_by_id);
        report.log_summary("reporting points");
        Ok((points, report))
    }

    /// Fetches and normalizes all obstacles of `country`, reporting skips.
    pub async fn fetch_obstacles(
        &self,
        country: Country,
    ) -> Result<(Vec<Obstacle>, NormalizationReport), Error> {
        let items = self.fetch_paged("obstacles", country).await?;
        let (obstacles, report) = obstacles::normalize(&items);
        report.log_summary("obstacles");
        Ok((obstacles, report))
    }

    /// Walks `GET {base}/{endpoint}?country=…&page=…&limit=…` until the
    /// reported `totalPages` is reached, collecting raw items.
    #[instrument(skip(self), fields(country = country.code()))]
    async fn fetch_paged(&self, endpoint: &str, country: Country) -> Result<Vec<Value>, Error> {
        let mut items = Vec::new();
        let mut page: u64 = 1;
        loop {
            let envelope = self.get_page(endpoint, country, page).await?;
            let fetched = envelope.items.len();
            debug!(
                page = envelope.page,
                total_pages = envelope.total_pages,
                fetched,
                "fetched page"
            );
            items.extend(envelope.items);
            // The empty-page guard prevents looping on a misreported total.
            if page >= envelope.total_pages || fetched == 0 {
                break;
            }
            page += 1;
        }
        info!(total = items.len(), "fetched all pages");
        Ok(items)
    }

    /// One page request with retry/backoff on 429, 5xx, and transport
    /// errors.
    async fn get_page(
        &self,
        endpoint: &str,
        country: Country,
        page: u64,
    ) -> Result<PageEnvelope, Error> {
        let url = format!("{}/{endpoint}", self.base_url);
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let result = self
                .http
                .get(&url)
                .header("x-openaip-api-key", &self.api_key)
                .query(&[
                    ("country", country.code().to_owned()),
                    ("page", page.to_string()),
                    ("limit", PAGE_LIMIT.to_string()),
                ])
                .send()
                .await;
            let retry_in = backoff_delay(attempt);
            match result {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response.json().await?);
                    }
                    if retryable(status) && attempt < MAX_ATTEMPTS {
                        warn!(%url, page, %status, attempt, ?retry_in, "retrying after HTTP error");
                        tokio::time::sleep(retry_in).await;
                        continue;
                    }
                    return Err(Error::provider(
                        PROVIDER,
                        format!("GET {url} page {page} failed with HTTP {status}"),
                    ));
                }
                Err(err) => {
                    if attempt < MAX_ATTEMPTS {
                        warn!(%url, page, %err, attempt, ?retry_in, "retrying after transport error");
                        tokio::time::sleep(retry_in).await;
                        continue;
                    }
                    return Err(Error::Http(err));
                }
            }
        }
    }
}

fn retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Exponential backoff before retry number `attempt + 1`.
fn backoff_delay(attempt: u32) -> Duration {
    BACKOFF_BASE * 2u32.saturating_pow(attempt.saturating_sub(1))
}

#[async_trait]
impl AirportProvider for OpenAipClient {
    async fn airports(&self, country: Country) -> Result<Vec<Airport>, Error> {
        Ok(self.fetch_airports(country).await?.0)
    }
}

#[async_trait]
impl AirspaceProvider for OpenAipClient {
    async fn airspaces(&self, country: Country) -> Result<Vec<Airspace>, Error> {
        Ok(self.fetch_airspaces(country).await?.0)
    }
}

#[async_trait]
impl NavaidProvider for OpenAipClient {
    async fn navaids(&self, country: Country) -> Result<Vec<Navaid>, Error> {
        Ok(self.fetch_navaids(country).await?.0)
    }
}

#[async_trait]
impl ReportingPointProvider for OpenAipClient {
    async fn reporting_points(&self, country: Country) -> Result<Vec<ReportingPoint>, Error> {
        Ok(self.fetch_reporting_points(country).await?.0)
    }
}

#[async_trait]
impl ObstacleProvider for OpenAipClient {
    async fn obstacles(&self, country: Country) -> Result<Vec<Obstacle>, Error> {
        Ok(self.fetch_obstacles(country).await?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_envelope_deserializes_from_fixture() {
        let envelope: PageEnvelope = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openaip/airports.json"
        ))
        .expect("fixture parses as page envelope");
        assert_eq!(envelope.page, 1);
        assert_eq!(envelope.total_pages, 46);
        assert_eq!(envelope.items.len(), 30);
    }

    #[test]
    fn retryable_statuses() {
        assert!(retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(retryable(StatusCode::BAD_GATEWAY));
        assert!(!retryable(StatusCode::UNAUTHORIZED));
        assert!(!retryable(StatusCode::NOT_FOUND));
    }

    #[test]
    fn backoff_doubles_per_attempt() {
        assert_eq!(backoff_delay(1), Duration::from_millis(500));
        assert_eq!(backoff_delay(2), Duration::from_millis(1000));
        assert_eq!(backoff_delay(3), Duration::from_millis(2000));
    }

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        let client = OpenAipClient::with_base_url("key", "http://localhost:1234/api/");
        assert_eq!(client.base_url, "http://localhost:1234/api");
        let default = OpenAipClient::new("key");
        assert_eq!(default.base_url, OpenAipClient::DEFAULT_BASE_URL);
    }
}

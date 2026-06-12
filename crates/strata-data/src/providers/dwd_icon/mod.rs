//! DWD ICON-D2 open-data gridded forecast provider
//! (`https://opendata.dwd.de/weather/nwp/icon-d2/grib/`). No API key.
//!
//! Layout, verified live on 2026-06-10 (pressure-level files 2026-06-11):
//! runs at 00/03/…/21 UTC (published ~1 h after run time), 49 hourly steps
//! `000..=048` per field, one `.grib2.bz2` file per step under
//! `{HH}/{field}/` — see [`files`] for the exact naming (single-level vs
//! pressure-level patterns) and [`decode`] for the GRIB2 specifics
//! (multi-message 15-minute sub-steps in `tot_prec`/`lpi`/`hzerocl`,
//! run-start accumulation for `tot_prec`, Section-6 bitmap → NaN).
//!
//! Field mapping: clct/clcl/clcm/clch → cloud cover, tot_prec (differenced)
//! → precipitation rate, lpi → thunderstorm potential, cape_ml → CAPE,
//! ceiling → ceiling, vis → visibility, u/v/t at 950/850/700/500 hPa →
//! winds/temperature aloft, hzerocl → freezing level.
//!
//! Units are normalized to the [`WeatherField`] contract at fetch time:
//! u/v arrive in m/s and hzerocl in meters AMSL (passed through);
//! temperature arrives in Kelvin and is converted to °C
//! ([`decode::kelvin_to_celsius`]).
//!
//! The provider is a pure fetch+decode — no disk caching, no scheduling;
//! the app layer owns refresh cadence and caching. DWD open data requires
//! source attribution ("DWD") in the UI.

mod decode;
mod files;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::Error;
use crate::domain::{
    GeoError, GriddedError, GriddedTimeline, StepKind, TimelineStep, WeatherField, WeatherGrid,
};
use crate::providers::GriddedWeatherProvider;

/// Provider label used in [`Error::Provider`] and log events.
const PROVIDER: &str = "dwd_icon";

/// Errors internal to this provider; converted to [`Error::Provider`]
/// (provider name `"dwd_icon"`) at the crate boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DwdIconError {
    #[error("bzip2 decompression: {0}")]
    Bzip2(#[source] std::io::Error),
    #[error("grib2: {0}")]
    Grib(#[from] grib::GribError),
    #[error("unsupported grid: {0}")]
    UnsupportedGrid(&'static str),
    #[error("unsupported forecast time unit")]
    UnsupportedTimeUnit,
    #[error("invalid time fields in grib message")]
    InvalidTime,
    #[error("no grib message valid at {valid_time}")]
    MessageNotFound { valid_time: DateTime<Utc> },
    #[error("decoded {got} values for a {ni}x{nj} grid")]
    ValueCountMismatch { got: usize, ni: usize, nj: usize },
    #[error("accumulation grids of consecutive steps do not align")]
    GridMismatch,
    #[error("{field}: valid time {valid_time} is not a fetchable step of run {run}")]
    InvalidValidTime {
        field: WeatherField,
        valid_time: DateTime<Utc>,
        run: DateTime<Utc>,
    },
    #[error("no published ICON-D2 run found (probed the {tried} newest runs)")]
    NoRunAvailable { tried: usize },
    #[error("grid: {0}")]
    Gridded(#[from] GriddedError),
    #[error("geo: {0}")]
    Geo(#[from] GeoError),
}

impl From<DwdIconError> for Error {
    fn from(err: DwdIconError) -> Self {
        Error::provider(PROVIDER, err)
    }
}

/// How long a probed newest-run answer is reused per field. Short enough
/// that a freshly published run (every 3 h) is picked up promptly, long
/// enough that a burst of fetches (the flight prefetch pulls up to 52
/// files per window) probes once instead of 1–3 HEAD requests per file.
const RUN_PROBE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// ICON-D2 gridded forecast client.
pub struct DwdIconD2 {
    http: reqwest::Client,
    base_url: String,
    /// Memoized [`Self::latest_run`] probe per field (never held across an
    /// await).
    run_cache: std::sync::Mutex<std::collections::HashMap<WeatherField, (std::time::Instant, DateTime<Utc>)>>,
}

impl DwdIconD2 {
    pub const DEFAULT_BASE_URL: &'static str = "https://opendata.dwd.de/weather/nwp/icon-d2/grib";

    pub fn new() -> Self {
        Self::with_base_url(Self::DEFAULT_BASE_URL)
    }

    /// Override the open-data root (fixture/local-server tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        // Step files are 0.03–3.3 MB; connect/read-inactivity timeouts kill
        // stalled connections (reqwest defaults to none) without capping
        // slow-but-progressing downloads. Falling back to the default
        // client only happens when TLS init fails, where `Client::new()`
        // would panic anyway.
        let http = reqwest::Client::builder()
            .user_agent(concat!("strata-data/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        Self {
            http,
            base_url,
            run_cache: std::sync::Mutex::default(),
        }
    }

    /// The newest fully published run for `field`: probes the *last* step
    /// (048) of the newest candidate runs so a run that is still uploading
    /// is skipped, and falls back up to two runs on 404. Successful probes
    /// are memoized for [`RUN_PROBE_TTL`] per field.
    async fn latest_run(&self, field: WeatherField) -> Result<DateTime<Utc>, Error> {
        if let Ok(cache) = self.run_cache.lock()
            && let Some((probed_at, run)) = cache.get(&field)
            && probed_at.elapsed() < RUN_PROBE_TTL
        {
            return Ok(*run);
        }
        let candidates = files::run_candidates(Utc::now());
        let tried = candidates.len();
        for run in candidates {
            let url = files::step_url(&self.base_url, field, run, files::FORECAST_HOURS);
            tracing::debug!(%url, "probing ICON-D2 run");
            let response = self.http.head(&url).send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                continue;
            }
            response.error_for_status()?;
            tracing::debug!(%run, %field, "selected ICON-D2 run");
            if let Ok(mut cache) = self.run_cache.lock() {
                cache.insert(field, (std::time::Instant::now(), run));
            }
            return Ok(run);
        }
        Err(DwdIconError::NoRunAvailable { tried }.into())
    }

    /// Downloads and decompresses one step file.
    async fn fetch_step_bytes(
        &self,
        field: WeatherField,
        run: DateTime<Utc>,
        step: u32,
    ) -> Result<Vec<u8>, Error> {
        let url = files::step_url(&self.base_url, field, run, step);
        tracing::debug!(%url, "downloading ICON-D2 step");
        let compressed = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        // Decompression of a few MB takes single-digit milliseconds;
        // acceptable inline (callers run on background executors).
        Ok(decode::decompress_bz2(&compressed)?)
    }

    /// Fetches the grid of one instantaneous step (everything except the
    /// differenced precipitation rate).
    async fn fetch_instant(
        &self,
        field: WeatherField,
        run: DateTime<Utc>,
        step: u32,
        valid_time: DateTime<Utc>,
    ) -> Result<crate::domain::RegularLatLonGrid, Error> {
        let bytes = self.fetch_step_bytes(field, run, step).await?;
        Ok(decode::decode_grid_at(bytes, valid_time)?)
    }

    /// Precipitation rate at `valid_time`: tot_prec is an accumulation from
    /// run start, so the rate is the difference between the accumulations
    /// of this step and the previous one (fetched concurrently).
    async fn fetch_precip_rate(
        &self,
        run: DateTime<Utc>,
        step: u32,
        valid_time: DateTime<Utc>,
    ) -> Result<crate::domain::RegularLatLonGrid, Error> {
        let field = WeatherField::PrecipRate;
        let (prev_bytes, curr_bytes) = futures::future::try_join(
            self.fetch_step_bytes(field, run, step - 1),
            self.fetch_step_bytes(field, run, step),
        )
        .await?;
        let prev_valid = valid_time - Duration::hours(1);
        let prev = decode::decode_grid_at(prev_bytes, prev_valid)?;
        let curr = decode::decode_grid_at(curr_bytes, valid_time)?;
        Ok(decode::accumulation_rate_mm_h(&prev, &curr)?)
    }
}

impl Default for DwdIconD2 {
    fn default() -> Self {
        Self::new()
    }
}

/// The hourly forecast steps of `run` that are fetchable for `field` (all
/// [`StepKind::Forecast`]; precipitation rate starts at step 1 because it
/// is differenced against the previous accumulation).
fn timeline_for_run(field: WeatherField, run: DateTime<Utc>) -> GriddedTimeline {
    let steps = (files::first_step(field)..=files::FORECAST_HOURS)
        .map(|h| TimelineStep {
            valid_time: run + Duration::hours(i64::from(h)),
            kind: StepKind::Forecast,
        })
        .collect();
    GriddedTimeline { run_time: run, steps }
}

#[async_trait]
impl GriddedWeatherProvider for DwdIconD2 {
    fn fields(&self) -> &[WeatherField] {
        &WeatherField::ALL
    }

    async fn timeline(&self, field: WeatherField) -> Result<GriddedTimeline, Error> {
        let run = self.latest_run(field).await?;
        Ok(timeline_for_run(field, run))
    }

    async fn fetch(
        &self,
        field: WeatherField,
        valid_time: DateTime<Utc>,
    ) -> Result<WeatherGrid, Error> {
        let run = self.latest_run(field).await?;
        let step = files::step_for(field, run, valid_time)?;
        let grid = match field {
            WeatherField::PrecipRate => self.fetch_precip_rate(run, step, valid_time).await?,
            // ICON-D2 publishes temperature in Kelvin; the WeatherField
            // contract is °C.
            WeatherField::Temperature(_) => {
                let kelvin = self.fetch_instant(field, run, step, valid_time).await?;
                decode::kelvin_to_celsius(&kelvin)?
            }
            _ => self.fetch_instant(field, run, step, valid_time).await?,
        };
        Ok(WeatherGrid {
            field,
            run_time: run,
            valid_time,
            grid,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 10, h, 0, 0).unwrap()
    }

    #[test]
    fn provider_serves_every_field() {
        let provider = DwdIconD2::new();
        assert_eq!(provider.fields(), WeatherField::ALL);
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let provider = DwdIconD2::with_base_url("http://localhost:1/grib/");
        assert_eq!(provider.base_url, "http://localhost:1/grib");
    }

    #[test]
    fn timeline_covers_49_hourly_forecast_steps() {
        let tl = timeline_for_run(WeatherField::CloudCover, t(12));
        assert_eq!(tl.run_time, t(12));
        assert_eq!(tl.steps.len(), 49);
        assert_eq!(tl.steps[0].valid_time, t(12));
        assert_eq!(tl.steps[48].valid_time, t(12) + Duration::hours(48));
        assert!(tl.steps.iter().all(|s| s.kind == StepKind::Forecast));
    }

    #[test]
    fn precip_rate_timeline_starts_one_hour_in() {
        let tl = timeline_for_run(WeatherField::PrecipRate, t(12));
        assert_eq!(tl.steps.len(), 48);
        assert_eq!(tl.steps[0].valid_time, t(13));
    }

    #[test]
    fn upper_air_timelines_cover_the_full_run() {
        use crate::domain::PressureLevel;

        for field in [
            WeatherField::WindU(PressureLevel::P850),
            WeatherField::WindV(PressureLevel::P500),
            WeatherField::Temperature(PressureLevel::P950),
            WeatherField::FreezingLevel,
        ] {
            let tl = timeline_for_run(field, t(12));
            assert_eq!(tl.steps.len(), 49, "{field}");
            assert_eq!(tl.steps[0].valid_time, t(12), "{field}");
            assert_eq!(tl.steps[48].valid_time, t(12) + Duration::hours(48), "{field}");
        }
    }
}

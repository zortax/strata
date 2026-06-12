//! DWD RV radar composite provider
//! (`https://opendata.dwd.de/weather/radar/composite/rv/`). No API key.
//!
//! Observed precipitation: a quality-checked radar analysis plus a +2 h
//! nowcast, published every 5 minutes as a `.tar.bz2` of 25 RADOLAN frames
//! (`_000` analysis, `_005`..`_120` nowcast) on the DE1200 grid — a 1 km
//! polar-stereographic raster (WGS84 ellipsoid variant) that [`resample`]
//! reprojects onto the same [`RegularLatLonGrid`](crate::domain::RegularLatLonGrid)
//! convention the ICON forecast provider uses, so consumers never see the
//! source projection. See [`files`] for the server layout, [`radolan`] for
//! the frame format and [`projection`] for the verified DE1200 parameters.
//!
//! Time is past-anchored: the timeline advertises a 2 h observed window
//! (analysis frames of older tarballs, retained ~2 days on the server)
//! plus the 2 h nowcast of the newest tarball, all on a 5-minute lattice.
//! [`recent_composite_times`] enumerates the tarball timestamps behind the
//! observed window for callers that page further back.
//!
//! The provider is a pure fetch+decode — no disk caching, no scheduling;
//! the app layer owns refresh cadence and caching. DWD open data requires
//! source attribution ("DWD") in the UI.

mod archive;
mod files;
mod projection;
mod radolan;
mod resample;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::Error;
use crate::domain::{GeoError, GriddedError, GriddedTimeline, WeatherField, WeatherGrid};
use crate::providers::GriddedWeatherProvider;

/// Provider label used in [`Error::Provider`] and log events.
const PROVIDER: &str = "dwd_radar";

/// The single field the radar composite serves.
const FIELDS: [WeatherField; 1] = [WeatherField::PrecipRate];

/// Errors internal to this provider; converted to [`Error::Provider`]
/// (provider name `"dwd_radar"`) at the crate boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DwdRadarError {
    #[error("tar.bz2 archive: {0}")]
    Archive(#[source] std::io::Error),
    #[error("archive member {name} not found")]
    MemberNotFound { name: String },
    #[error("radolan header: {0}")]
    InvalidHeader(&'static str),
    #[error("invalid measurement timestamp in radolan header")]
    InvalidTimestamp,
    #[error("payload holds {got} bytes for a {nx}x{ny} grid ({expected} expected)")]
    PayloadSizeMismatch {
        got: usize,
        expected: usize,
        nx: usize,
        ny: usize,
    },
    #[error("unexpected product {product:?} (expected RV)")]
    UnexpectedProduct { product: String },
    #[error("frame is valid at {got}, requested {requested}")]
    FrameTimeMismatch {
        got: DateTime<Utc>,
        requested: DateTime<Utc>,
    },
    #[error("{field} is not served by the radar composite")]
    UnsupportedField { field: WeatherField },
    #[error("valid time {valid_time} is not a fetchable step for analysis {analysis}")]
    InvalidValidTime {
        valid_time: DateTime<Utc>,
        analysis: DateTime<Utc>,
    },
    #[error("no published RV composite found (probed the {tried} newest timestamps)")]
    NoCompositeAvailable { tried: usize },
    #[error("grid: {0}")]
    Gridded(#[from] GriddedError),
    #[error("geo: {0}")]
    Geo(#[from] GeoError),
}

impl From<DwdRadarError> for Error {
    fn from(err: DwdRadarError) -> Self {
        Error::provider(PROVIDER, err)
    }
}

/// DWD RV radar composite client (observed precipitation + 2 h nowcast).
pub struct DwdRadarRv {
    http: reqwest::Client,
    base_url: String,
}

impl DwdRadarRv {
    pub const DEFAULT_BASE_URL: &'static str =
        "https://opendata.dwd.de/weather/radar/composite/rv";

    pub fn new() -> Self {
        Self::with_base_url(Self::DEFAULT_BASE_URL)
    }

    /// Override the open-data root (fixture/local-server tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        // Tarballs are ~2.4 MB; connect/read-inactivity timeouts kill
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
        Self { http, base_url }
    }

    /// The newest published composite: probes the expected newest 5-minute
    /// timestamps (publication lags ~3 min) and walks back on 404.
    async fn latest_analysis(&self) -> Result<DateTime<Utc>, Error> {
        let candidates = files::composite_candidates(Utc::now());
        let tried = candidates.len();
        for analysis in candidates {
            let url = files::tarball_url(&self.base_url, analysis);
            tracing::debug!(%url, "probing RV composite");
            let response = self.http.head(&url).send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                continue;
            }
            response.error_for_status()?;
            tracing::debug!(%analysis, "selected RV composite");
            return Ok(analysis);
        }
        Err(DwdRadarError::NoCompositeAvailable { tried }.into())
    }

    /// Downloads one composite tarball.
    async fn fetch_tarball(&self, analysis: DateTime<Utc>) -> Result<Vec<u8>, Error> {
        let url = files::tarball_url(&self.base_url, analysis);
        tracing::debug!(%url, "downloading RV composite");
        let bytes = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(bytes.to_vec())
    }
}

impl Default for DwdRadarRv {
    fn default() -> Self {
        Self::new()
    }
}

/// The 5-minute-aligned composite timestamps within `window` before
/// `latest` (which is floored onto the 5-minute lattice first), ascending
/// and including `latest` itself. Each timestamp names one tarball whose
/// `_000` member is the observed analysis frame for that instant — the
/// paging primitive for histories longer than the advertised 2 h window
/// (the server retains ~2 days).
pub fn recent_composite_times(latest: DateTime<Utc>, window: Duration) -> Vec<DateTime<Utc>> {
    let latest = files::floor_to_step(latest);
    let mut times = Vec::new();
    let mut t = latest;
    while latest - t <= window {
        times.push(t);
        t -= Duration::minutes(i64::from(files::STEP_MINUTES));
    }
    times.reverse();
    times
}

/// Decodes one RADOLAN frame into a lat-lon precipitation-rate grid,
/// verifying it is the RV product valid at `valid_time`.
fn decode_frame(
    frame_bytes: &[u8],
    valid_time: DateTime<Utc>,
) -> Result<crate::domain::RegularLatLonGrid, DwdRadarError> {
    let frame = radolan::parse_frame(frame_bytes)?;
    if frame.product != "RV" {
        return Err(DwdRadarError::UnexpectedProduct { product: frame.product });
    }
    if frame.valid_time() != valid_time {
        return Err(DwdRadarError::FrameTimeMismatch {
            got: frame.valid_time(),
            requested: valid_time,
        });
    }
    resample::resample_to_latlon(&frame.rate_mm_h(), frame.nx, frame.ny)
}

#[async_trait]
impl GriddedWeatherProvider for DwdRadarRv {
    fn fields(&self) -> &[WeatherField] {
        &FIELDS
    }

    async fn timeline(&self, field: WeatherField) -> Result<GriddedTimeline, Error> {
        if field != WeatherField::PrecipRate {
            return Err(DwdRadarError::UnsupportedField { field }.into());
        }
        let analysis = self.latest_analysis().await?;
        Ok(files::timeline_for(analysis))
    }

    async fn fetch(
        &self,
        field: WeatherField,
        valid_time: DateTime<Utc>,
    ) -> Result<WeatherGrid, Error> {
        if field != WeatherField::PrecipRate {
            return Err(DwdRadarError::UnsupportedField { field }.into());
        }
        let analysis = self.latest_analysis().await?;
        let location = files::locate_frame(analysis, valid_time)?;
        let tarball = self.fetch_tarball(location.tarball_time).await?;
        let member = files::member_name(location.tarball_time, location.forecast_minutes);
        // Unpack + decode + reproject take tens of milliseconds for a
        // ~2.6 MB frame; acceptable inline (callers run on background
        // executors).
        let frame_bytes = archive::extract_member(&tarball, &member)?;
        let grid = decode_frame(&frame_bytes, valid_time)?;
        Ok(WeatherGrid {
            field,
            // For nowcast steps this is the newest analysis; for observed
            // past steps the older tarball's own analysis (== valid_time),
            // which is the run the data genuinely belongs to.
            run_time: location.tarball_time,
            valid_time,
            grid,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use crate::domain::LatLon;

    use super::radolan::testutil::{RV_FIXTURE, decompress, synthetic_frame};
    use super::*;

    fn analysis_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 10, 17, 10, 0).unwrap()
    }

    #[test]
    fn provider_serves_only_precip_rate() {
        let provider = DwdRadarRv::new();
        assert_eq!(provider.fields(), [WeatherField::PrecipRate]);
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let provider = DwdRadarRv::with_base_url("http://localhost:1/rv/");
        assert_eq!(provider.base_url, "http://localhost:1/rv");
    }

    #[test]
    fn recent_composite_times_walk_back_ascending() {
        let latest = Utc.with_ymd_and_hms(2026, 6, 10, 17, 13, 42).unwrap();
        let times = recent_composite_times(latest, Duration::minutes(15));
        assert_eq!(
            times,
            vec![
                Utc.with_ymd_and_hms(2026, 6, 10, 16, 55, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 6, 10, 17, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 6, 10, 17, 5, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 6, 10, 17, 10, 0).unwrap(),
            ]
        );
        assert!(recent_composite_times(latest, Duration::minutes(-1)).is_empty());
    }

    #[test]
    fn frames_with_wrong_product_or_time_are_rejected() {
        // The synthetic frame is valid at analysis + 30 min.
        let bytes = synthetic_frame([0; 6]);
        assert!(decode_frame(&bytes, analysis_time() + Duration::minutes(30)).is_ok());
        assert!(matches!(
            decode_frame(&bytes, analysis_time()),
            Err(DwdRadarError::FrameTimeMismatch { .. })
        ));
        let mut wrong_product = bytes;
        wrong_product[0] = b'W';
        wrong_product[1] = b'N';
        assert!(matches!(
            decode_frame(&wrong_product, analysis_time() + Duration::minutes(30)),
            Err(DwdRadarError::UnexpectedProduct { product }) if product == "WN"
        ));
    }

    /// End-to-end pipeline on the real analysis frame of the tarball
    /// published 2026-06-10 17:10 UTC: parse → mm/h → reproject. The
    /// reference numbers were computed independently with a Python
    /// implementation of the documented DE1200 projection against the
    /// same bytes.
    #[test]
    fn real_frame_decodes_to_a_latlon_rate_grid() {
        let grid = decode_frame(&decompress(RV_FIXTURE), analysis_time()).unwrap();

        // Deterministic target lattice over the DE1200 extent at the
        // chosen 0.01° × 0.015° spacing.
        assert_eq!(grid.ni(), 1152);
        assert_eq!(grid.nj(), 1055);
        assert!((grid.origin().lat() - 45.68).abs() < 1e-9);
        assert!((grid.origin().lon() - 1.47).abs() < 1e-9);
        let extent = grid.extent();
        assert!((extent.north() - 56.22).abs() < 1e-9);
        assert!((extent.east() - 18.735).abs() < 1e-9);

        // Rates stay within the frame's decoded range (max raw value 635
        // → 6.35 mm / 5 min → 76.2 mm/h) and roughly half the bounding
        // box lies outside radar coverage.
        let real: Vec<f32> = grid.values().iter().copied().filter(|v| !v.is_nan()).collect();
        let nan_fraction = 1.0 - real.len() as f64 / grid.values().len() as f64;
        assert!((0.3..0.6).contains(&nan_fraction), "NaN fraction {nan_fraction}");
        assert!(real.iter().all(|&v| (0.0..=76.2001).contains(&v)));

        // The heaviest cell of this frame sits in the Eifel; bilinear
        // resampling keeps its neighborhood heavy (Python reference:
        // 72.48 mm/h at the source pixel center).
        let peak = grid
            .sample(LatLon::new(49.86062102, 6.82603986).unwrap())
            .expect("peak inside coverage");
        assert!((60.0..=76.2001).contains(&peak), "peak sample {peak}");

        // Major cities lie well inside radar coverage (dry ≠ no-data).
        for (lat, lon) in [(52.52, 13.405), (53.55, 9.99), (48.137, 11.575)] {
            let v = grid
                .sample(LatLon::new(lat, lon).unwrap())
                .expect("city inside coverage");
            assert!(v >= 0.0);
        }

        // The far corners of the lat-lon bounding box fall outside the
        // rotated projected rectangle → no data at all.
        assert_eq!(grid.sample(grid.extent().south_west()), None);
        assert_eq!(grid.sample(grid.extent().north_east()), None);
    }
}

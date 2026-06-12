//! In-memory TTL cache around any [`WeatherProvider`]. Weather is dynamic
//! data: cached briefly in memory, never persisted as authoritative.
//!
//! Entries are kept past their TTL so a failing upstream can be bridged
//! with stale data (logged as a warning) instead of an error. Concurrent
//! misses for the same key may fetch twice — harmless for this data volume.

use std::future::Future;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::Error;
use crate::domain::{BoundingBox, Metar, Sigmet, Taf};
use crate::providers::{WeatherProvider, WeatherQuery};

/// Spec default: METARs refresh every 5 minutes.
pub const DEFAULT_METAR_TTL: Duration = Duration::from_secs(5 * 60);
/// Spec default: TAFs refresh every 15 minutes.
pub const DEFAULT_TAF_TTL: Duration = Duration::from_secs(15 * 60);
/// Spec default: SIGMETs refresh every 5 minutes.
pub const DEFAULT_SIGMET_TTL: Duration = Duration::from_secs(5 * 60);

/// Distinct request keys kept per data kind; beyond this the entry with the
/// oldest fetch time is evicted (viewport panning produces a stream of
/// distinct bbox keys).
const MAX_ENTRIES_PER_KIND: usize = 16;

type NowFn = Box<dyn Fn() -> Instant + Send + Sync>;

/// TTL caching wrapper around any [`WeatherProvider`], keyed by request.
pub struct CachedWeatherProvider<P> {
    inner: P,
    ttls: Ttls,
    cache: Mutex<CacheState>,
    now: NowFn,
}

#[derive(Debug, Clone, Copy)]
struct Ttls {
    metars: Duration,
    tafs: Duration,
    sigmets: Duration,
}

#[derive(Default)]
struct CacheState {
    metars: Vec<Entry<WeatherQuery, Vec<Metar>>>,
    tafs: Vec<Entry<WeatherQuery, Vec<Taf>>>,
    sigmets: Vec<Entry<BoundingBox, Vec<Sigmet>>>,
}

struct Entry<K, V> {
    key: K,
    fetched_at: Instant,
    value: V,
}

impl<P: WeatherProvider> CachedWeatherProvider<P> {
    /// One TTL for all three data kinds. Prefer [`Self::with_default_ttls`]
    /// for the spec'd per-kind values.
    pub fn new(inner: P, ttl: Duration) -> Self {
        Self::with_ttls(inner, ttl, ttl, ttl)
    }

    /// The spec defaults: METARs 5 min, TAFs 15 min, SIGMETs 5 min.
    pub fn with_default_ttls(inner: P) -> Self {
        Self::with_ttls(
            inner,
            DEFAULT_METAR_TTL,
            DEFAULT_TAF_TTL,
            DEFAULT_SIGMET_TTL,
        )
    }

    pub fn with_ttls(
        inner: P,
        metar_ttl: Duration,
        taf_ttl: Duration,
        sigmet_ttl: Duration,
    ) -> Self {
        Self {
            inner,
            ttls: Ttls {
                metars: metar_ttl,
                tafs: taf_ttl,
                sigmets: sigmet_ttl,
            },
            cache: Mutex::new(CacheState::default()),
            now: Box::new(Instant::now),
        }
    }

    /// Replaces the time source (deterministic TTL tests).
    #[cfg(test)]
    fn with_clock(mut self, now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    /// Drops all cached entries (the UI's "refresh now" path).
    pub fn invalidate(&self) {
        let mut cache = self.cache.lock();
        cache.metars.clear();
        cache.tafs.clear();
        cache.sigmets.clear();
    }

    /// Serves `key` from cache while fresh; otherwise awaits `fetch` and
    /// stores the result. On a fetch error an expired entry is served stale
    /// with a warning; the error surfaces only when nothing is cached.
    async fn cached<K, V, F>(
        &self,
        select: fn(&mut CacheState) -> &mut Vec<Entry<K, V>>,
        ttl: Duration,
        key: K,
        what: &'static str,
        fetch: F,
    ) -> Result<V, Error>
    where
        K: PartialEq + Clone + Send,
        V: Clone + Send,
        F: Future<Output = Result<V, Error>> + Send,
    {
        {
            let now = (self.now)();
            let mut cache = self.cache.lock();
            let entries = select(&mut cache);
            if let Some(entry) = entries.iter().find(|entry| entry.key == key)
                && now.duration_since(entry.fetched_at) < ttl
            {
                return Ok(entry.value.clone());
            }
        } // — lock dropped before awaiting the fetch

        match fetch.await {
            Ok(value) => {
                let fetched_at = (self.now)();
                let mut cache = self.cache.lock();
                store(select(&mut cache), key, fetched_at, value.clone());
                Ok(value)
            }
            Err(error) => {
                let mut cache = self.cache.lock();
                let entries = select(&mut cache);
                match entries.iter().find(|entry| entry.key == key) {
                    Some(entry) => {
                        tracing::warn!(%error, what, "weather fetch failed; serving stale cached data");
                        Ok(entry.value.clone())
                    }
                    None => Err(error),
                }
            }
        }
    }
}

fn store<K: PartialEq, V>(entries: &mut Vec<Entry<K, V>>, key: K, fetched_at: Instant, value: V) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.key == key) {
        entry.fetched_at = fetched_at;
        entry.value = value;
        return;
    }
    if entries.len() >= MAX_ENTRIES_PER_KIND
        && let Some(oldest) = entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.fetched_at)
            .map(|(index, _)| index)
    {
        entries.swap_remove(oldest);
    }
    entries.push(Entry {
        key,
        fetched_at,
        value,
    });
}

#[async_trait]
impl<P: WeatherProvider> WeatherProvider for CachedWeatherProvider<P> {
    async fn metars(&self, query: WeatherQuery) -> Result<Vec<Metar>, Error> {
        self.cached(
            |cache| &mut cache.metars,
            self.ttls.metars,
            query.clone(),
            "metars",
            self.inner.metars(query),
        )
        .await
    }

    async fn tafs(&self, query: WeatherQuery) -> Result<Vec<Taf>, Error> {
        self.cached(
            |cache| &mut cache.tafs,
            self.ttls.tafs,
            query.clone(),
            "tafs",
            self.inner.tafs(query),
        )
        .await
    }

    async fn sigmets(&self, bbox: BoundingBox) -> Result<Vec<Sigmet>, Error> {
        self.cached(
            |cache| &mut cache.sigmets,
            self.ttls.sigmets,
            bbox,
            "sigmets",
            self.inner.sigmets(bbox),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use chrono::DateTime;

    use super::*;
    use crate::domain::{IcaoCode, LatLon, Polygon, SigmetHazard, TafGroup};

    #[derive(Default)]
    struct FakeProvider {
        metar_calls: AtomicUsize,
        taf_calls: AtomicUsize,
        sigmet_calls: AtomicUsize,
        fail: AtomicBool,
    }

    impl FakeProvider {
        fn check_failure(&self) -> Result<(), Error> {
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::provider("fake", "upstream down"))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl WeatherProvider for FakeProvider {
        async fn metars(&self, _query: WeatherQuery) -> Result<Vec<Metar>, Error> {
            self.metar_calls.fetch_add(1, Ordering::SeqCst);
            self.check_failure()?;
            Ok(vec![sample_metar()])
        }

        async fn tafs(&self, _query: WeatherQuery) -> Result<Vec<Taf>, Error> {
            self.taf_calls.fetch_add(1, Ordering::SeqCst);
            self.check_failure()?;
            Ok(vec![sample_taf()])
        }

        async fn sigmets(&self, _bbox: BoundingBox) -> Result<Vec<Sigmet>, Error> {
            self.sigmet_calls.fetch_add(1, Ordering::SeqCst);
            self.check_failure()?;
            Ok(vec![sample_sigmet()])
        }
    }

    fn icao(code: &str) -> IcaoCode {
        IcaoCode::new(code).expect("valid test ICAO code")
    }

    fn sample_metar() -> Metar {
        Metar {
            raw: "METAR EDDF 092320Z 25004KT CAVOK 14/10 Q1014 NOSIG".to_owned(),
            station: icao("EDDF"),
            observed_at: DateTime::from_timestamp(1_781_047_200, 0).expect("valid epoch"),
            decoded: None,
        }
    }

    fn sample_taf() -> Taf {
        let at = DateTime::from_timestamp(1_781_049_600, 0).expect("valid epoch");
        Taf {
            raw: "TAF EDDF 092300Z 1000/1024 22005KT CAVOK".to_owned(),
            station: icao("EDDF"),
            issued_at: at,
            valid_from: at,
            valid_to: at + chrono::Duration::hours(24),
            base: TafGroup {
                wind: None,
                visibility: None,
                weather: Vec::new(),
                clouds: Vec::new(),
            },
            changes: Vec::new(),
        }
    }

    fn sample_sigmet() -> Sigmet {
        let p = |lat, lon| LatLon::new(lat, lon).expect("valid test point");
        let at = DateTime::from_timestamp(1_781_046_000, 0).expect("valid epoch");
        Sigmet {
            fir: "EDGG".to_owned(),
            hazard: SigmetHazard::Thunderstorm,
            geometry: Polygon::new(
                vec![p(48.0, 8.0), p(49.0, 8.0), p(49.0, 9.0)],
                Vec::new(),
            )
            .expect("valid test polygon"),
            valid_from: at,
            valid_to: at + chrono::Duration::hours(4),
            raw: "WSDL31 EDGG ...".to_owned(),
        }
    }

    /// A controllable clock: bump the returned offset to advance time.
    fn test_clock() -> (
        Arc<Mutex<Duration>>,
        impl Fn() -> Instant + Send + Sync + 'static,
    ) {
        let start = Instant::now();
        let offset = Arc::new(Mutex::new(Duration::ZERO));
        let handle = Arc::clone(&offset);
        (offset, move || start + *handle.lock())
    }

    fn bbox_query() -> WeatherQuery {
        WeatherQuery::Bbox(de_bbox())
    }

    fn de_bbox() -> crate::domain::BoundingBox {
        crate::domain::Country::DE.bounding_box()
    }

    fn stations_query() -> WeatherQuery {
        WeatherQuery::Stations(vec![icao("EDDF")])
    }

    #[tokio::test]
    async fn fresh_entry_is_served_without_refetching() {
        let (_offset, clock) = test_clock();
        let cached = CachedWeatherProvider::new(FakeProvider::default(), DEFAULT_METAR_TTL)
            .with_clock(clock);

        let first = cached.metars(bbox_query()).await.expect("fetch succeeds");
        let second = cached.metars(bbox_query()).await.expect("served cached");
        assert_eq!(first, second);
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_entry_is_refetched() {
        let (offset, clock) = test_clock();
        let cached = CachedWeatherProvider::new(FakeProvider::default(), DEFAULT_METAR_TTL)
            .with_clock(clock);

        cached.metars(bbox_query()).await.expect("fetch succeeds");
        *offset.lock() = DEFAULT_METAR_TTL + Duration::from_secs(1);
        cached.metars(bbox_query()).await.expect("refetched");
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_is_keyed_by_request() {
        let (_offset, clock) = test_clock();
        let cached = CachedWeatherProvider::new(FakeProvider::default(), DEFAULT_METAR_TTL)
            .with_clock(clock);

        cached.metars(bbox_query()).await.expect("fetch succeeds");
        cached.metars(stations_query()).await.expect("fetch succeeds");
        cached.metars(bbox_query()).await.expect("served cached");
        cached.metars(stations_query()).await.expect("served cached");
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn per_kind_ttls_expire_independently() {
        let (offset, clock) = test_clock();
        let cached = CachedWeatherProvider::with_ttls(
            FakeProvider::default(),
            Duration::from_secs(60),
            Duration::from_secs(600),
            Duration::from_secs(60),
        )
        .with_clock(clock);

        cached.metars(bbox_query()).await.expect("fetch succeeds");
        cached.tafs(bbox_query()).await.expect("fetch succeeds");
        *offset.lock() = Duration::from_secs(120);
        cached.metars(bbox_query()).await.expect("refetched");
        cached.tafs(bbox_query()).await.expect("still cached");
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 2);
        assert_eq!(cached.inner.taf_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidate_drops_all_entries() {
        let (_offset, clock) = test_clock();
        let cached = CachedWeatherProvider::with_default_ttls(FakeProvider::default())
            .with_clock(clock);

        cached.metars(bbox_query()).await.expect("fetch succeeds");
        cached.sigmets(de_bbox()).await.expect("fetch succeeds");
        cached.invalidate();
        cached.metars(bbox_query()).await.expect("refetched");
        cached.sigmets(de_bbox()).await.expect("refetched");
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 2);
        assert_eq!(cached.inner.sigmet_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_data_is_served_when_the_upstream_fails() {
        let (offset, clock) = test_clock();
        let cached = CachedWeatherProvider::new(FakeProvider::default(), DEFAULT_METAR_TTL)
            .with_clock(clock);

        let fresh = cached.metars(bbox_query()).await.expect("fetch succeeds");
        *offset.lock() = DEFAULT_METAR_TTL + Duration::from_secs(1);
        cached.inner.fail.store(true, Ordering::SeqCst);

        let stale = cached.metars(bbox_query()).await.expect("stale served");
        assert_eq!(fresh, stale);
        // The refetch was attempted before falling back.
        assert_eq!(cached.inner.metar_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn errors_surface_when_nothing_is_cached() {
        let (_offset, clock) = test_clock();
        let provider = FakeProvider::default();
        provider.fail.store(true, Ordering::SeqCst);
        let cached =
            CachedWeatherProvider::new(provider, DEFAULT_METAR_TTL).with_clock(clock);

        let result = cached.metars(bbox_query()).await;
        assert!(matches!(result, Err(Error::Provider { .. })));
    }

    #[tokio::test]
    async fn entry_count_per_kind_is_bounded() {
        let (_offset, clock) = test_clock();
        let cached = CachedWeatherProvider::with_default_ttls(FakeProvider::default())
            .with_clock(clock);

        for i in 0..(MAX_ENTRIES_PER_KIND + 4) {
            let query = WeatherQuery::Stations(vec![icao(&format!("ED{i:02}"))]);
            cached.metars(query).await.expect("fetch succeeds");
        }
        assert_eq!(cached.cache.lock().metars.len(), MAX_ENTRIES_PER_KIND);
    }
}

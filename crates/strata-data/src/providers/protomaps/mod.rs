//! Protomaps daily-build basemap extraction.
//!
//! Discovers the latest daily planet build, then pulls the tiles covering
//! a bbox out of it via HTTP range reads (`pmtiles` crate) into a local
//! MBTiles file: bounded-parallel fetches, a single SQLite writer thread,
//! retry with backoff, and resume support (already-present tiles are
//! skipped). Tile bytes are stored byte-identical to the source archive
//! (gzip-compressed MVT — see the `compression` metadata row).
//!
//! Build archives live under `https://build.protomaps.com/{key}`, but the
//! JSON index behind the listing page is on a separate host (verified
//! 2026-06-10): `https://build-metadata.protomaps.dev/builds.json`.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt as _;
use pmtiles::{
    AsyncBackend, AsyncPmTilesReader, Compression, DirectoryCache, HashMapCache, Header,
    HttpBackend, PmtError, TileCoord, TileType,
};
use tokio::sync::mpsc;

use crate::Error;
use crate::domain::BoundingBox;

mod builds;
pub mod mbtiles;
mod tiles;

pub use mbtiles::{Mbtiles, MbtilesError};

const PROVIDER: &str = "protomaps";
const FETCH_CONCURRENCY: usize = 16;
const MAX_FETCH_ATTEMPTS: u32 = 5;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const WRITE_QUEUE_DEPTH: usize = 64;
const WRITE_BATCH_MAX: usize = 128;

/// An XYZ-addressed slippy-map tile (y counts down from the north edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileXyz {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl fmt::Display for TileXyz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.z, self.x, self.y)
    }
}

pub struct ProtomapsExtractor {
    http: reqwest::Client,
    builds_url: String,
}

impl Default for ProtomapsExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress snapshot passed to the extraction callback (drives the ingest
/// CLI progress bar) and returned as the final summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractProgress {
    /// Tiles accounted for so far — fetched, found absent upstream, or
    /// skipped because a previous run already stored them.
    pub tiles_done: u64,
    /// `None` until the tile tree walk has established the total.
    pub tiles_total: Option<u64>,
    /// Tile bytes written during this run (excludes skipped tiles).
    pub bytes_written: u64,
}

impl ProtomapsExtractor {
    pub const DEFAULT_BUILDS_URL: &'static str = "https://build.protomaps.com";

    /// The JSON index behind the builds listing page. The file host
    /// ([`Self::DEFAULT_BUILDS_URL`]) serves only the archives themselves.
    pub const DEFAULT_BUILDS_INDEX_URL: &'static str =
        "https://build-metadata.protomaps.dev/builds.json";

    pub fn new() -> Self {
        Self::with_builds_url(Self::DEFAULT_BUILDS_URL)
    }

    /// Override the builds root (fixture/local-server tests). The index is
    /// then expected at `{builds_url}/builds.json` and archives at
    /// `{builds_url}/{key}`.
    pub fn with_builds_url(builds_url: impl Into<String>) -> Self {
        let builds_url = builds_url.into().trim_end_matches('/').to_string();
        Self { http: http_client(), builds_url }
    }

    /// URL of the most recent daily planet build (`….pmtiles`).
    pub async fn latest_build_url(&self) -> Result<String, Error> {
        let index_url = self.builds_index_url();
        let entries: Vec<builds::BuildEntry> = self
            .http
            .get(&index_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let latest = builds::latest(&entries).ok_or_else(|| {
            Error::provider(PROVIDER, format!("no .pmtiles builds listed at {index_url}"))
        })?;
        tracing::info!(key = %latest.key, uploaded = ?latest.uploaded, "latest protomaps build");
        Ok(format!("{}/{}", self.builds_url, latest.key))
    }

    fn builds_index_url(&self) -> String {
        if self.builds_url == Self::DEFAULT_BUILDS_URL {
            Self::DEFAULT_BUILDS_INDEX_URL.to_string()
        } else {
            format!("{}/builds.json", self.builds_url)
        }
    }

    /// Extracts all tiles intersecting `bbox` for z0..=`max_zoom` from the
    /// remote build at `build_url` into an MBTiles file at `dest` (created
    /// if missing). Tiles already present in `dest` are skipped, so an
    /// interrupted extraction resumes where it left off. Each tile fetch is
    /// retried with exponential backoff; the extraction fails once a tile
    /// exhausts its retries. Returns the final progress summary.
    pub async fn extract_to_mbtiles<F>(
        &self,
        build_url: &str,
        bbox: BoundingBox,
        max_zoom: u8,
        dest: &Path,
        progress: F,
    ) -> Result<ExtractProgress, Error>
    where
        F: FnMut(ExtractProgress) + Send,
    {
        let reader = AsyncPmTilesReader::<HttpBackend, _>::new_with_cached_url(
            HashMapCache::default(),
            self.http.clone(),
            build_url,
        )
        .await
        .map_err(|e| Error::provider(PROVIDER, e))?;
        extract_with_reader(&reader, build_url, bbox, max_zoom, dest, progress).await
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("strata/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        // Static configuration with rustls: building cannot fail.
        .expect("reqwest client construction")
}

struct TileWrite {
    tile: TileXyz,
    data: Vec<u8>,
}

enum Outcome {
    Completed,
    FetchFailed(Error),
    WriterGone,
}

/// Extraction core, generic over the archive backend so tests can run it
/// against a local file instead of HTTP.
async fn extract_with_reader<B, C, F>(
    reader: &AsyncPmTilesReader<B, C>,
    source: &str,
    bbox: BoundingBox,
    max_zoom: u8,
    dest: &Path,
    mut progress: F,
) -> Result<ExtractProgress, Error>
where
    B: AsyncBackend + Sync + Send,
    C: DirectoryCache + Sync + Send,
    F: FnMut(ExtractProgress) + Send,
{
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut mbtiles = Mbtiles::open(dest).map_err(|e| Error::provider(PROVIDER, e))?;
    let existing = mbtiles
        .existing_tiles()
        .map_err(|e| Error::provider(PROVIDER, e))?;

    let header = reader.get_header();
    let max_zoom = if max_zoom > header.max_zoom {
        tracing::warn!(
            requested = max_zoom,
            available = header.max_zoom,
            "requested max zoom exceeds the archive's; clamping"
        );
        header.max_zoom
    } else {
        max_zoom
    };

    let metadata_json = reader
        .get_metadata()
        .await
        .map_err(|e| Error::provider(PROVIDER, e))?;
    let entries = metadata_entries(&metadata_json, header, &bbox, max_zoom, source);
    let entry_refs: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (*k, v.as_str())).collect();
    mbtiles
        .set_metadata(&entry_refs)
        .map_err(|e| Error::provider(PROVIDER, e))?;

    let pyramid = tiles::pyramid(&bbox, max_zoom);
    let total: u64 = pyramid.iter().map(tiles::TileRect::count).sum();
    let mut worklist = Vec::new();
    for rect in pyramid {
        worklist.extend(rect.coords().filter(|coord| !existing.contains(coord)));
    }
    let skipped = total - worklist.len() as u64;
    tracing::info!(
        source,
        dest = %dest.display(),
        total,
        skipped,
        max_zoom,
        "extracting basemap tiles"
    );

    let mut report = ExtractProgress {
        tiles_done: skipped,
        tiles_total: Some(total),
        bytes_written: 0,
    };
    progress(report);
    if worklist.is_empty() {
        return Ok(report);
    }

    let (tx, rx) = mpsc::channel::<TileWrite>(WRITE_QUEUE_DEPTH);
    let writer = std::thread::spawn(move || write_loop(mbtiles, rx));

    let outcome = {
        let mut fetches = futures::stream::iter(worklist)
            .map(|coord| async move { (coord, fetch_with_retry(reader, coord).await) })
            .buffer_unordered(FETCH_CONCURRENCY);

        let mut outcome = Outcome::Completed;
        while let Some((coord, result)) = fetches.next().await {
            match result {
                Ok(Some(data)) => {
                    report.bytes_written += data.len() as u64;
                    if tx.send(TileWrite { tile: coord, data }).await.is_err() {
                        outcome = Outcome::WriterGone;
                        break;
                    }
                }
                Ok(None) => {
                    tracing::trace!(tile = %coord, "tile absent in remote archive");
                }
                Err(e) => {
                    outcome = Outcome::FetchFailed(e);
                    break;
                }
            }
            report.tiles_done += 1;
            progress(report);
        }
        outcome
    };

    drop(tx);
    let writer_result = match writer.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::provider(PROVIDER, e)),
        Err(_) => Err(Error::provider(PROVIDER, "mbtiles writer thread panicked")),
    };

    match (outcome, writer_result) {
        (Outcome::Completed, Ok(())) => {
            tracing::info!(
                tiles = report.tiles_done,
                bytes = report.bytes_written,
                "basemap extract complete"
            );
            Ok(report)
        }
        (Outcome::FetchFailed(e), _) => Err(e),
        (_, Err(e)) => Err(e),
        (Outcome::WriterGone, Ok(())) => Err(Error::provider(
            PROVIDER,
            "tile writer stopped before the extract finished",
        )),
    }
}

/// Drains the tile channel on a dedicated thread, batching inserts into
/// transactions (single writer; SQLite connections must not be shared).
fn write_loop(mut mbtiles: Mbtiles, mut rx: mpsc::Receiver<TileWrite>) -> Result<(), MbtilesError> {
    let mut batch: Vec<(TileXyz, Vec<u8>)> = Vec::with_capacity(WRITE_BATCH_MAX);
    while let Some(first) = rx.blocking_recv() {
        batch.push((first.tile, first.data));
        while batch.len() < WRITE_BATCH_MAX {
            match rx.try_recv() {
                Ok(write) => batch.push((write.tile, write.data)),
                Err(_) => break,
            }
        }
        mbtiles.put_tiles(batch.drain(..))?;
    }
    Ok(())
}

async fn fetch_with_retry<B, C>(
    reader: &AsyncPmTilesReader<B, C>,
    tile: TileXyz,
) -> Result<Option<Vec<u8>>, Error>
where
    B: AsyncBackend + Sync + Send,
    C: DirectoryCache + Sync + Send,
{
    let coord =
        TileCoord::new(tile.z, tile.x, tile.y).map_err(|e| Error::provider(PROVIDER, e))?;
    let mut attempt = 1u32;
    loop {
        match reader.get_tile(coord).await {
            Ok(data) => return Ok(data.map(|bytes| bytes.to_vec())),
            Err(e) if attempt < MAX_FETCH_ATTEMPTS && is_transient(&e) => {
                let backoff = INITIAL_BACKOFF * 2u32.saturating_pow(attempt - 1);
                tracing::warn!(
                    tile = %tile,
                    attempt,
                    backoff_ms = backoff.as_millis() as u64,
                    error = %e,
                    "transient tile fetch error; backing off"
                );
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err(e) => return Err(Error::provider(PROVIDER, e)),
        }
    }
}

fn is_transient(error: &PmtError) -> bool {
    match error {
        PmtError::Http(e) => match e.status() {
            // Permanent client errors — retrying cannot help.
            Some(status) if status.is_client_error() => matches!(status.as_u16(), 408 | 429),
            // 5xx, timeouts, connection and body errors.
            _ => true,
        },
        PmtError::Reading(_)
        | PmtError::UnexpectedNumberOfBytesReturned(..)
        | PmtError::ResponseBodyTooLong(..) => true,
        _ => false,
    }
}

fn metadata_entries(
    metadata_json: &str,
    header: &Header,
    bbox: &BoundingBox,
    max_zoom: u8,
    source: &str,
) -> Vec<(&'static str, String)> {
    let upstream: serde_json::Value =
        serde_json::from_str(metadata_json).unwrap_or(serde_json::Value::Null);
    let name = upstream
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Protomaps Basemap")
        .to_string();

    let mut attribution = upstream
        .get("attribution")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    for (marker, credit) in [
        ("OpenStreetMap", "© OpenStreetMap contributors"),
        ("Protomaps", "© Protomaps"),
    ] {
        if !attribution.contains(marker) {
            if !attribution.is_empty() {
                attribution.push(' ');
            }
            attribution.push_str(credit);
        }
    }

    let format = match header.tile_type {
        TileType::Mvt => "pbf",
        TileType::Png => "png",
        TileType::Jpeg => "jpg",
        TileType::Webp => "webp",
        TileType::Avif => "avif",
        TileType::Mlt | TileType::Unknown => {
            tracing::warn!(tile_type = ?header.tile_type, "unexpected tile type; recording format=pbf");
            "pbf"
        }
    };
    let compression = match header.tile_compression {
        Compression::Gzip => "gzip",
        Compression::Brotli => "br",
        Compression::Zstd => "zstd",
        Compression::None => "none",
        Compression::Unknown => "unknown",
    };
    let center = bbox.center();

    let mut entries = vec![
        ("name", name),
        ("format", format.to_string()),
        ("type", "baselayer".to_string()),
        ("description", format!("Bbox extract of {source}")),
        (
            "bounds",
            format!("{},{},{},{}", bbox.west(), bbox.south(), bbox.east(), bbox.north()),
        ),
        ("center", format!("{},{},{}", center.lon(), center.lat(), max_zoom.min(6))),
        ("minzoom", "0".to_string()),
        ("maxzoom", max_zoom.to_string()),
        ("attribution", attribution),
        // Layer schema verbatim from the source archive — the renderer
        // needs `vector_layers`; MBTiles requires this key for pbf.
        ("json", metadata_json.to_string()),
        // Tile blobs are byte-identical to the source archive; this row
        // records how they are compressed.
        ("compression", compression.to_string()),
    ];
    if let Some(version) = upstream.get("version").and_then(serde_json::Value::as_str) {
        entries.push(("version", version.to_string()));
    }
    entries
}

#[cfg(test)]
mod tests {
    use pmtiles::{MmapBackend, PmTilesWriter, TileId};

    use super::*;

    const FIXTURE_METADATA: &str = r#"{"name":"fixture basemap","attribution":"Protomaps © OpenStreetMap contributors","version":"4.0.0","vector_layers":[{"id":"water","fields":{}}]}"#;

    #[test]
    fn builds_index_url_for_default_and_custom_roots() {
        let default = ProtomapsExtractor::new();
        assert_eq!(
            default.builds_index_url(),
            ProtomapsExtractor::DEFAULT_BUILDS_INDEX_URL
        );

        let custom = ProtomapsExtractor::with_builds_url("http://localhost:9999/");
        assert_eq!(custom.builds_index_url(), "http://localhost:9999/builds.json");
        assert_eq!(custom.builds_url, "http://localhost:9999");
    }

    #[tokio::test]
    async fn metadata_entries_fill_required_rows() {
        // Upstream metadata without attribution/name must still produce
        // OpenStreetMap + Protomaps credits and a pbf format row.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hdr.pmtiles");
        {
            let file = std::fs::File::create(&path).expect("create");
            let mut writer = PmTilesWriter::new(TileType::Mvt)
                .tile_compression(Compression::None)
                .metadata("{}")
                .create(file)
                .expect("writer");
            writer
                .add_tile(TileCoord::new(0, 0, 0).expect("coord"), b"x")
                .expect("add");
            writer.finalize().expect("finalize");
        }
        let backend = MmapBackend::try_from(path.to_str().expect("utf8 path"))
            .await
            .expect("mmap backend");
        let reader = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("reader");

        let bbox = BoundingBox::new(5.5, 47.0, 15.5, 55.2).expect("bbox");
        let entries = metadata_entries("{}", reader.get_header(), &bbox, 13, "test://fixture");
        let get = |k: &str| {
            entries
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.clone())
        };
        let attribution = get("attribution").expect("attribution");
        assert!(attribution.contains("OpenStreetMap"));
        assert!(attribution.contains("Protomaps"));
        assert_eq!(get("bounds").as_deref(), Some("5.5,47,15.5,55.2"));
        assert_eq!(get("maxzoom").as_deref(), Some("13"));
        assert_eq!(get("minzoom").as_deref(), Some("0"));
        assert_eq!(get("json").as_deref(), Some("{}"));
        assert_eq!(get("format").as_deref(), Some("pbf"));
        assert_eq!(get("compression").as_deref(), Some("none"));
    }

    fn write_fixture_archive(path: &Path) {
        let file = std::fs::File::create(path).expect("create pmtiles");
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .tile_compression(Compression::None)
            .max_zoom(2)
            .bounds(-180.0, -85.0, 180.0, 85.0)
            .metadata(FIXTURE_METADATA)
            .create(file)
            .expect("create writer");
        let mut coords = Vec::new();
        for z in 0u8..=2 {
            for x in 0..(1u32 << z) {
                for y in 0..(1u32 << z) {
                    coords.push(TileCoord::new(z, x, y).expect("valid coord"));
                }
            }
        }
        // Clustered archives want ascending tile ids.
        coords.sort_by_key(|c| TileId::from(*c));
        for coord in coords {
            let data = format!("tile {}/{}/{}", coord.z(), coord.x(), coord.y());
            writer.add_tile(coord, data.as_bytes()).expect("add tile");
        }
        writer.finalize().expect("finalize");
    }

    #[tokio::test]
    async fn extracts_fixture_archive_skipping_existing_tiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("fixture.pmtiles");
        write_fixture_archive(&archive);

        let backend = MmapBackend::try_from(archive.to_str().expect("utf8 path"))
            .await
            .expect("mmap backend");
        let reader = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("reader");

        let dest = dir.path().join("out.mbtiles");
        // Pre-populate one tile: it must be skipped, not refetched.
        {
            let mut mb = Mbtiles::open(&dest).expect("open dest");
            mb.put_tile(TileXyz { z: 0, x: 0, y: 0 }, b"preexisting")
                .expect("seed tile");
        }

        let bbox = BoundingBox::new(-179.9, -80.0, 179.9, 80.0).expect("bbox");
        let mut reports = Vec::new();
        let summary =
            extract_with_reader(&reader, "test://fixture", bbox, 2, &dest, |p| reports.push(p))
                .await
                .expect("extract");

        // 1 + 4 + 16 tiles; the seeded one counted as done from the start.
        assert_eq!(summary.tiles_total, Some(21));
        assert_eq!(summary.tiles_done, 21);
        assert!(summary.bytes_written > 0);
        assert_eq!(reports.first().map(|r| r.tiles_done), Some(1));
        assert_eq!(reports.last().copied(), Some(summary));

        let mb = Mbtiles::open(&dest).expect("reopen");
        assert_eq!(mb.existing_tiles().expect("existing").len(), 21);
        // Resume semantics: the seeded tile was not overwritten.
        assert_eq!(
            mb.tile(TileXyz { z: 0, x: 0, y: 0 }).expect("tile"),
            Some(b"preexisting".to_vec())
        );
        // Fetched tiles round-trip byte-identically (incl. y-flip).
        assert_eq!(
            mb.tile(TileXyz { z: 2, x: 3, y: 1 }).expect("tile"),
            Some(b"tile 2/3/1".to_vec())
        );
        assert_eq!(mb.metadata("format").expect("meta"), Some("pbf".into()));
        assert_eq!(mb.metadata("compression").expect("meta"), Some("none".into()));
        assert_eq!(mb.metadata("minzoom").expect("meta"), Some("0".into()));
        assert_eq!(mb.metadata("maxzoom").expect("meta"), Some("2".into()));
        assert_eq!(
            mb.metadata("bounds").expect("meta"),
            Some("-179.9,-80,179.9,80".into())
        );
        let json = mb.metadata("json").expect("meta").expect("json row");
        assert!(json.contains("vector_layers"));
        let attribution = mb
            .metadata("attribution")
            .expect("meta")
            .expect("attribution row");
        assert!(attribution.contains("OpenStreetMap"));
        assert!(attribution.contains("Protomaps"));

        // A second run finds everything present and writes nothing new.
        let rerun = extract_with_reader(&reader, "test://fixture", bbox, 2, &dest, |_| {})
            .await
            .expect("rerun");
        assert_eq!(rerun.tiles_done, 21);
        assert_eq!(rerun.bytes_written, 0);
    }

    #[tokio::test]
    async fn max_zoom_is_clamped_to_archive_zoom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("fixture.pmtiles");
        write_fixture_archive(&archive);
        let backend = MmapBackend::try_from(archive.to_str().expect("utf8 path"))
            .await
            .expect("mmap backend");
        let reader = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("reader");

        let dest = dir.path().join("clamped.mbtiles");
        let bbox = BoundingBox::new(-179.9, -80.0, 179.9, 80.0).expect("bbox");
        let summary = extract_with_reader(&reader, "test://fixture", bbox, 10, &dest, |_| {})
            .await
            .expect("extract");
        assert_eq!(summary.tiles_total, Some(21));
        let mb = Mbtiles::open(&dest).expect("open");
        assert_eq!(mb.metadata("maxzoom").expect("meta"), Some("2".into()));
    }
}

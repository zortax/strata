//! [`TileSource`] adapters bridging on-disk data to the renderer.
//!
//! `TileSource::tile` is called from the renderer's worker threads and may
//! block; both adapters do synchronous SQLite reads. The MBTiles adapter
//! holds a small fixed pool of read-only connections so concurrent workers
//! don't serialize into one lane.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::OpenFlags;
use strata_data::store::Store;
use strata_render::{TileId, TileSource};

/// Read-only connections per MBTiles archive — sized for the render worker
/// pool (2–8 threads) without keeping an excess of open file handles.
const MBTILES_POOL_SIZE: usize = 4;

/// Vector basemap tiles from an MBTiles archive (`basemap.mbtiles`).
///
/// MBTiles stores rows in TMS order; [`TileId`] is slippy XYZ, so the row is
/// flipped (`tms_y = 2^z - 1 - y`). Tile data is returned as stored — the
/// renderer's decoder sniffs gzip/zlib itself.
pub struct MbTilesSource {
    conns: Vec<Mutex<rusqlite::Connection>>,
    /// Round-robin start slot for the next lookup.
    next: AtomicUsize,
    /// `metadata.maxzoom`, when the archive declares one. Handed to the
    /// renderer via [`TileSource::max_zoom`] so tile selection clamps to the
    /// data instead of a hardcoded default.
    max_zoom: Option<u8>,
}

impl MbTilesSource {
    /// Opens the archive read-only. Errors (most commonly: file does not
    /// exist because `strata-ingest basemap` never ran) are the caller's to
    /// log; pass `None` to the renderer in that case.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let mut conns = Vec::with_capacity(MBTILES_POOL_SIZE);
        let mut max_zoom = None;
        for slot in 0..MBTILES_POOL_SIZE {
            let conn = rusqlite::Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            // Wait out brief locks from a concurrent `strata-ingest basemap`
            // (WAL checkpoint/restart) instead of surfacing SQLITE_BUSY as a
            // spurious "missing tile".
            conn.busy_timeout(Duration::from_millis(500))?;
            if slot == 0 {
                // Fail at startup, not on the first worker-thread read, when
                // the file exists but is not an MBTiles archive.
                conn.prepare(
                    "SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles LIMIT 0",
                )?;
                max_zoom = read_metadata_max_zoom(&conn);
                tracing::info!(
                    path = %path.display(),
                    ?max_zoom,
                    "opened mbtiles archive"
                );
            }
            conns.push(Mutex::new(conn));
        }
        Ok(Self {
            conns,
            next: AtomicUsize::new(0),
            max_zoom,
        })
    }

    /// An available connection: try every slot from a rotating start, fall
    /// back to blocking on the start slot when all are busy.
    fn lock_conn(&self) -> MutexGuard<'_, rusqlite::Connection> {
        let start = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        for offset in 0..self.conns.len() {
            let slot = (start + offset) % self.conns.len();
            if let Some(guard) = self.conns[slot].try_lock() {
                return guard;
            }
        }
        self.conns[start].lock()
    }
}

impl TileSource for MbTilesSource {
    fn tile(&self, id: TileId) -> Option<Vec<u8>> {
        let tms_y = (1u32 << id.z).checked_sub(1 + id.y)?;
        let conn = self.lock_conn();
        // prepare_cached: one prepared statement per pooled connection
        // instead of re-parsing the SQL on every tile.
        let result = conn
            .prepare_cached(
                "SELECT tile_data FROM tiles
                 WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
            )
            .and_then(|mut stmt| {
                stmt.query_row((id.z as i64, id.x as i64, tms_y as i64), |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .map(Some)
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
            });
        match result {
            Ok(data) => data,
            Err(err) => {
                tracing::warn!(z = id.z, x = id.x, y = id.y, %err, "mbtiles read failed");
                None
            }
        }
    }

    fn max_zoom(&self) -> Option<u8> {
        self.max_zoom
    }
}

/// The archive's declared `metadata.maxzoom`, tolerant of how tools store
/// it (TEXT per spec, but INTEGER/REAL exist in the wild). A missing table,
/// missing row or unparsable value is `None` — the renderer then falls back
/// to its configured default.
fn read_metadata_max_zoom(conn: &rusqlite::Connection) -> Option<u8> {
    use rusqlite::types::Value;
    let value = conn
        .query_row(
            "SELECT value FROM metadata WHERE name = 'maxzoom'",
            (),
            |row| row.get::<_, Value>(0),
        )
        .ok()?;
    match value {
        // TEXT affinity also captures REAL-ish writes ("12.0").
        Value::Text(text) => {
            let text = text.trim();
            text.parse()
                .ok()
                .or_else(|| text.parse::<f64>().ok().and_then(f64_zoom))
        }
        Value::Integer(int) => u8::try_from(int).ok(),
        Value::Real(real) => f64_zoom(real),
        _ => None,
    }
}

fn f64_zoom(value: f64) -> Option<u8> {
    (value.is_finite() && (0.0..=255.0).contains(&value) && value.fract() == 0.0)
        .then_some(value as u8)
}

/// Terrain hillshade PNG tiles from the SQLite store (XYZ addressing,
/// written by `strata-ingest terrain`).
pub struct StoreTerrainSource {
    store: Arc<Store>,
}

impl StoreTerrainSource {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl TileSource for StoreTerrainSource {
    fn tile(&self, id: TileId) -> Option<Vec<u8>> {
        match self.store.terrain_tile(id.z, id.x, id.y) {
            Ok(tile) => tile,
            Err(err) => {
                tracing::warn!(z = id.z, x = id.x, y = id.y, %err, "terrain tile read failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_mbtiles(path: &Path) {
        let conn = rusqlite::Connection::open(path).expect("create db");
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT, value TEXT);
             CREATE TABLE tiles (
                 zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB
             );",
        )
        .expect("schema");
        // Tile z=2 x=1 with XYZ y=1 -> TMS row = 2^2 - 1 - 1 = 2.
        conn.execute("INSERT INTO tiles VALUES (2, 1, 2, x'C0FFEE')", ())
            .expect("insert");
    }

    #[test]
    fn mbtiles_reads_with_y_flip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("basemap.mbtiles");
        fixture_mbtiles(&path);

        let source = MbTilesSource::open(&path).expect("open");
        let id = TileId::new(2, 1, 1).expect("valid tile id");
        assert_eq!(source.tile(id), Some(vec![0xC0, 0xFF, 0xEE]));

        // The same coordinates *without* the flip must not resolve.
        let unflipped = TileId::new(2, 1, 2).expect("valid tile id");
        assert_eq!(source.tile(unflipped), None);
    }

    #[test]
    fn mbtiles_without_maxzoom_metadata_reports_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("basemap.mbtiles");
        fixture_mbtiles(&path);
        let source = MbTilesSource::open(&path).expect("open");
        assert_eq!(source.max_zoom(), None);
    }

    #[test]
    fn mbtiles_maxzoom_metadata_is_read_in_text_and_integer_forms() {
        for (value_sql, expected) in [
            ("'13'", Some(13u8)),   // spec form: TEXT
            ("' 11 '", Some(11)),   // sloppy whitespace
            ("12", Some(12)),       // INTEGER-typed in the wild
            ("12.0", Some(12)),     // REAL-typed in the wild
            ("'not-a-zoom'", None), // garbage degrades to the fallback
            ("999", None),          // out of u8 range
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("basemap.mbtiles");
            fixture_mbtiles(&path);
            {
                let conn = rusqlite::Connection::open(&path).expect("open rw");
                conn.execute(
                    &format!("INSERT INTO metadata VALUES ('maxzoom', {value_sql})"),
                    (),
                )
                .expect("insert metadata");
            }
            let source = MbTilesSource::open(&path).expect("open");
            assert_eq!(source.max_zoom(), expected, "metadata value {value_sql}");
        }
    }

    #[test]
    fn mbtiles_missing_file_is_an_open_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(MbTilesSource::open(&dir.path().join("nope.mbtiles")).is_err());
    }

    #[test]
    fn mbtiles_wrong_schema_is_an_open_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-mbtiles.sqlite");
        let conn = rusqlite::Connection::open(&path).expect("create db");
        conn.execute_batch("CREATE TABLE foo (bar TEXT);")
            .expect("schema");
        drop(conn);
        assert!(MbTilesSource::open(&path).is_err());
    }
}

//! Minimal MBTiles 1.3 container (SQLite) used for the local basemap
//! extract. The public API is addressed with XYZ tile coordinates; the
//! TMS row flip the MBTiles spec mandates happens inside this module.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use super::TileXyz;
use super::tiles::MAX_TILE_ZOOM;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MbtilesError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("tile coordinates out of range: {0}")]
    OutOfRange(TileXyz),
    #[error("invalid tile address stored in tiles table: z={z} column={column} row={row}")]
    InvalidStoredTile { z: i64, column: i64, row: i64 },
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS metadata (
    name  TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tiles (
    zoom_level  INTEGER NOT NULL,
    tile_column INTEGER NOT NULL,
    tile_row    INTEGER NOT NULL,
    tile_data   BLOB NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS tile_index
    ON tiles (zoom_level, tile_column, tile_row);
";

const INSERT_TILE: &str = "INSERT OR REPLACE INTO tiles \
    (zoom_level, tile_column, tile_row, tile_data) VALUES (?1, ?2, ?3, ?4)";

/// An MBTiles archive on disk. Opening creates the file and schema when
/// missing; existing tiles are preserved (resume support).
pub struct Mbtiles {
    conn: Connection,
}

impl Mbtiles {
    pub fn open(path: &Path) -> Result<Self, MbtilesError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Upserts metadata rows (`name` is the primary key).
    pub fn set_metadata(&mut self, entries: &[(&str, &str)]) -> Result<(), MbtilesError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO metadata (name, value) VALUES (?1, ?2) \
                 ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            )?;
            for (name, value) in entries {
                stmt.execute(params![name, value])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn metadata(&self, name: &str) -> Result<Option<String>, MbtilesError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM metadata WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn put_tile(&mut self, tile: TileXyz, data: &[u8]) -> Result<(), MbtilesError> {
        let row = tms_row(tile)?;
        self.conn
            .execute(INSERT_TILE, params![tile.z, tile.x, row, data])?;
        Ok(())
    }

    /// Inserts a batch of tiles in one transaction; returns how many.
    pub fn put_tiles<I>(&mut self, batch: I) -> Result<usize, MbtilesError>
    where
        I: IntoIterator<Item = (TileXyz, Vec<u8>)>,
    {
        let tx = self.conn.transaction()?;
        let mut written = 0usize;
        {
            let mut stmt = tx.prepare_cached(INSERT_TILE)?;
            for (tile, data) in batch {
                let row = tms_row(tile)?;
                stmt.execute(params![tile.z, tile.x, row, data])?;
                written += 1;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// Tile bytes at an XYZ address, exactly as stored (typically
    /// gzip-compressed MVT — see the `compression` metadata row).
    pub fn tile(&self, tile: TileXyz) -> Result<Option<Vec<u8>>, MbtilesError> {
        let row = tms_row(tile)?;
        Ok(self
            .conn
            .query_row(
                "SELECT tile_data FROM tiles \
                 WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
                params![tile.z, tile.x, row],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// All tile addresses already present, in XYZ coordinates (one upfront
    /// scan; used to skip finished tiles when resuming an extract).
    pub fn existing_tiles(&self) -> Result<HashSet<TileXyz>, MbtilesError> {
        let mut stmt = self
            .conn
            .prepare("SELECT zoom_level, tile_column, tile_row FROM tiles")?;
        let mut tiles = HashSet::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let (z, column, tms): (i64, i64, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
            let tile = tile_from_tms(z, column, tms).ok_or(MbtilesError::InvalidStoredTile {
                z,
                column,
                row: tms,
            })?;
            tiles.insert(tile);
        }
        Ok(tiles)
    }
}

/// XYZ→TMS flip: MBTiles rows count up from the south edge,
/// `row = 2^z - 1 - y`.
fn tms_row(tile: TileXyz) -> Result<u32, MbtilesError> {
    if tile.z > MAX_TILE_ZOOM {
        return Err(MbtilesError::OutOfRange(tile));
    }
    let n = 1u64 << tile.z;
    if u64::from(tile.x) >= n || u64::from(tile.y) >= n {
        return Err(MbtilesError::OutOfRange(tile));
    }
    Ok((n - 1 - u64::from(tile.y)) as u32)
}

/// TMS→XYZ: inverse of [`tms_row`], `None` for rows that cannot have been
/// written through this module.
fn tile_from_tms(z: i64, column: i64, row: i64) -> Option<TileXyz> {
    let z = u8::try_from(z).ok().filter(|z| *z <= MAX_TILE_ZOOM)?;
    let x = u32::try_from(column).ok()?;
    let tms = u64::try_from(row).ok()?;
    let n = 1u64 << z;
    if u64::from(x) >= n || tms >= n {
        return None;
    }
    Some(TileXyz {
        z,
        x,
        y: (n - 1 - tms) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_mbtiles() -> (tempfile::TempDir, Mbtiles) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mb = Mbtiles::open(&dir.path().join("test.mbtiles")).expect("open");
        (dir, mb)
    }

    #[test]
    fn tile_roundtrip_with_y_flip() {
        let (_dir, mut mb) = temp_mbtiles();
        let tile = TileXyz { z: 2, x: 1, y: 1 };
        mb.put_tile(tile, b"payload").expect("put");

        // Read back through the XYZ helper…
        assert_eq!(mb.tile(tile).expect("get"), Some(b"payload".to_vec()));

        // …and via the raw TMS row convention: row = 2^2 - 1 - 1 = 2.
        let raw: Vec<u8> = mb
            .conn
            .query_row(
                "SELECT tile_data FROM tiles \
                 WHERE zoom_level = 2 AND tile_column = 1 AND tile_row = 2",
                [],
                |r| r.get(0),
            )
            .expect("raw row at flipped y");
        assert_eq!(raw, b"payload");

        // The unflipped row must not exist.
        assert_eq!(mb.tile(TileXyz { z: 2, x: 1, y: 2 }).expect("get"), None);
    }

    #[test]
    fn put_tile_replaces_existing() {
        let (_dir, mut mb) = temp_mbtiles();
        let tile = TileXyz { z: 0, x: 0, y: 0 };
        mb.put_tile(tile, b"old").expect("put");
        mb.put_tile(tile, b"new").expect("put");
        assert_eq!(mb.tile(tile).expect("get"), Some(b"new".to_vec()));
    }

    #[test]
    fn batch_insert_and_existing_tiles_survive_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("resume.mbtiles");
        let a = TileXyz { z: 1, x: 0, y: 1 };
        let b = TileXyz { z: 3, x: 5, y: 2 };
        {
            let mut mb = Mbtiles::open(&path).expect("open");
            let written = mb
                .put_tiles(vec![(a, b"a".to_vec()), (b, b"b".to_vec())])
                .expect("batch");
            assert_eq!(written, 2);
        }
        let mb = Mbtiles::open(&path).expect("reopen");
        let existing = mb.existing_tiles().expect("existing");
        assert_eq!(existing, HashSet::from([a, b]));
        assert_eq!(mb.tile(b).expect("get"), Some(b"b".to_vec()));
    }

    #[test]
    fn metadata_upserts() {
        let (_dir, mut mb) = temp_mbtiles();
        mb.set_metadata(&[("name", "first"), ("format", "pbf")])
            .expect("set");
        mb.set_metadata(&[("name", "second")]).expect("set again");
        assert_eq!(mb.metadata("name").expect("get"), Some("second".into()));
        assert_eq!(mb.metadata("format").expect("get"), Some("pbf".into()));
        assert_eq!(mb.metadata("missing").expect("get"), None);
    }

    #[test]
    fn out_of_range_coordinates_are_rejected() {
        let (_dir, mut mb) = temp_mbtiles();
        let bad = TileXyz { z: 2, x: 4, y: 0 };
        assert!(matches!(
            mb.put_tile(bad, b"x"),
            Err(MbtilesError::OutOfRange(t)) if t == bad
        ));
    }
}

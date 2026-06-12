//! Connection setup and `user_version`-based schema migrations.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::StoreError;

/// Current schema version (`PRAGMA user_version`). Bump together with a new
/// migration arm in [`migrate`].
const SCHEMA_VERSION: i64 = 3;

/// All feature tables share this shape; `ident` and `name` are denormalized
/// search columns (uppercased at insert time so SQL comparisons stay
/// case-insensitive beyond ASCII), `data` is the postcard blob of the domain
/// struct. Each table is paired with an `{table}_rtree` R*Tree keyed by the
/// base-table rowid carrying the feature's bounding box.
const FEATURE_TABLES: [&str; 5] = [
    "airspaces",
    "airports",
    "navaids",
    "reporting_points",
    "obstacles",
];

/// Opens the database at `path` (creating parent directories and the file as
/// needed), applies pragmas, and migrates the schema.
pub(super) fn open(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    // WAL lets concurrent Store instances (one per reader thread) coexist
    // with the single ingest writer.
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if !(0..=SCHEMA_VERSION).contains(&version) {
        return Err(StoreError::Schema(format!(
            "schema version {version} not supported (expected 0..={SCHEMA_VERSION})"
        )));
    }
    if version < SCHEMA_VERSION {
        tracing::info!(
            from = version,
            to = SCHEMA_VERSION,
            "creating/migrating store schema"
        );
    }
    if version == 0 {
        apply_v1(conn)?;
        version = 1;
    }
    if version == 1 {
        apply_v2(conn)?;
        version = 2;
    }
    if version == 2 {
        apply_v3(conn)?;
    }
    Ok(())
}

pub(super) fn apply_v1(conn: &Connection) -> Result<(), StoreError> {
    let mut ddl = String::from("BEGIN;\n");
    for table in FEATURE_TABLES {
        ddl.push_str(&format!(
            "CREATE TABLE {table} (
                 id    INTEGER PRIMARY KEY,
                 ident TEXT,
                 name  TEXT,
                 data  BLOB NOT NULL
             );
             CREATE INDEX {table}_ident_idx ON {table}(ident);
             CREATE VIRTUAL TABLE {table}_rtree
                 USING rtree(id, min_lon, max_lon, min_lat, max_lat);\n"
        ));
    }
    ddl.push_str(
        "CREATE TABLE terrain_tiles (
             z    INTEGER NOT NULL,
             x    INTEGER NOT NULL,
             y    INTEGER NOT NULL,
             data BLOB NOT NULL,
             PRIMARY KEY (z, x, y)
         );
         CREATE TABLE meta (
             dataset         TEXT PRIMARY KEY,
             source          TEXT NOT NULL,
             airac_id        TEXT,
             airac_effective TEXT,
             ingested_at     TEXT NOT NULL
         );
         PRAGMA user_version = 1;
         COMMIT;",
    );
    conn.execute_batch(&ddl)?;
    Ok(())
}

/// v2: the max-pooled elevation grid. `data` is a zlib-compressed 256×256
/// little-endian-i16 cell raster — see the `elevation` module docs for the
/// full format.
pub(super) fn apply_v2(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE elevation_tiles (
             tx   INTEGER NOT NULL,
             ty   INTEGER NOT NULL,
             data BLOB NOT NULL,
             PRIMARY KEY (tx, ty)
         );
         PRAGMA user_version = 2;
         COMMIT;",
    )?;
    Ok(())
}

/// v3: the country dimension.
///
/// - Every feature row carries the ISO alpha-2 `country` it was ingested
///   for; replace-all becomes per-(dataset, country). Pre-v3 rows are
///   backfilled as `DE` — the only country v2 could hold.
/// - `meta` is rekeyed to `(dataset, country)`; existing rows migrate to
///   `DE` losslessly. SQLite cannot alter a primary key, so the table is
///   rebuilt and the rows copied.
/// - Tile tables (`terrain_tiles`, `elevation_tiles`, the basemap MBTiles)
///   stay global: tiles merge across countries; per-country coverage is
///   tracked via the per-country meta rows + bbox tile counts.
fn apply_v3(conn: &Connection) -> Result<(), StoreError> {
    let mut ddl = String::from("BEGIN;\n");
    for table in FEATURE_TABLES {
        ddl.push_str(&format!(
            "ALTER TABLE {table} ADD COLUMN country TEXT NOT NULL DEFAULT 'DE';
             CREATE INDEX {table}_country_idx ON {table}(country);\n"
        ));
    }
    ddl.push_str(
        "CREATE TABLE meta_v3 (
             dataset         TEXT NOT NULL,
             country         TEXT NOT NULL,
             source          TEXT NOT NULL,
             airac_id        TEXT,
             airac_effective TEXT,
             ingested_at     TEXT NOT NULL,
             PRIMARY KEY (dataset, country)
         );
         INSERT INTO meta_v3 (dataset, country, source, airac_id, airac_effective, ingested_at)
             SELECT dataset, 'DE', source, airac_id, airac_effective, ingested_at FROM meta;
         DROP TABLE meta;
         ALTER TABLE meta_v3 RENAME TO meta;
         PRAGMA user_version = 3;
         COMMIT;",
    );
    conn.execute_batch(&ddl)?;
    Ok(())
}

//! Hillshade raster tile storage (slippy XYZ addressing, y grows south).

use rusqlite::{Connection, OptionalExtension, params};

use super::StoreError;

pub(super) fn get(
    conn: &Connection,
    z: u8,
    x: u32,
    y: u32,
) -> Result<Option<Vec<u8>>, StoreError> {
    // Hot path: the renderer fetches every visible tile through here, so the
    // statement is cached instead of re-prepared per call.
    let mut stmt =
        conn.prepare_cached("SELECT data FROM terrain_tiles WHERE z = ?1 AND x = ?2 AND y = ?3")?;
    let png = stmt
        .query_row(params![i64::from(z), i64::from(x), i64::from(y)], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(png)
}

pub(super) fn put(
    conn: &Connection,
    z: u8,
    x: u32,
    y: u32,
    png: &[u8],
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO terrain_tiles (z, x, y, data) VALUES (?1, ?2, ?3, ?4)",
        params![i64::from(z), i64::from(x), i64::from(y), png],
    )?;
    Ok(())
}

//! Store-resident **max-pooled elevation grid** — the profile/corridor
//! terrain source. Written by the terrain ingest stage from the Copernicus
//! GLO-30 DEM, read by planning samplers.
//!
//! # Tile format (schema v2, table `elevation_tiles(tx, ty, data)`)
//!
//! - **Grid:** global 6 arc-second (1/600°) cells anchored at (90° S,
//!   180° W); cell `(x, y)` covers `[-180 + x/600, -180 + (x+1)/600)`
//!   longitude × `[-90 + y/600, -90 + (y+1)/600)` latitude (see [`grid`]).
//! - **Tile:** `(tx, ty)` groups the 256×256 cells
//!   `x ∈ [tx·256, (tx+1)·256)`, `y ∈ [ty·256, (ty+1)·256)`.
//! - **`data` BLOB:** zlib-compressed (flate2); decompressed exactly
//!   `256·256·2` bytes — row-major from the tile's **south-west** cell
//!   (columns west→east within a row, rows south→north), each cell one
//!   **little-endian `i16`**: the *maximum* GLO-30 DEM sample inside the
//!   cell, rounded **up** (ceil) to whole meters AMSL. [`ELEVATION_NO_DATA`]
//!   (`i16::MIN`) marks cells without any DEM data (open sea on unpublished
//!   squares, DEM voids, or never ingested).
//!
//! Max-pooling preserves the safety semantics: aggregation can only raise
//! terrain, never hide a ridge — a reported cell value is never below any
//! DEM sample inside the cell (property-tested in [`pool`]).
//!
//! Compression is zlib via `flate2` — already a strata-data dependency
//! (terrain pipeline), pure Rust path, and ~3–5× on these blobs; zstd would
//! add a new dependency for marginal gain at 128 KiB tile size.

mod adapter;
mod grid;
mod pool;

pub use adapter::ElevationTileSet;
pub use grid::{ELEVATION_CELLS_PER_DEGREE, ELEVATION_TILE_SIDE, ElevationTileId};
pub use pool::ElevationPooler;

use std::io::{Read as _, Write as _};

use rusqlite::{Connection, OptionalExtension, params};

use super::StoreError;
use crate::domain::{BoundingBox, MetersAmsl};
use grid::{Cell, cell_of};

/// Sentinel cell value: no DEM data in this cell.
pub const ELEVATION_NO_DATA: i16 = i16::MIN;

/// Cells per tile (256 × 256).
const CELLS_PER_TILE: usize = ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE;

/// One decoded 256×256-cell elevation tile (see the module docs for the
/// exact cell layout and value semantics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevationTile {
    id: ElevationTileId,
    cells: Vec<i16>,
}

impl ElevationTile {
    /// Builds a tile from `256·256` cell values, row-major from the
    /// south-west cell. Fails when the count is off.
    pub fn new(id: ElevationTileId, cells: Vec<i16>) -> Result<Self, StoreError> {
        if cells.len() != CELLS_PER_TILE {
            return Err(StoreError::Schema(format!(
                "elevation tile needs {CELLS_PER_TILE} cells, got {}",
                cells.len()
            )));
        }
        Ok(Self { id, cells })
    }

    pub fn id(&self) -> ElevationTileId {
        self.id
    }

    /// Raw cell values, row-major from the south-west cell (columns
    /// west→east, rows south→north). [`ELEVATION_NO_DATA`] marks no-data.
    pub fn cells(&self) -> &[i16] {
        &self.cells
    }

    /// Max-pooled elevation of the cell containing `(lat, lon)`; `None`
    /// when the position is outside this tile or the cell has no data.
    pub fn max_at(&self, lat: f64, lon: f64) -> Option<MetersAmsl> {
        self.value_at(cell_of(lat, lon)).map(meters)
    }

    /// Cell-wise maximum with `other` (same tile id). The no-data sentinel
    /// is `i16::MIN`, so it never wins against real data — merging a fresh
    /// partial-coverage tile with a stored one preserves both.
    pub fn merge_max(&mut self, other: &Self) {
        debug_assert_eq!(self.id, other.id, "merging tiles of different ids");
        for (mine, theirs) in self.cells.iter_mut().zip(&other.cells) {
            *mine = (*mine).max(*theirs);
        }
    }

    /// Value of a global cell, `None` outside the tile or no-data.
    pub(crate) fn value_at(&self, cell: Cell) -> Option<i16> {
        let value = self.cells[self.id.index_of(cell)?];
        (value != ELEVATION_NO_DATA).then_some(value)
    }
}

fn meters(value: i16) -> MetersAmsl {
    MetersAmsl(f64::from(value))
}

// --- blob codec -------------------------------------------------------------

fn encode(cells: &[i16]) -> Result<Vec<u8>, StoreError> {
    let mut raw = Vec::with_capacity(cells.len() * 2);
    for cell in cells {
        raw.extend_from_slice(&cell.to_le_bytes());
    }
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw)?;
    Ok(encoder.finish()?)
}

fn decode(id: ElevationTileId, blob: &[u8]) -> Result<ElevationTile, StoreError> {
    let mut raw = Vec::with_capacity(CELLS_PER_TILE * 2);
    flate2::read::ZlibDecoder::new(blob)
        .read_to_end(&mut raw)
        .map_err(|e| {
            StoreError::Schema(format!("elevation tile ({}, {}): {e}", id.tx, id.ty))
        })?;
    if raw.len() != CELLS_PER_TILE * 2 {
        return Err(StoreError::Schema(format!(
            "elevation tile ({}, {}) decompresses to {} bytes, expected {}",
            id.tx,
            id.ty,
            raw.len(),
            CELLS_PER_TILE * 2
        )));
    }
    let cells = raw
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(ElevationTile { id, cells })
}

// --- SQL --------------------------------------------------------------------

pub(super) fn get(
    conn: &Connection,
    id: ElevationTileId,
) -> Result<Option<ElevationTile>, StoreError> {
    // Point samplers come through here per query — cache the statement.
    let mut stmt =
        conn.prepare_cached("SELECT data FROM elevation_tiles WHERE tx = ?1 AND ty = ?2")?;
    let blob: Option<Vec<u8>> = stmt
        .query_row(params![i64::from(id.tx), i64::from(id.ty)], |row| row.get(0))
        .optional()?;
    blob.map(|blob| decode(id, &blob)).transpose()
}

pub(super) fn put(conn: &Connection, tile: &ElevationTile) -> Result<(), StoreError> {
    let blob = encode(&tile.cells)?;
    conn.execute(
        "INSERT OR REPLACE INTO elevation_tiles (tx, ty, data) VALUES (?1, ?2, ?3)",
        params![i64::from(tile.id.tx), i64::from(tile.id.ty), blob],
    )?;
    Ok(())
}

/// Raw compressed blobs intersecting `bbox` — kept separate from
/// [`decode_blobs`] so callers can release the connection lock before the
/// zlib inflation (decoding ~100 tiles takes tens of ms and must not
/// stall other users of a shared connection).
pub(super) fn blobs_in_bbox(
    conn: &Connection,
    bbox: BoundingBox,
) -> Result<Vec<(ElevationTileId, Vec<u8>)>, StoreError> {
    let (lo, hi) = grid::tile_range(bbox);
    let mut stmt = conn.prepare_cached(
        "SELECT tx, ty, data FROM elevation_tiles
         WHERE tx BETWEEN ?1 AND ?2 AND ty BETWEEN ?3 AND ?4
         ORDER BY ty, tx",
    )?;
    let rows = stmt.query_map(
        params![
            i64::from(lo.tx),
            i64::from(hi.tx),
            i64::from(lo.ty),
            i64::from(hi.ty)
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        },
    )?;
    let mut blobs = Vec::new();
    for row in rows {
        let (tx, ty, blob) = row?;
        let id = ElevationTileId { tx: tx as u32, ty: ty as u32 };
        blobs.push((id, blob));
    }
    Ok(blobs)
}

/// Decodes the blobs collected by [`blobs_in_bbox`].
pub(super) fn decode_blobs(
    blobs: Vec<(ElevationTileId, Vec<u8>)>,
) -> Result<Vec<ElevationTile>, StoreError> {
    blobs
        .into_iter()
        .map(|(id, blob)| decode(id, &blob))
        .collect()
}

pub(super) fn max_at(
    conn: &Connection,
    lat: f64,
    lon: f64,
) -> Result<Option<MetersAmsl>, StoreError> {
    let cell = cell_of(lat, lon);
    let Some(tile) = get(conn, ElevationTileId::of_cell(cell))? else {
        return Ok(None);
    };
    Ok(tile.value_at(cell).map(meters))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ElevationTileId {
        ElevationTileId::containing(50.5, 10.5)
    }

    fn uniform(value: i16) -> ElevationTile {
        ElevationTile::new(id(), vec![value; CELLS_PER_TILE]).expect("cell count")
    }

    #[test]
    fn new_rejects_wrong_cell_counts() {
        assert!(ElevationTile::new(id(), vec![0; CELLS_PER_TILE - 1]).is_err());
        assert!(ElevationTile::new(id(), Vec::new()).is_err());
        assert!(ElevationTile::new(id(), vec![0; CELLS_PER_TILE]).is_ok());
    }

    #[test]
    fn codec_round_trips_including_sentinel_and_extremes() {
        let mut cells = vec![ELEVATION_NO_DATA; CELLS_PER_TILE];
        cells[0] = -430; // Dead Sea-ish
        cells[1] = 0;
        cells[2] = 8849;
        cells[CELLS_PER_TILE - 1] = i16::MAX;
        let tile = ElevationTile::new(id(), cells).expect("cell count");

        let blob = encode(tile.cells()).expect("encode");
        let back = decode(tile.id(), &blob).expect("decode");
        assert_eq!(back, tile);
        // Mostly-sentinel tiles compress far below the 128 KiB raw size.
        assert!(blob.len() < CELLS_PER_TILE / 4, "blob is {} bytes", blob.len());
    }

    #[test]
    fn decode_rejects_garbage_and_truncated_blobs() {
        assert!(decode(id(), b"not zlib at all").is_err());

        let short = encode(&vec![7i16; CELLS_PER_TILE - 100]).expect("encode");
        let err = decode(id(), &short).expect_err("short blob must fail");
        assert!(matches!(err, StoreError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn max_at_reports_data_inside_and_none_outside_or_no_data() {
        let tile = uniform(123);
        assert_eq!(tile.max_at(50.5, 10.5), Some(MetersAmsl(123.0)));
        // A position in a different tile.
        assert_eq!(tile.max_at(52.5, 10.5), None);

        let empty = uniform(ELEVATION_NO_DATA);
        assert_eq!(empty.max_at(50.5, 10.5), None);
    }

    #[test]
    fn merge_max_keeps_the_higher_cell_and_real_data_beats_sentinel() {
        let mut a = uniform(100);
        a.cells[0] = ELEVATION_NO_DATA;
        a.cells[1] = 500;
        let mut b = uniform(200);
        b.cells[1] = ELEVATION_NO_DATA;

        a.merge_max(&b);
        assert_eq!(a.cells[0], 200, "sentinel loses to data");
        assert_eq!(a.cells[1], 500, "data survives the other side's sentinel");
        assert_eq!(a.cells[2], 200, "per-cell max");
    }
}

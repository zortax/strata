//! Pure cell/tile addressing math for the max-pooled elevation grid.
//!
//! The grid is global and fixed: square 6 arc-second (1/600°) cells anchored
//! at (90° S, 180° W). Cell `(x, y)` covers longitudes
//! `[-180 + x/600, -180 + (x+1)/600)` and latitudes
//! `[-90 + y/600, -90 + (y+1)/600)` — x grows east, y grows north. Tiles
//! group 256×256 cells: tile `(tx, ty)` owns cells
//! `x ∈ [tx·256, (tx+1)·256)`, `y ∈ [ty·256, (ty+1)·256)`.
//!
//! Positions exactly on a cell edge belong to the cell north/east of the
//! edge (floor semantics). Writers and readers must (and do) share these
//! functions, so a queried position always lands in the cell it was pooled
//! into.

use crate::domain::BoundingBox;

/// Cells per side of one stored elevation tile.
pub const ELEVATION_TILE_SIDE: usize = 256;

/// Grid resolution: 6 arc-second cells, 600 per degree (≈185 m north-south).
pub const ELEVATION_CELLS_PER_DEGREE: u32 = 600;

pub(super) const SIDE: u32 = ELEVATION_TILE_SIDE as u32;

/// Total cells around the globe (longitude) and pole to pole (latitude).
const CELLS_X: u32 = 360 * ELEVATION_CELLS_PER_DEGREE;
const CELLS_Y: u32 = 180 * ELEVATION_CELLS_PER_DEGREE;

/// A global cell index: x east from 180° W, y north from 90° S.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub x: u32,
    pub y: u32,
}

/// Global cell column of a longitude (finite degrees; the world edges
/// clamp into the outermost column).
pub(crate) fn cell_x_of(lon: f64) -> u32 {
    clamp_floor((lon + 180.0) * f64::from(ELEVATION_CELLS_PER_DEGREE), CELLS_X)
}

/// Global cell row of a latitude (finite degrees; the poles clamp into the
/// outermost row).
pub(crate) fn cell_y_of(lat: f64) -> u32 {
    clamp_floor((lat + 90.0) * f64::from(ELEVATION_CELLS_PER_DEGREE), CELLS_Y)
}

fn clamp_floor(value: f64, count: u32) -> u32 {
    (value.floor() as i64).clamp(0, i64::from(count) - 1) as u32
}

/// The cell containing `(lat, lon)`.
pub(crate) fn cell_of(lat: f64, lon: f64) -> Cell {
    Cell {
        x: cell_x_of(lon),
        y: cell_y_of(lat),
    }
}

/// Identifies one 256×256-cell elevation tile (see module docs for the
/// addressing scheme).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElevationTileId {
    pub tx: u32,
    pub ty: u32,
}

impl ElevationTileId {
    /// The tile whose cell grid contains `(lat, lon)` (finite degrees).
    pub fn containing(lat: f64, lon: f64) -> Self {
        Self::of_cell(cell_of(lat, lon))
    }

    pub(crate) fn of_cell(cell: Cell) -> Self {
        Self {
            tx: cell.x / SIDE,
            ty: cell.y / SIDE,
        }
    }

    /// South-west corner cell of this tile.
    pub(crate) fn origin(self) -> Cell {
        Cell {
            x: self.tx * SIDE,
            y: self.ty * SIDE,
        }
    }

    /// Row-major index of `cell` within this tile, counted from the
    /// south-west cell (columns west→east, rows south→north); `None` when
    /// the cell lies outside the tile.
    pub(crate) fn index_of(self, cell: Cell) -> Option<usize> {
        let origin = self.origin();
        let col = cell.x.checked_sub(origin.x)?;
        let row = cell.y.checked_sub(origin.y)?;
        (col < SIDE && row < SIDE).then(|| row as usize * ELEVATION_TILE_SIDE + col as usize)
    }
}

/// Inclusive tile-id corners covering `bbox` (south-west, north-east).
pub(super) fn tile_range(bbox: BoundingBox) -> (ElevationTileId, ElevationTileId) {
    (
        ElevationTileId::of_cell(cell_of(bbox.south(), bbox.west())),
        ElevationTileId::of_cell(cell_of(bbox.north(), bbox.east())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_corners_clamp_into_the_grid() {
        assert_eq!(cell_x_of(-180.0), 0);
        assert_eq!(cell_x_of(180.0), CELLS_X - 1);
        assert_eq!(cell_y_of(-90.0), 0);
        assert_eq!(cell_y_of(90.0), CELLS_Y - 1);
    }

    #[test]
    fn six_hundred_cells_per_degree() {
        // Integer degrees are exact in f64 — cell edges align with them.
        assert_eq!(cell_x_of(10.0), (10 + 180) * 600);
        assert_eq!(cell_x_of(11.0) - cell_x_of(10.0), 600);
        assert_eq!(cell_y_of(51.0) - cell_y_of(50.0), 600);
        // Mid-cell positions land in the cell west/south of the next edge.
        assert_eq!(cell_x_of(10.0005), (10 + 180) * 600);
        assert_eq!(cell_y_of(50.0005), (50 + 90) * 600);
    }

    #[test]
    fn tile_of_cell_and_origin_round_trip() {
        let cell = cell_of(50.5, 10.5);
        let id = ElevationTileId::of_cell(cell);
        assert_eq!(id, ElevationTileId::containing(50.5, 10.5));
        let origin = id.origin();
        assert!(origin.x <= cell.x && cell.x < origin.x + SIDE);
        assert!(origin.y <= cell.y && cell.y < origin.y + SIDE);
    }

    #[test]
    fn index_of_is_row_major_from_the_south_west() {
        let id = ElevationTileId { tx: 10, ty: 20 };
        let origin = id.origin();
        assert_eq!(id.index_of(origin), Some(0));
        assert_eq!(
            id.index_of(Cell { x: origin.x + 1, y: origin.y }),
            Some(1)
        );
        assert_eq!(
            id.index_of(Cell { x: origin.x, y: origin.y + 1 }),
            Some(ELEVATION_TILE_SIDE)
        );
        assert_eq!(
            id.index_of(Cell { x: origin.x + SIDE - 1, y: origin.y + SIDE - 1 }),
            Some(ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE - 1)
        );
        // Outside in every direction.
        assert_eq!(id.index_of(Cell { x: origin.x + SIDE, y: origin.y }), None);
        assert_eq!(id.index_of(Cell { x: origin.x, y: origin.y + SIDE }), None);
        assert_eq!(
            id.index_of(Cell { x: origin.x.wrapping_sub(1), y: origin.y }),
            None
        );
    }

    #[test]
    fn tile_range_orders_corners() {
        let bbox = BoundingBox::new(5.5, 47.0, 15.5, 55.2).expect("valid bbox");
        let (lo, hi) = tile_range(bbox);
        assert!(lo.tx <= hi.tx && lo.ty <= hi.ty);
        // Germany is 10° wide = 6000 cells ≈ 23.4 tiles; depending on how
        // the corners cut tile boundaries the inclusive span is 23 or 24.
        let span = hi.tx - lo.tx;
        assert!((23..=24).contains(&span), "span {span}");
    }
}

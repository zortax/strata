//! Screen-space label decluttering: a uniform grid of axis-aligned boxes.
//!
//! Skip-not-shift: callers insert boxes in priority order; a box that
//! overlaps anything already placed is rejected and its label is simply not
//! drawn this frame.

use glam::Vec2;
use rustc_hash::FxHashMap;

/// A grid over logical-px screen space. Built fresh each frame.
pub(crate) struct CollisionGrid {
    cell_px: f32,
    /// Cell → indices into `boxes` of every box touching that cell.
    cells: FxHashMap<(i32, i32), Vec<usize>>,
    boxes: Vec<(Vec2, Vec2)>,
}

impl CollisionGrid {
    pub fn new(cell_px: f32) -> Self {
        Self {
            cell_px: cell_px.max(1.0),
            cells: FxHashMap::default(),
            boxes: Vec::new(),
        }
    }

    /// Place `[min, max]` if it overlaps nothing already placed. Boxes that
    /// merely touch (shared edge) do not count as overlapping.
    pub fn try_insert(&mut self, min: Vec2, max: Vec2) -> bool {
        let (cx0, cy0, cx1, cy1) = self.cell_range(min, max);
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let Some(indices) = self.cells.get(&(cx, cy)) else {
                    continue;
                };
                for &i in indices {
                    let (omin, omax) = self.boxes[i];
                    if min.x < omax.x && omin.x < max.x && min.y < omax.y && omin.y < max.y {
                        return false;
                    }
                }
            }
        }
        let index = self.boxes.len();
        self.boxes.push((min, max));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                self.cells.entry((cx, cy)).or_default().push(index);
            }
        }
        true
    }

    fn cell_range(&self, min: Vec2, max: Vec2) -> (i32, i32, i32, i32) {
        (
            (min.x / self.cell_px).floor() as i32,
            (min.y / self.cell_px).floor() as i32,
            (max.x / self.cell_px).floor() as i32,
            (max.y / self.cell_px).floor() as i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn overlapping_box_is_rejected() {
        let mut grid = CollisionGrid::new(64.0);
        assert!(grid.try_insert(v(10.0, 10.0), v(60.0, 30.0)));
        assert!(!grid.try_insert(v(50.0, 20.0), v(100.0, 40.0)));
    }

    #[test]
    fn disjoint_boxes_both_fit() {
        let mut grid = CollisionGrid::new(64.0);
        assert!(grid.try_insert(v(0.0, 0.0), v(40.0, 20.0)));
        assert!(grid.try_insert(v(100.0, 100.0), v(140.0, 120.0)));
    }

    #[test]
    fn overlap_is_detected_across_cell_borders() {
        let mut grid = CollisionGrid::new(16.0);
        // Spans many cells.
        assert!(grid.try_insert(v(-10.0, 5.0), v(200.0, 12.0)));
        // Overlaps only in a cell far from the first box's origin.
        assert!(!grid.try_insert(v(150.0, 8.0), v(170.0, 30.0)));
    }

    #[test]
    fn touching_edges_do_not_collide() {
        let mut grid = CollisionGrid::new(64.0);
        assert!(grid.try_insert(v(0.0, 0.0), v(40.0, 20.0)));
        assert!(grid.try_insert(v(40.0, 0.0), v(80.0, 20.0)));
    }

    #[test]
    fn rejected_box_leaves_no_trace() {
        let mut grid = CollisionGrid::new(64.0);
        assert!(grid.try_insert(v(0.0, 0.0), v(40.0, 20.0)));
        assert!(!grid.try_insert(v(10.0, 10.0), v(50.0, 30.0)));
        // The rejected box must not block this one.
        assert!(grid.try_insert(v(41.0, 21.0), v(60.0, 40.0)));
    }

    #[test]
    fn negative_coordinates_work() {
        let mut grid = CollisionGrid::new(32.0);
        assert!(grid.try_insert(v(-100.0, -50.0), v(-60.0, -30.0)));
        assert!(!grid.try_insert(v(-70.0, -40.0), v(-20.0, -10.0)));
    }
}

use super::*;

use crate::camera::{Camera, Viewport};
use glam::UVec2;

#[test]
fn root_tile_spans_the_world() {
    let root = TileId::new(0, 0, 0).expect("root tile");
    let (min, max) = root.world_bounds();
    assert_eq!(min, DVec2::ZERO);
    assert_eq!(max, DVec2::ONE);
    assert_eq!(root.world_size(), 1.0);
    assert!(root.parent().is_none());
}

#[test]
fn new_rejects_out_of_range() {
    assert!(TileId::new(0, 1, 0).is_none());
    assert!(TileId::new(3, 8, 0).is_none());
    assert!(TileId::new(3, 0, 8).is_none());
    assert!(TileId::new(31, 0, 0).is_none());
    assert!(TileId::new(3, 7, 7).is_some());
}

#[test]
fn child_bounds_quarter_the_parent() {
    let root = TileId { z: 0, x: 0, y: 0 };
    let children = root.children().expect("children below max zoom");
    let (min, max) = children[3].world_bounds();
    assert_eq!(min, DVec2::new(0.5, 0.5));
    assert_eq!(max, DVec2::ONE);
}

#[test]
fn parent_child_round_trip() {
    let tile = TileId { z: 7, x: 67, y: 43 };
    let children = tile.children().expect("children");
    for child in children {
        assert_eq!(child.parent(), Some(tile));
        assert!(tile.is_ancestor_of(child));
    }
    assert_eq!(tile.ancestor(7), Some(tile));
    assert_eq!(tile.ancestor(5), Some(TileId { z: 5, x: 16, y: 10 }));
    assert!(tile.ancestor(8).is_none());
}

#[test]
fn containing_clamps_to_world() {
    assert_eq!(
        TileId::containing(2, DVec2::new(-0.5, 1.5)),
        TileId { z: 2, x: 0, y: 3 }
    );
    assert_eq!(
        TileId::containing(2, DVec2::new(0.999, 0.001)),
        TileId { z: 2, x: 3, y: 0 }
    );
}

#[test]
fn display_level_uses_bias_and_clamp() {
    assert_eq!(display_level(6.8, 13, TILE_PICK_BIAS), 7);
    assert_eq!(display_level(6.69, 13, TILE_PICK_BIAS), 6);
    // Overzoom clamps to the source max — never asks beyond the data.
    assert_eq!(display_level(19.0, 13, TILE_PICK_BIAS), 13);
    assert_eq!(display_level(4.0, 13, TILE_PICK_BIAS), 4);
}

#[test]
fn display_level_clamps_for_any_bias_and_max() {
    // Camera max zoom (19) against various declared source maxima: the
    // selected level must never exceed the source max, whatever the bias.
    for max in [9u8, 11, 13, 15] {
        for bias in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            assert_eq!(display_level(19.0, max, bias), max, "max={max} bias={bias}");
        }
    }
    // …and never go below zero at the shallow end.
    assert_eq!(display_level(0.0, 13, -2.0), 0);
}

#[test]
fn negative_detail_bias_delays_the_level_switch() {
    // With the old +0.3 bias z13 arrived at camera 12.7; the default
    // basemap bias must delay it past the integer boundary (0.5–1 later).
    let shift = TILE_PICK_BIAS - DEFAULT_BASEMAP_DETAIL_BIAS;
    assert!((0.5..=1.0).contains(&shift), "default shift {shift} out of range");
    assert_eq!(display_level(12.7, 13, DEFAULT_BASEMAP_DETAIL_BIAS), 12);
    assert_eq!(display_level(13.4, 13, DEFAULT_BASEMAP_DETAIL_BIAS), 12);
    assert_eq!(display_level(13.5, 13, DEFAULT_BASEMAP_DETAIL_BIAS), 13);
}

#[test]
fn tile_source_max_zoom_defaults_to_none() {
    struct Plain;
    impl TileSource for Plain {
        fn tile(&self, _id: TileId) -> Option<Vec<u8>> {
            None
        }
    }
    assert_eq!(Plain.max_zoom(), None);
}

fn camera_at(zoom: f64) -> Camera {
    let mut camera = Camera::new(Viewport::new(UVec2::new(1024, 768), 1.0));
    // Drive zoom through the public animation path to its target.
    camera.zoom_about(glam::DVec2::new(512.0, 384.0), zoom - camera.zoom());
    for _ in 0..2000 {
        camera.tick(std::time::Duration::from_millis(16));
    }
    assert_eq!(camera.zoom(), zoom);
    camera
}

#[test]
fn viewport_coverage_covers_visible_bounds() {
    let camera = camera_at(8.0);
    let coverage = viewport_coverage(&camera, 13, TILE_PICK_BIAS);
    assert_eq!(coverage.level, 8);
    assert!(!coverage.overzoomed);
    assert!(!coverage.tiles.is_empty());

    let (min, max) = camera.visible_world_bounds();
    // Every covering tile intersects the viewport, and the corners are covered.
    for tile in &coverage.tiles {
        let (tmin, tmax) = tile.world_bounds();
        assert!(tmax.x >= min.x && tmin.x <= max.x);
        assert!(tmax.y >= min.y && tmin.y <= max.y);
    }
    for corner in [min, max, DVec2::new(min.x, max.y), DVec2::new(max.x, min.y)] {
        let id = TileId::containing(coverage.level, corner);
        assert!(coverage.tiles.contains(&id), "corner {corner:?} uncovered");
    }
}

#[test]
fn viewport_coverage_overzooms_past_source_max() {
    let camera = camera_at(16.0);
    let coverage = viewport_coverage(&camera, 13, TILE_PICK_BIAS);
    assert_eq!(coverage.level, 13);
    assert!(coverage.overzoomed);
    // Deep overzoom over a small viewport: very few source tiles.
    assert!(coverage.tiles.len() <= 4, "got {}", coverage.tiles.len());
}

#[test]
fn viewport_coverage_at_camera_max_zoom_still_yields_tiles() {
    // Camera hard max (19) over a z13-deep source: coverage must clamp and
    // stay non-empty — the basemap never has "nothing to even ask for".
    let camera = camera_at(19.0);
    for bias in [DEFAULT_BASEMAP_DETAIL_BIAS, 0.0, TILE_PICK_BIAS] {
        let coverage = viewport_coverage(&camera, 13, bias);
        assert_eq!(coverage.level, 13, "bias {bias}");
        assert!(coverage.overzoomed, "bias {bias}");
        assert!(!coverage.tiles.is_empty(), "bias {bias}");
    }
}

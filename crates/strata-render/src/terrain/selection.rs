//! Pure tile-selection logic: level clamping, viewport coverage at a fixed
//! level, ancestor fallback and overzoom UV windows, and the per-frame draw
//! plan. Kept free of GPU/cache types so it is unit-testable.
//!
//! NOTE: this mirrors what the basemap layer needs as well; if the basemap
//! grows an identical helper set, hoist these into `crate::tiles`.

use crate::tiles::{TILE_PICK_BIAS, TileId, display_level};

use glam::DVec2;

/// Shallowest level terrain tiles exist at (ingest generates z5–11; the
/// deep end comes from `RendererConfig::max_terrain_zoom`).
pub(crate) const MIN_TERRAIN_LEVEL: u8 = 5;

/// The terrain tile level for a continuous camera zoom: the core
/// [`display_level`] (with the terrain pick bias [`TILE_PICK_BIAS`])
/// clamped into `[min_level, max_level]`. Below `min_level` the coarsest
/// tiles are underzoomed; beyond `max_level` they are overzoomed.
pub(crate) fn select_level(zoom: f64, min_level: u8, max_level: u8) -> u8 {
    display_level(zoom, max_level, TILE_PICK_BIAS).max(min_level.min(max_level))
}

/// Tiles at `level` covering the world-space rectangle `(min, max)`,
/// row-major, clamped to the world square.
pub(crate) fn coverage_at_level(level: u8, min: DVec2, max: DVec2) -> Vec<TileId> {
    let n = TileId::tiles_across(level);
    let lo = TileId::containing(level, min);
    let hi = TileId::containing(level, max);
    let mut tiles = Vec::with_capacity(((hi.x - lo.x + 1) as usize) * ((hi.y - lo.y + 1) as usize));
    for y in lo.y..=hi.y.min(n - 1) {
        for x in lo.x..=hi.x.min(n - 1) {
            tiles.push(TileId { z: level, x, y });
        }
    }
    tiles
}

/// What the cache knows about a tile, as seen by the planner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TileStatus {
    /// Texture resident; `fade` is the fade-in progress in `0..=1`.
    Ready { fade: f32 },
    /// Fetch/decode in flight.
    Pending,
    /// Source has no such tile (or it failed to decode) — do not re-request.
    Missing,
    /// Never seen — a fetch should be submitted.
    Unknown,
}

/// One quad to draw: `texture`'s tile texture stretched over `target`'s
/// world rect (UV sub-window via [`uv_window`] when `texture` is an
/// ancestor of `target`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlannedDraw {
    pub texture: TileId,
    pub target: TileId,
    pub fade: f32,
}

/// The per-frame outcome of planning: quads to draw (in paint order) and
/// tiles to fetch.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct TilePlan {
    pub draws: Vec<PlannedDraw>,
    pub fetch: Vec<TileId>,
}

/// Plan draws for the `needed` coverage. Per tile:
///
/// - ready & fully faded → draw it;
/// - ready & still fading → draw the nearest ready ancestor underneath
///   (no flicker), then the tile on top with its fade;
/// - not ready → draw the nearest ready ancestor windowed to the tile's
///   rect (no holes), and request a fetch if the tile is [`Unknown`].
///
/// [`Unknown`]: TileStatus::Unknown
pub(crate) fn plan_tiles(
    needed: &[TileId],
    min_level: u8,
    status: impl Fn(TileId) -> TileStatus,
) -> TilePlan {
    let mut plan = TilePlan::default();
    for &tile in needed {
        match status(tile) {
            TileStatus::Ready { fade } => {
                if fade < 1.0
                    && let Some(under) = nearest_ready_ancestor(tile, min_level, &status)
                {
                    plan.draws.push(PlannedDraw {
                        texture: under.texture,
                        target: tile,
                        fade: under.fade,
                    });
                }
                plan.draws.push(PlannedDraw {
                    texture: tile,
                    target: tile,
                    fade,
                });
            }
            not_ready => {
                if not_ready == TileStatus::Unknown {
                    plan.fetch.push(tile);
                }
                if let Some(fallback) = nearest_ready_ancestor(tile, min_level, &status) {
                    plan.draws.push(PlannedDraw {
                        texture: fallback.texture,
                        target: tile,
                        fade: fallback.fade,
                    });
                }
            }
        }
    }
    plan
}

struct ReadyAncestor {
    texture: TileId,
    fade: f32,
}

fn nearest_ready_ancestor(
    tile: TileId,
    min_level: u8,
    status: &impl Fn(TileId) -> TileStatus,
) -> Option<ReadyAncestor> {
    let mut current = tile;
    while let Some(parent) = current.parent() {
        if parent.z < min_level {
            return None;
        }
        if let TileStatus::Ready { fade } = status(parent) {
            return Some(ReadyAncestor {
                texture: parent,
                fade,
            });
        }
        current = parent;
    }
    None
}

/// UV rect `(min, max)` of `target`'s world rect within `texture`'s texture.
/// Identity for `texture == target`; `None` if `texture` is not an ancestor
/// of `target`. Texture v grows with world y (row 0 = north), so no flip.
pub(crate) fn uv_window(texture: TileId, target: TileId) -> Option<(DVec2, DVec2)> {
    if !texture.is_ancestor_of(target) {
        return None;
    }
    let shift = target.z - texture.z;
    let n = 1u64 << shift;
    let inv = 1.0 / n as f64;
    let dx = target.x as u64 - texture.x as u64 * n;
    let dy = target.y as u64 - texture.y as u64 * n;
    let min = DVec2::new(dx as f64 * inv, dy as f64 * inv);
    Some((min, min + DVec2::splat(inv)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use rustc_hash::FxHashMap;

    fn tile(z: u8, x: u32, y: u32) -> TileId {
        TileId::new(z, x, y).expect("valid tile")
    }

    #[test]
    fn select_level_clamps_to_terrain_range() {
        assert_eq!(select_level(4.0, 5, 11), 5);
        assert_eq!(select_level(0.0, 5, 11), 5);
        assert_eq!(select_level(19.0, 5, 11), 11);
        assert_eq!(select_level(8.5, 5, 11), 8);
    }

    #[test]
    fn select_level_uses_core_pick_bias() {
        // display_level = floor(zoom + 0.3)
        assert_eq!(select_level(6.6, 5, 11), 6);
        assert_eq!(select_level(6.8, 5, 11), 7);
    }

    #[test]
    fn select_level_survives_degenerate_range() {
        // max below min: clamp must not panic and must respect max.
        assert_eq!(select_level(10.0, 5, 3), 3);
    }

    #[test]
    fn coverage_is_row_major_and_complete() {
        // A rect inside tile (2..=3, 1..=2) at z3.
        let min = DVec2::new(2.1 / 8.0, 1.1 / 8.0);
        let max = DVec2::new(3.9 / 8.0, 2.9 / 8.0);
        let tiles = coverage_at_level(3, min, max);
        assert_eq!(
            tiles,
            vec![tile(3, 2, 1), tile(3, 3, 1), tile(3, 2, 2), tile(3, 3, 2)]
        );
    }

    #[test]
    fn coverage_clamps_to_world_edges() {
        let tiles = coverage_at_level(1, DVec2::new(-0.5, 0.6), DVec2::new(0.4, 1.7));
        assert_eq!(tiles, vec![tile(1, 0, 1)]);
    }

    #[test]
    fn uv_window_identity_and_quadrants() {
        let parent = tile(5, 10, 12);
        assert_eq!(uv_window(parent, parent), Some((DVec2::ZERO, DVec2::ONE)));
        let children = parent.children().expect("children");
        let expected = [
            (DVec2::new(0.0, 0.0), DVec2::new(0.5, 0.5)),
            (DVec2::new(0.5, 0.0), DVec2::new(1.0, 0.5)),
            (DVec2::new(0.0, 0.5), DVec2::new(0.5, 1.0)),
            (DVec2::new(0.5, 0.5), DVec2::new(1.0, 1.0)),
        ];
        for (child, want) in children.iter().zip(expected) {
            assert_eq!(uv_window(parent, *child), Some(want));
        }
        // Two levels down: a sixteenth.
        let grandchild = tile(7, 10 * 4 + 3, 12 * 4 + 1);
        assert_eq!(
            uv_window(parent, grandchild),
            Some((DVec2::new(0.75, 0.25), DVec2::new(1.0, 0.5)))
        );
    }

    #[test]
    fn uv_window_rejects_non_ancestors() {
        assert_eq!(uv_window(tile(5, 10, 12), tile(6, 0, 0)), None);
        assert_eq!(uv_window(tile(6, 0, 0), tile(5, 0, 0)), None);
    }

    fn status_map(entries: &[(TileId, TileStatus)]) -> impl Fn(TileId) -> TileStatus + '_ {
        let map: FxHashMap<TileId, TileStatus> = entries.iter().copied().collect();
        move |id| map.get(&id).copied().unwrap_or(TileStatus::Unknown)
    }

    #[test]
    fn plan_draws_ready_tiles_without_fallback() {
        let t = tile(7, 60, 40);
        let plan = plan_tiles(&[t], 5, status_map(&[(t, TileStatus::Ready { fade: 1.0 })]));
        assert_eq!(
            plan.draws,
            vec![PlannedDraw {
                texture: t,
                target: t,
                fade: 1.0
            }]
        );
        assert!(plan.fetch.is_empty());
    }

    #[test]
    fn plan_requests_unknown_but_not_pending_or_missing() {
        let unknown = tile(7, 60, 40);
        let pending = tile(7, 61, 40);
        let missing = tile(7, 62, 40);
        let plan = plan_tiles(
            &[unknown, pending, missing],
            5,
            status_map(&[
                (pending, TileStatus::Pending),
                (missing, TileStatus::Missing),
            ]),
        );
        assert_eq!(plan.fetch, vec![unknown]);
        assert!(plan.draws.is_empty(), "no ancestors ready, nothing to draw");
    }

    #[test]
    fn plan_falls_back_to_nearest_ready_ancestor() {
        let t = tile(8, 120, 80);
        let parent = tile(7, 60, 40);
        let grandparent = tile(6, 30, 20);
        // Grandparent ready, parent not: the *nearest ready* one wins.
        let plan = plan_tiles(
            &[t],
            5,
            status_map(&[
                (parent, TileStatus::Pending),
                (grandparent, TileStatus::Ready { fade: 1.0 }),
            ]),
        );
        assert_eq!(
            plan.draws,
            vec![PlannedDraw {
                texture: grandparent,
                target: t,
                fade: 1.0
            }]
        );
        assert_eq!(plan.fetch, vec![t]);
    }

    #[test]
    fn plan_does_not_fall_back_below_min_level() {
        let t = tile(6, 30, 20);
        let parent = tile(5, 15, 10);
        let plan = plan_tiles(
            &[t],
            6,
            status_map(&[(parent, TileStatus::Ready { fade: 1.0 })]),
        );
        assert!(plan.draws.is_empty());
    }

    #[test]
    fn plan_underdraws_ancestor_while_fading_in() {
        let t = tile(8, 120, 80);
        let parent = tile(7, 60, 40);
        let plan = plan_tiles(
            &[t],
            5,
            status_map(&[
                (t, TileStatus::Ready { fade: 0.4 }),
                (parent, TileStatus::Ready { fade: 1.0 }),
            ]),
        );
        assert_eq!(
            plan.draws,
            vec![
                PlannedDraw {
                    texture: parent,
                    target: t,
                    fade: 1.0
                },
                PlannedDraw {
                    texture: t,
                    target: t,
                    fade: 0.4
                },
            ]
        );
    }
}

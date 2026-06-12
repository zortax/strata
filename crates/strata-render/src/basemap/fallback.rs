//! Ancestor-fallback draw planning, kept pure (a function over cache state)
//! so the zero-pop guarantee is unit-testable without a GPU.

use crate::tiles::TileId;

/// What the cache knows about one tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileReadiness {
    /// Not cached (not requested yet, in flight, or failed).
    Missing,
    /// Cached and authoritative, but contains no drawable geometry — nothing
    /// to draw here and no fallback wanted.
    Empty,
    /// Cached with a GPU mesh; `fading` while the ~150 ms fade-in runs.
    Ready { fading: bool },
}

/// Upper bound on stacked draws per wanted tile (a fully-opaque base plus a
/// few fading generations is all that is ever useful).
const MAX_STACK: usize = 4;

/// The meshes to draw for `wanted`, bottom (coarsest) first, given cache
/// state. Guarantees: if any ancestor of a missing/fading tile is ready, the
/// area is covered by the nearest ready one — zooming never shows holes; a
/// fading tile is always backed by the nearest opaque ancestor underneath.
pub fn plan_draws(wanted: TileId, readiness: impl Fn(TileId) -> TileReadiness) -> Vec<TileId> {
    let mut draws = Vec::new();
    let mut current = Some(wanted);
    while let Some(tile) = current {
        match readiness(tile) {
            TileReadiness::Ready { fading } => {
                draws.push(tile);
                if !fading || draws.len() >= MAX_STACK {
                    break;
                }
            }
            // An authoritative empty tile means the area truly has nothing —
            // a coarser ancestor would only paint stale geometry over it.
            TileReadiness::Empty => break,
            TileReadiness::Missing => {}
        }
        current = tile.parent();
    }
    draws.reverse();
    draws
}

#[cfg(test)]
mod tests {
    use super::*;

    use rustc_hash::FxHashMap;

    fn tile(z: u8, x: u32, y: u32) -> TileId {
        TileId::new(z, x, y).expect("valid tile id")
    }

    fn lookup(states: &[(TileId, TileReadiness)]) -> impl Fn(TileId) -> TileReadiness {
        let map: FxHashMap<TileId, TileReadiness> = states.iter().copied().collect();
        move |id| map.get(&id).copied().unwrap_or(TileReadiness::Missing)
    }

    #[test]
    fn ready_tile_draws_alone() {
        let wanted = tile(10, 530, 340);
        let draws = plan_draws(
            wanted,
            lookup(&[(wanted, TileReadiness::Ready { fading: false })]),
        );
        assert_eq!(draws, vec![wanted]);
    }

    #[test]
    fn missing_tile_falls_back_to_nearest_ready_ancestor() {
        let wanted = tile(10, 530, 340);
        let grandparent = tile(8, 132, 85);
        assert!(grandparent.is_ancestor_of(wanted));
        let draws = plan_draws(
            wanted,
            lookup(&[
                (grandparent, TileReadiness::Ready { fading: false }),
                // An even coarser ready ancestor must NOT be picked.
                (tile(6, 33, 21), TileReadiness::Ready { fading: false }),
            ]),
        );
        assert_eq!(draws, vec![grandparent]);
    }

    #[test]
    fn fading_tile_is_backed_by_opaque_ancestor() {
        let wanted = tile(10, 530, 340);
        let parent = tile(9, 265, 170);
        let draws = plan_draws(
            wanted,
            lookup(&[
                (wanted, TileReadiness::Ready { fading: true }),
                (parent, TileReadiness::Ready { fading: false }),
            ]),
        );
        assert_eq!(
            draws,
            vec![parent, wanted],
            "opaque base first, fade on top"
        );
    }

    #[test]
    fn fading_chain_stacks_bottom_first() {
        let wanted = tile(10, 530, 340);
        let parent = tile(9, 265, 170);
        let grandparent = tile(8, 132, 85);
        let draws = plan_draws(
            wanted,
            lookup(&[
                (wanted, TileReadiness::Ready { fading: true }),
                (parent, TileReadiness::Ready { fading: true }),
                (grandparent, TileReadiness::Ready { fading: false }),
            ]),
        );
        assert_eq!(draws, vec![grandparent, parent, wanted]);
    }

    #[test]
    fn empty_tile_draws_nothing_and_blocks_fallback() {
        let wanted = tile(10, 530, 340);
        let parent = tile(9, 265, 170);
        let draws = plan_draws(
            wanted,
            lookup(&[
                (wanted, TileReadiness::Empty),
                (parent, TileReadiness::Ready { fading: false }),
            ]),
        );
        assert!(draws.is_empty());
    }

    #[test]
    fn empty_ancestor_stops_the_walk_for_missing_tiles() {
        let wanted = tile(10, 530, 340);
        let parent = tile(9, 265, 170);
        let grandparent = tile(8, 132, 85);
        let draws = plan_draws(
            wanted,
            lookup(&[
                (parent, TileReadiness::Empty),
                (grandparent, TileReadiness::Ready { fading: false }),
            ]),
        );
        assert!(draws.is_empty());
    }

    #[test]
    fn nothing_cached_draws_nothing() {
        let draws = plan_draws(tile(10, 530, 340), |_| TileReadiness::Missing);
        assert!(draws.is_empty());
    }

    /// The walk through missing tiles is depth-unbounded: even a far-away
    /// ancestor (here 13 levels up — the root) still stands in. This is what
    /// keeps a deeply overzoomed view covered while a fresh ingest has only
    /// written low-zoom tiles so far.
    #[test]
    fn fallback_walk_reaches_arbitrarily_deep_ancestors() {
        let wanted = tile(13, 4400, 2686);
        let root = tile(0, 0, 0);
        let draws = plan_draws(
            wanted,
            lookup(&[(root, TileReadiness::Ready { fading: false })]),
        );
        assert_eq!(draws, vec![root]);
    }

    #[test]
    fn stack_depth_is_bounded() {
        // Every ancestor fading: the stack must cap at MAX_STACK draws.
        let wanted = tile(12, 2120, 1360);
        let draws = plan_draws(wanted, |_| TileReadiness::Ready { fading: true });
        assert_eq!(draws.len(), MAX_STACK);
        assert_eq!(*draws.last().expect("non-empty"), wanted);
    }
}

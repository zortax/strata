//! LRU tile cache with fade-in state, in-flight tracking and a TTL'd
//! negative cache for tiles the source does not have. Generic over the
//! resident resource so the eviction/fade logic is testable without a GPU.
//!
//! Negative entries expire after `missing_ttl`: terrain tiles can be
//! ingested (`strata-ingest terrain`) while the app runs, and the WAL store
//! read would see them — expired entries let new tiles appear on the next
//! rendered frame after the TTL instead of requiring a restart.

use super::selection::TileStatus;
use crate::tiles::TileId;

use lru::LruCache;
use rustc_hash::FxHashSet;

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

/// Fade-in duration for freshly uploaded tiles.
pub(crate) const FADE_IN_SECONDS: f32 = 0.15;

struct ReadyTile<T> {
    resource: T,
    fade: f32,
}

pub(crate) struct TileCache<T> {
    ready: LruCache<TileId, ReadyTile<T>>,
    pending: FxHashSet<TileId>,
    /// Negative cache: when each miss was recorded; entries older than
    /// `missing_ttl` no longer count as missing.
    missing: LruCache<TileId, Instant>,
    missing_ttl: Duration,
}

impl<T> TileCache<T> {
    pub fn new(capacity: usize, missing_capacity: usize, missing_ttl: Duration) -> Self {
        let cap = |n: usize| NonZeroUsize::new(n.max(1)).unwrap_or(NonZeroUsize::MIN);
        Self {
            ready: LruCache::new(cap(capacity)),
            pending: FxHashSet::default(),
            missing: LruCache::new(cap(missing_capacity)),
            missing_ttl,
        }
    }

    /// Whether `id` has a still-valid negative entry (non-promoting).
    fn missing_fresh(&self, id: TileId) -> bool {
        self.missing
            .peek(&id)
            .is_some_and(|at| at.elapsed() < self.missing_ttl)
    }

    /// Non-promoting status lookup for the planner. Expired negative entries
    /// report [`TileStatus::Unknown`] so the planner re-requests them.
    pub fn status(&self, id: TileId) -> TileStatus {
        if let Some(tile) = self.ready.peek(&id) {
            TileStatus::Ready { fade: tile.fade }
        } else if self.pending.contains(&id) {
            TileStatus::Pending
        } else if self.missing_fresh(id) {
            TileStatus::Missing
        } else {
            TileStatus::Unknown
        }
    }

    /// Mark `id` in-flight. `false` when a fetch would be redundant (already
    /// resident, pending or freshly known missing). Expired negative entries
    /// are dropped lazily here and the fetch proceeds.
    pub fn begin_fetch(&mut self, id: TileId) -> bool {
        if self.ready.peek(&id).is_some() {
            return false;
        }
        match self.missing.peek(&id) {
            Some(at) if at.elapsed() < self.missing_ttl => return false,
            Some(_) => {
                self.missing.pop(&id);
            }
            None => {}
        }
        self.pending.insert(id)
    }

    /// Insert a resident tile (fade starts at 0) and clear bookkeeping.
    pub fn insert_ready(&mut self, id: TileId, resource: T) {
        self.pending.remove(&id);
        self.missing.pop(&id);
        self.ready.put(
            id,
            ReadyTile {
                resource,
                fade: 0.0,
            },
        );
    }

    /// Negative-cache `id`: the source has no such tile (or decode failed).
    /// Re-checked once the TTL passes.
    pub fn insert_missing(&mut self, id: TileId) {
        self.pending.remove(&id);
        self.missing.put(id, Instant::now());
    }

    /// Non-promoting resource access (used during draw).
    pub fn resource(&self, id: TileId) -> Option<&T> {
        self.ready.peek(&id).map(|tile| &tile.resource)
    }

    /// Mark `id` recently used so the LRU keeps drawn tiles resident.
    pub fn promote(&mut self, id: TileId) {
        self.ready.promote(&id);
    }

    /// Advance every fade by `dt`; returns true while any tile is still
    /// fading in.
    pub fn advance_fades(&mut self, dt: Duration) -> bool {
        let step = dt.as_secs_f32() / FADE_IN_SECONDS;
        let mut fading = false;
        for (_, tile) in self.ready.iter_mut() {
            if tile.fade < 1.0 {
                tile.fade = (tile.fade + step).min(1.0);
                fading |= tile.fade < 1.0;
            }
        }
        fading
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub fn ready_count(&self) -> usize {
        self.ready.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32) -> TileId {
        TileId::new(8, x, 0).expect("valid tile")
    }

    /// Long enough that nothing expires within a test run.
    const TTL: Duration = Duration::from_secs(3600);

    #[test]
    fn begin_fetch_marks_pending_exactly_once() {
        let mut cache = TileCache::<u32>::new(4, 4, TTL);
        assert_eq!(cache.status(tile(1)), TileStatus::Unknown);
        assert!(cache.begin_fetch(tile(1)));
        assert!(!cache.begin_fetch(tile(1)), "already pending");
        assert_eq!(cache.status(tile(1)), TileStatus::Pending);
        assert_eq!(cache.pending_count(), 1);
    }

    #[test]
    fn insert_ready_clears_pending_and_fades_in() {
        let mut cache = TileCache::new(4, 4, TTL);
        assert!(cache.begin_fetch(tile(1)));
        cache.insert_ready(tile(1), 42u32);
        assert_eq!(cache.pending_count(), 0);
        assert_eq!(cache.status(tile(1)), TileStatus::Ready { fade: 0.0 });
        assert_eq!(cache.resource(tile(1)), Some(&42));
        assert!(
            !cache.begin_fetch(tile(1)),
            "resident tiles are not refetched"
        );

        // Half the fade duration → fade 0.5; full duration clamps at 1.
        let half = Duration::from_secs_f32(FADE_IN_SECONDS / 2.0);
        assert!(cache.advance_fades(half));
        assert_eq!(cache.status(tile(1)), TileStatus::Ready { fade: 0.5 });
        assert!(!cache.advance_fades(Duration::from_secs_f32(FADE_IN_SECONDS)));
        assert_eq!(cache.status(tile(1)), TileStatus::Ready { fade: 1.0 });
    }

    #[test]
    fn insert_missing_negative_caches() {
        let mut cache = TileCache::<u32>::new(4, 4, TTL);
        assert!(cache.begin_fetch(tile(2)));
        cache.insert_missing(tile(2));
        assert_eq!(cache.pending_count(), 0);
        assert_eq!(cache.status(tile(2)), TileStatus::Missing);
        assert!(
            !cache.begin_fetch(tile(2)),
            "missing tiles are not refetched within the TTL"
        );
    }

    /// With an expired TTL the negative entry no longer blocks anything:
    /// the planner sees `Unknown`, the refetch proceeds, and a tile ingested
    /// while the app runs (`strata-ingest terrain`) becomes resident.
    #[test]
    fn expired_missing_entries_are_refetchable() {
        let mut cache = TileCache::<u32>::new(4, 4, Duration::ZERO);
        cache.insert_missing(tile(3));
        assert_eq!(cache.status(tile(3)), TileStatus::Unknown);
        assert!(cache.begin_fetch(tile(3)), "expired miss must refetch");
        cache.insert_ready(tile(3), 7);
        assert!(matches!(cache.status(tile(3)), TileStatus::Ready { .. }));
        assert_eq!(cache.resource(tile(3)), Some(&7));
    }

    #[test]
    fn lru_evicts_least_recently_used_beyond_capacity() {
        let mut cache = TileCache::new(2, 4, TTL);
        cache.insert_ready(tile(1), 1u32);
        cache.insert_ready(tile(2), 2u32);
        cache.promote(tile(1)); // tile 2 is now least recently used
        cache.insert_ready(tile(3), 3u32);
        assert_eq!(cache.ready_count(), 2);
        assert_eq!(cache.status(tile(2)), TileStatus::Unknown, "evicted");
        assert!(matches!(cache.status(tile(1)), TileStatus::Ready { .. }));
        assert!(matches!(cache.status(tile(3)), TileStatus::Ready { .. }));
    }
}

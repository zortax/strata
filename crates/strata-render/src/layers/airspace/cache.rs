//! Persistent per-feature mesh cache with an LRU byte budget.
//!
//! Each entry holds one airspace's tessellated geometry in coordinates
//! local to its **own** origin, so cached meshes are valid in any future
//! set and can be rebased cheaply during assembly. The byte budget (vertex
//! plus index bytes) is the multi-country scaling story: a single country's
//! airspaces stay fully resident, while continent-sized datasets evict by
//! recency instead of growing without bound.

use crate::layers::tess::{FillMesh, FillVertex, LineMesh, LineVertex};
use crate::text::LabelRequest;

use glam::DVec2;
use lru::LruCache;

use std::sync::Arc;

/// Default LRU byte budget for the airspace mesh cache (≈128 MiB of vertex
/// and index data). Override via
/// [`crate::renderer::RendererConfig::airspace_mesh_cache_bytes`].
pub const DEFAULT_AIRSPACE_MESH_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// Cache identity: feature id × theme generation. A theme switch bumps the
/// generation so every feature misses and re-tessellates with the new
/// colors; stale generations are swept opportunistically on the next set
/// change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub id: u64,
    pub theme_generation: u64,
}

/// One feature's tessellated geometry — pure, reusable artifacts.
///
/// Vertices are in world units **relative to [`Self::origin`]** (a point on
/// the feature itself, f64), so f32 only ever holds feature-sized
/// quantities; assembly rebases them to a common set origin with one small
/// f32 offset per feature. The band label (pole-of-inaccessibility anchor —
/// pure geometry) is cached alongside and never recomputed on a hit.
#[derive(Debug, Clone)]
pub struct FeatureMesh {
    /// Per-feature local origin: exterior-ring bbox center, f64 world units.
    pub origin: DVec2,
    pub fill: FillMesh,
    pub border: LineMesh,
    /// Vertical-band label with an absolute world anchor (origin-free).
    pub label: Option<LabelRequest>,
    /// Geometry + style + label fingerprint; guards against a feature id
    /// being re-fed with different content.
    pub fingerprint: u64,
}

impl FeatureMesh {
    /// Bytes the budget accounts for: vertex + index data of both meshes.
    pub fn byte_size(&self) -> usize {
        self.fill.vertices.len() * std::mem::size_of::<FillVertex>()
            + self.fill.indices.len() * std::mem::size_of::<u32>()
            + self.border.vertices.len() * std::mem::size_of::<LineVertex>()
            + self.border.indices.len() * std::mem::size_of::<u32>()
    }
}

/// Hit/miss/eviction counters since the last [`MeshCache::take_stats`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// LRU feature-mesh cache enforcing a byte budget.
///
/// Entries are `Arc`-shared with the layer's current set, so evicting an
/// entry that is still on screen never breaks drawing — it only means the
/// next revisit re-tessellates it.
pub struct MeshCache {
    entries: LruCache<CacheKey, Arc<FeatureMesh>>,
    budget_bytes: usize,
    resident_bytes: usize,
    stats: CacheStats,
}

impl MeshCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: LruCache::unbounded(),
            budget_bytes,
            resident_bytes: 0,
            stats: CacheStats::default(),
        }
    }

    /// Change the byte budget; shrinking evicts immediately.
    #[cfg(test)]
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget_bytes = bytes;
        self.enforce_budget();
    }

    #[cfg(test)]
    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    /// Vertex + index bytes currently resident.
    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Look up a feature mesh, refreshing its recency. A fingerprint
    /// mismatch (same id, different geometry/style/label) is a miss and
    /// drops the stale entry.
    pub fn get(&mut self, key: CacheKey, fingerprint: u64) -> Option<Arc<FeatureMesh>> {
        let found = self.entries.get(&key).map(Arc::clone);
        match found {
            Some(mesh) if mesh.fingerprint == fingerprint => {
                self.stats.hits += 1;
                Some(mesh)
            }
            Some(_) => {
                if let Some(stale) = self.entries.pop(&key) {
                    self.resident_bytes -= stale.byte_size();
                }
                self.stats.misses += 1;
                None
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// Insert (most-recent position) and evict least-recently-used entries
    /// until the budget holds. A mesh larger than the entire budget is not
    /// cached at all — the caller's `Arc` still draws it.
    pub fn insert(&mut self, key: CacheKey, mesh: Arc<FeatureMesh>) {
        let bytes = mesh.byte_size();
        if bytes > self.budget_bytes {
            return;
        }
        if let Some(replaced) = self.entries.put(key, mesh) {
            self.resident_bytes -= replaced.byte_size();
        }
        self.resident_bytes += bytes;
        self.enforce_budget();
    }

    /// Drop every entry from a theme generation other than `current` —
    /// they can never hit again (the generation only moves forward).
    pub fn sweep_stale(&mut self, current: u64) {
        let stale: Vec<CacheKey> = self
            .entries
            .iter()
            .filter(|(key, _)| key.theme_generation != current)
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(mesh) = self.entries.pop(&key) {
                self.resident_bytes -= mesh.byte_size();
            }
        }
    }

    /// Counters since the previous call (reset on read).
    pub fn take_stats(&mut self) -> CacheStats {
        std::mem::take(&mut self.stats)
    }

    fn enforce_budget(&mut self) {
        while self.resident_bytes > self.budget_bytes {
            let Some((_, evicted)) = self.entries.pop_lru() else {
                break;
            };
            self.resident_bytes -= evicted.byte_size();
            self.stats.evictions += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic mesh with a deterministic byte size:
    /// `n` fill vertices (24 B) + `n` indices (4 B) = 28 B per unit.
    fn mesh(n: usize, fingerprint: u64) -> Arc<FeatureMesh> {
        Arc::new(FeatureMesh {
            origin: DVec2::ZERO,
            fill: FillMesh {
                vertices: vec![
                    FillVertex {
                        pos: [0.0; 2],
                        color: [0.0; 4],
                    };
                    n
                ],
                indices: vec![0; n],
            },
            border: LineMesh::default(),
            label: None,
            fingerprint,
        })
    }

    fn key(id: u64, theme_generation: u64) -> CacheKey {
        CacheKey {
            id,
            theme_generation,
        }
    }

    const UNIT: usize = std::mem::size_of::<FillVertex>() + std::mem::size_of::<u32>();

    #[test]
    fn byte_size_counts_vertices_and_indices() {
        assert_eq!(mesh(10, 0).byte_size(), 10 * UNIT);
    }

    #[test]
    fn hit_returns_the_cached_mesh_and_counts() {
        let mut cache = MeshCache::new(usize::MAX);
        cache.insert(key(1, 0), mesh(4, 77));
        assert!(cache.get(key(1, 0), 77).is_some());
        assert!(cache.get(key(2, 0), 77).is_none());
        let stats = cache.take_stats();
        assert_eq!((stats.hits, stats.misses), (1, 1));
        assert_eq!(cache.take_stats(), CacheStats::default(), "stats reset");
    }

    /// Same id, different content: the stale entry must not be served (and
    /// is dropped so its bytes free up).
    #[test]
    fn fingerprint_mismatch_is_a_miss_and_drops_the_entry() {
        let mut cache = MeshCache::new(usize::MAX);
        cache.insert(key(1, 0), mesh(4, 77));
        assert!(cache.get(key(1, 0), 78).is_none());
        assert_eq!(cache.resident_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn eviction_respects_the_byte_budget_and_evicts_least_recent() {
        // Budget fits exactly two 10-unit meshes.
        let mut cache = MeshCache::new(2 * 10 * UNIT);
        cache.insert(key(1, 0), mesh(10, 0));
        cache.insert(key(2, 0), mesh(10, 0));
        assert_eq!(cache.resident_bytes(), 2 * 10 * UNIT);

        // Touch 1 so 2 becomes the least recently used, then overflow.
        assert!(cache.get(key(1, 0), 0).is_some());
        cache.insert(key(3, 0), mesh(10, 0));

        assert!(cache.get(key(2, 0), 0).is_none(), "LRU entry evicted");
        assert!(cache.get(key(1, 0), 0).is_some(), "recently used survives");
        assert!(cache.get(key(3, 0), 0).is_some());
        assert!(cache.resident_bytes() <= cache.budget_bytes());
        assert_eq!(cache.take_stats().evictions, 1);
    }

    #[test]
    fn oversized_mesh_is_not_cached() {
        let mut cache = MeshCache::new(5 * UNIT);
        cache.insert(key(1, 0), mesh(10, 0));
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.resident_bytes(), 0);
    }

    #[test]
    fn shrinking_the_budget_evicts_immediately() {
        let mut cache = MeshCache::new(usize::MAX);
        for id in 0..4 {
            cache.insert(key(id, 0), mesh(10, 0));
        }
        cache.set_budget(2 * 10 * UNIT);
        assert_eq!(cache.entry_count(), 2);
        assert!(cache.resident_bytes() <= cache.budget_bytes());
    }

    /// Theme switch = new generation: old-generation entries miss and the
    /// sweep reclaims their bytes.
    #[test]
    fn theme_generation_invalidates_and_sweeps() {
        let mut cache = MeshCache::new(usize::MAX);
        cache.insert(key(1, 0), mesh(10, 9));
        cache.insert(key(2, 0), mesh(10, 9));
        assert!(cache.get(key(1, 1), 9).is_none(), "new generation misses");
        cache.sweep_stale(1);
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.resident_bytes(), 0);
    }
}

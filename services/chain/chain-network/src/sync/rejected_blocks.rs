use std::num::NonZeroUsize;

use lb_core::header::HeaderId;
use lru::LruCache;
use tracing::debug;

use crate::{metrics, sync::LOG_TARGET};

/// Bounded LRU of block IDs that the orphan pipeline should skip
/// (known-invalid or older-than-LIB).
///
/// A capacity of `0` disables the cache: all queries return `false` and all
/// insertions are no-ops. This allows operators to turn the feature off via
/// config without conditional logic at every call site.
pub struct RejectedBlocks {
    cache: Option<LruCache<HeaderId, ()>>,
}

impl RejectedBlocks {
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: NonZeroUsize::new(capacity).map(LruCache::new),
        }
    }

    /// Returns `true` if either the block itself or its parent (when known) is
    /// in the cache. Touches matching entries on hit so frequently-checked
    /// rejections stay in the cache.
    ///
    /// Always returns `false` when the cache is disabled.
    pub fn contains_block_or_parent(
        &mut self,
        block_id: &HeaderId,
        parent_id: Option<&HeaderId>,
    ) -> bool {
        let Some(cache) = self.cache.as_mut() else {
            return false;
        };
        cache.get(block_id).is_some() || parent_id.is_some_and(|p| cache.get(p).is_some())
    }

    /// No-op when the cache is disabled.
    pub fn insert(&mut self, block_id: HeaderId) {
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        if cache.put(block_id, ()).is_none() {
            debug!(target: LOG_TARGET, ?block_id, "inserted rejected block into cache");
            metrics::orphan_blocks_rejected_inserted_total();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_cache_never_rejects_blocks_or_parents() {
        let id = HeaderId::from([1; 32]);
        let mut cache = RejectedBlocks::new(0);
        cache.insert(id);
        assert!(!cache.contains_block_or_parent(&id, Some(&id)));
    }

    #[test]
    fn hits_refresh_recency_before_eviction() {
        let [first, second, third, child] = [1, 2, 3, 4].map(|byte| HeaderId::from([byte; 32]));
        let mut cache = RejectedBlocks::new(2);
        cache.insert(first);
        cache.insert(second);
        assert!(cache.contains_block_or_parent(&child, Some(&first)));
        cache.insert(third);
        assert!(!cache.contains_block_or_parent(&second, None));
        assert!(cache.contains_block_or_parent(&first, None));
        assert!(cache.contains_block_or_parent(&third, None));
    }

    #[test]
    fn duplicate_insertion_preserves_other_entry() {
        let first = HeaderId::from([1; 32]);
        let second = HeaderId::from([2; 32]);
        let mut cache = RejectedBlocks::new(2);
        cache.insert(first);
        cache.insert(second);
        cache.insert(first);
        assert!(cache.contains_block_or_parent(&first, None));
        assert!(cache.contains_block_or_parent(&second, None));
    }
}

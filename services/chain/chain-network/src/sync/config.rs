use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

const MAX_ORPHAN_CACHE_SIZE: NonZeroUsize =
    NonZeroUsize::new(1000).expect("MAX_ORPHAN_CACHE_SIZE must be non-zero");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    pub orphan: OrphanConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrphanConfig {
    /// The maximum number of pending orphans to keep in the cache.
    #[serde(default = "default_max_orphan_cache_size")]
    pub max_orphan_cache_size: NonZeroUsize,
    /// The maximum number of block IDs to remember in the rejected-blocks
    /// negative cache. The orphan pipeline consults this cache to short-circuit
    /// known-bad/older-than-LIB blocks before enqueuing or downloading them.
    ///
    /// Setting this to `0` disables the cache entirely.
    pub max_rejected_cache_size: usize,
}

const fn default_max_orphan_cache_size() -> NonZeroUsize {
    MAX_ORPHAN_CACHE_SIZE
}

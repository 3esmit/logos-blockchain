use std::{collections::HashSet, hash::Hash, num::NonZeroUsize, time::Duration};

use serde::{Deserialize, Serialize};

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BootstrapConfig<NodeId>
where
    NodeId: Clone + Eq + Hash,
{
    pub ibd: IbdConfig<NodeId>,
}

/// IBD configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IbdConfig<NodeId>
where
    NodeId: Clone + Eq + Hash,
{
    /// Trusted peers to query for the chain tip during IBD.
    pub trusted_peers: HashSet<NodeId>,
    /// Retry policy for tip-fetch batches.
    pub tips_fetch: TipsFetchConfig,
}

/// Retry policy for tip-fetch batches at the start of each IBD round.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct TipsFetchConfig {
    /// Total number of attempts.
    pub attempts: NonZeroUsize,
    /// Fixed delay between attempts.
    pub delay_between_attempts: Duration,
}

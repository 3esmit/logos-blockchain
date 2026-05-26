use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
    sync::Arc,
};

use lb_core::{header::HeaderId, mantle::Transaction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxHistoryState<TxId>
where
    TxId: Eq + Hash,
{
    pub arrivals: BTreeMap<u64, TxId>,
    tx_index: HashMap<TxId, u64>,
    block_txs: HashMap<HeaderId, Vec<TxId>>,
    next_version: u64,
}

/// Versioned record of mempool arrivals plus the per-block tx-id cache used
/// to prune them.
///
/// Forks that branch from a historical (and therefore stale) ancestor catch
/// up by replaying the slice of arrivals since the ancestor was demoted —
/// the alternative of eagerly applying every arrival to every historical
/// state would duplicate tx bodies across forks.
pub struct TxHistory<Tx, TxId>
where
    TxId: Eq + Hash,
{
    arrivals: BTreeMap<u64, Arc<Tx>>,
    tx_index: HashMap<TxId, u64>,
    block_txs: HashMap<HeaderId, Vec<TxId>>,
    next_version: u64,
}

impl<Tx, TxId> Default for TxHistory<Tx, TxId>
where
    TxId: Eq + Hash,
{
    fn default() -> Self {
        Self {
            arrivals: BTreeMap::new(),
            tx_index: HashMap::new(),
            block_txs: HashMap::new(),
            next_version: 0,
        }
    }
}

impl<Tx, TxId> TxHistory<Tx, TxId>
where
    TxId: Eq + Hash,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// Version a state should be tagged with after applying every arrival
    /// recorded so far. Also the version the next arrival will receive.
    pub const fn version(&self) -> u64 {
        self.next_version
    }
}

impl<Tx> TxHistory<Tx, Tx::Hash>
where
    Tx: Transaction + Clone,
{
    pub fn to_state(&self) -> TxHistoryState<Tx::Hash> {
        TxHistoryState {
            arrivals: self
                .arrivals
                .iter()
                .map(|(&idx, tx)| (idx, tx.hash()))
                .collect(),
            tx_index: self.tx_index.clone(),
            block_txs: self.block_txs.clone(),
            next_version: self.next_version,
        }
    }

    pub fn from_state_and_txs(
        TxHistoryState {
            arrivals,
            tx_index,
            block_txs,
            next_version,
        }: TxHistoryState<Tx::Hash>,
        txs: &HashMap<Tx::Hash, Arc<Tx>>,
    ) -> Self {
        let arrivals: BTreeMap<u64, Arc<Tx>> = arrivals
            .iter()
            .map(|(&idx, tx_hash)| {
                (
                    idx,
                    Arc::clone(
                        txs.get(tx_hash)
                            .expect("Tx should be present in saved state"),
                    ),
                )
            })
            .collect();
        Self {
            arrivals,
            tx_index,
            block_txs,
            next_version,
        }
    }

    /// Append `tx` and return the version assigned to it. The caller retains
    /// its own `Arc<Tx>` for broadcasting; the history just shares the body
    /// via reference counting.
    pub fn record_tx(&mut self, tx: Arc<Tx>) -> u64 {
        let version = self.next_version;
        self.next_version += 1;
        self.tx_index.insert(tx.hash(), version);
        self.arrivals.insert(version, tx);
        version
    }

    /// Arrivals with version `>= from`, in arrival order. Returns `Arc<Tx>`
    /// so replay callers pay only refcount bumps, not body clones.
    pub fn txs_since(&self, from: u64) -> Vec<Arc<Tx>> {
        self.arrivals
            .range(from..)
            .map(|(_, tx)| Arc::clone(tx))
            .collect()
    }

    /// Cache the tx hashes confirmed by a tracked block so they can be
    /// evicted from the log when the block enters LIB.
    pub fn record_block(&mut self, block_id: HeaderId, tx_hashes: Vec<Tx::Hash>) {
        self.block_txs.insert(block_id, tx_hashes);
    }

    /// Drop the cached entry for a block without touching the log. Use for
    /// stale blocks — their txs may still be pending on surviving forks.
    pub fn forget_block(&mut self, block_id: &HeaderId) {
        self.block_txs.remove(block_id);
    }

    /// Drop a block and evict its txs from the log. Use when the block enters
    /// LIB: its txs are now confirmed on every surviving fork and no longer
    /// belong in the mempool.
    pub fn confirm_block(&mut self, block_id: &HeaderId) {
        if let Some(hashes) = self.block_txs.remove(block_id) {
            for h in hashes {
                self.forget_tx(&h);
            }
        }
    }

    /// Remove a tx from the log (used for force-remove).
    pub fn forget_tx(&mut self, tx_id: &Tx::Hash) -> bool {
        if let Some(version) = self.tx_index.remove(tx_id) {
            self.arrivals.remove(&version);
            return true;
        }
        false
    }
}

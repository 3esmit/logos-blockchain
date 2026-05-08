use std::{collections::HashSet, hash::Hash};

use lb_core::mantle::{DependencyId, TransactionDependencies};
use rpds::{HashTrieMap, HashTrieSet};

pub struct TxTrackerState<Tx, TxId>
where
    TxId: Eq + Hash,
{
    ready_txs: HashTrieMap<TxId, Tx>,
    orphan_txs: HashTrieMap<TxId, Tx>,
    dep_to_tx: HashTrieMap<DependencyId, HashTrieSet<TxId>>,
    tx_pending_count: HashTrieMap<TxId, usize>,
    // TODO: Replace later for the current ledger state
    processed_deps: HashSet<DependencyId>,
}

impl<Tx> TxTrackerState<Tx, Tx::Hash>
where
    Tx: TransactionDependencies + Clone,
{
    pub fn new() -> Self {
        Self {
            ready_txs: HashTrieMap::new(),
            orphan_txs: HashTrieMap::new(),
            dep_to_tx: HashTrieMap::new(),
            tx_pending_count: HashTrieMap::new(),
            processed_deps: HashSet::new(),
        }
    }

    pub fn process_tx(&mut self, tx: Tx) {
        let consumes: HashSet<DependencyId> = tx.consumes().collect();
        let missing_deps: HashSet<DependencyId> =
            consumes.difference(&self.processed_deps).cloned().collect();
        let pending_deps_count = missing_deps.len();
        if missing_deps.is_empty() {
            for dep in missing_deps {
                if let Some(entry) = self.dep_to_tx.get_mut(&dep) {
                    entry.insert_mut(tx.hash());
                } else {
                    let set = HashTrieSet::new().insert(tx.hash());
                    self.dep_to_tx.insert_mut(dep, set);
                }
            }
            self.tx_pending_count
                .insert_mut(tx.hash(), pending_deps_count);
            self.orphan_txs.insert_mut(tx.hash(), tx);
        } else {
            self.ready_txs.insert_mut(tx.hash(), tx);
        }
    }

    pub fn tx_in_block(&mut self, tx_id: &Tx::Hash) {
        if let Some(tx) = pop(&mut self.ready_txs, tx_id) {
            self.processed_deps.extend(tx.produces());
            for dep in tx.produces() {
                let Some(waiting_ids) = self.dep_to_tx.get(&dep) else {
                    continue;
                };
                for waiting_id in waiting_ids {
                    if let Some(pending_count) = self.tx_pending_count.get_mut(waiting_id) {
                        *pending_count -= 1;
                        if *pending_count == 0
                            && let Some(orphan_tx) = pop(&mut self.orphan_txs, waiting_id)
                        {
                            self.ready_txs.insert_mut(waiting_id.clone(), orphan_tx);
                        }
                    }
                }
            }
        }
    }
}

pub fn pop<K, V>(map: &mut HashTrieMap<K, V>, key: &K) -> Option<V>
where
    V: Clone,
    K: Eq + Hash,
{
    map.get(key).cloned().inspect(|_| {
        map.remove_mut(key);
    })
}

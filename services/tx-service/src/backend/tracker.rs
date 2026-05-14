use std::{collections::HashSet, hash::Hash};

use lb_core::mantle::{DependencyId, TransactionDependencies};
use rpds::{HashTrieMap, HashTrieSet};

#[derive(Clone, Debug)]
pub struct TxTrackerState<Tx, TxId>
where
    TxId: Eq + Hash,
{
    ready_txs: HashTrieMap<TxId, Tx>,
    orphan_txs: HashTrieMap<TxId, Tx>,
    dep_to_tx: HashTrieMap<DependencyId, HashTrieSet<TxId>>,
    tx_pending_count: HashTrieMap<TxId, usize>,
    frontier_deps: HashTrieSet<DependencyId>,
}

impl<Tx, TxId> Default for TxTrackerState<Tx, TxId>
where
    TxId: Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Tx, TxId> TxTrackerState<Tx, TxId>
where
    TxId: Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            ready_txs: HashTrieMap::new(),
            orphan_txs: HashTrieMap::new(),
            dep_to_tx: HashTrieMap::new(),
            tx_pending_count: HashTrieMap::new(),
            frontier_deps: HashTrieSet::new(),
        }
    }

    pub fn with_frontier_deps(frontier_deps: impl IntoIterator<Item = DependencyId>) -> Self {
        let default = Self::default();
        Self {
            frontier_deps: HashTrieSet::from_iter(frontier_deps),
            ..default
        }
    }
}

impl<Tx> TxTrackerState<Tx, Tx::Hash>
where
    Tx: TransactionDependencies + Clone,
{
    pub fn process_tx(&mut self, tx: Tx) {
        let consumes: HashSet<DependencyId> = tx.consumes().collect();
        let missing_deps: HashSet<DependencyId> = consumes
            .difference(&HashSet::from_iter(self.frontier_deps.iter().cloned()))
            .cloned()
            .collect();
        let pending_deps_count = missing_deps.len();
        if missing_deps.is_empty() {
            self.ready_txs.insert_mut(tx.hash(), tx);
        } else {
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
        }
    }

    pub fn tx_in_block(&mut self, tx_id: &Tx::Hash) {
        if let Some(tx) = pop(&mut self.ready_txs, tx_id) {
            self.update_frontier_deps(&tx);
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

    fn update_frontier_deps(&mut self, tx: &Tx) {
        for dep_id in tx.produces() {
            self.frontier_deps.insert_mut(dep_id);
        }
        for dep_id in tx.consumes() {
            self.frontier_deps.remove_mut(&dep_id);
        }
    }
}

#[cfg(test)]
impl<Tx, TxId> TxTrackerState<Tx, TxId>
where
    TxId: Eq + Hash + Clone,
{
    pub fn is_ready(&self, id: &TxId) -> bool {
        self.ready_txs.contains_key(id)
    }

    pub fn is_orphan(&self, id: &TxId) -> bool {
        self.orphan_txs.contains_key(id)
    }

    pub fn has_processed_dep(&self, dep: &DependencyId) -> bool {
        self.frontier_deps.contains(dep)
    }

    pub fn ready_count(&self) -> usize {
        self.ready_txs.size()
    }

    pub fn orphan_count(&self) -> usize {
        self.orphan_txs.size()
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use bytes::Bytes;
    use lb_core::mantle::{DependencyId, Transaction, TransactionDependencies, TransactionHasher};

    use super::TxTrackerState;

    // ── mock transaction type ────────────────────────────────────────────────

    #[derive(Clone, Debug)]
    struct TestTx {
        id: &'static str,
        consumes: Vec<&'static str>,
        produces: Vec<&'static str>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct TestTxId(&'static str);

    impl Transaction for TestTx {
        const HASHER: TransactionHasher<Self> = |tx| TestTxId(tx.id);
        type Hash = TestTxId;

        fn as_signing(&self) -> Vec<u8> {
            self.id.as_bytes().to_vec()
        }
    }

    impl TransactionDependencies for TestTx {
        fn consumes(&self) -> impl Iterator<Item = DependencyId> {
            self.consumes
                .iter()
                .map(|s| Bytes::from_static(s.as_bytes()))
        }

        fn produces(&self) -> impl Iterator<Item = DependencyId> {
            self.produces
                .iter()
                .map(|s| Bytes::from_static(s.as_bytes()))
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn tx(id: &'static str, consumes: Vec<&'static str>, produces: Vec<&'static str>) -> TestTx {
        TestTx {
            id,
            consumes,
            produces,
        }
    }

    fn dep(s: &'static str) -> DependencyId {
        Bytes::from_static(s.as_bytes())
    }

    fn ready_names(tracker: &TxTrackerState<TestTx, TestTxId>) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = tracker.ready_txs.keys().map(|k| k.0).collect();
        names.sort_unstable();
        names
    }

    fn orphan_names(tracker: &TxTrackerState<TestTx, TestTxId>) -> HashSet<&'static str> {
        tracker.orphan_txs.keys().map(|k| k.0).collect()
    }

    // ── test ─────────────────────────────────────────────────────────────────
    /// Diamond dependency test
    /// ```
    /// Dependency graph:
    ///   root (pre-existing)
    ///     └── tx_genesis (root → genesis)
    ///           ├── tx_mint_a  (genesis → token_a)
    ///           ├── tx_mint_b  (genesis → token_b, never spent)
    ///           └── tx_fund    (genesis → coin_x, coin_y)
    ///                  ├── tx_chain   (coin_x → coin_z)       ─────────────┐
    ///                  └── tx_combine (token_a + coin_y → nft_1 + coin_w) ─┤
    ///                         └── tx_settle (coin_z + coin_w → coin_final) ┘
    /// ```
    #[test]
    fn test_diamond_dependency_graph() {
        let mut tracker: TxTrackerState<TestTx, TestTxId> = TxTrackerState::new();
        tracker.frontier_deps.insert_mut(dep("root"));

        let tx_genesis = tx("tx_genesis", vec!["root"], vec!["genesis"]);
        let tx_mint_a = tx("tx_mint_a", vec!["genesis"], vec!["token_a"]);
        let tx_mint_b = tx("tx_mint_b", vec!["genesis"], vec!["token_b"]);
        let tx_fund = tx("tx_fund", vec!["genesis"], vec!["coin_x", "coin_y"]);
        // coin_mid is internal to tx_chain; only external dep is coin_x
        let tx_chain = tx("tx_chain", vec!["coin_x"], vec!["coin_z"]);
        let tx_combine = tx(
            "tx_combine",
            vec!["token_a", "coin_y"],
            vec!["nft_1", "coin_w"],
        );
        let tx_settle = tx("tx_settle", vec!["coin_z", "coin_w"], vec!["coin_final"]);

        // Submit in reverse topological order
        for t in [
            tx_settle, tx_combine, tx_chain, tx_mint_b, tx_mint_a, tx_fund, tx_genesis,
        ] {
            tracker.process_tx(t);
        }

        assert_eq!(ready_names(&tracker), vec!["tx_genesis"]);
        assert_eq!(
            orphan_names(&tracker),
            [
                "tx_mint_a",
                "tx_mint_b",
                "tx_fund",
                "tx_chain",
                "tx_combine",
                "tx_settle"
            ]
            .into_iter()
            .collect()
        );

        // tx_genesis confirmed → unlocks tx_mint_a, tx_mint_b, tx_fund
        tracker.tx_in_block(&TestTxId("tx_genesis"));
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_fund", "tx_mint_a", "tx_mint_b"].into_iter().collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            ["tx_chain", "tx_combine", "tx_settle"]
                .into_iter()
                .collect()
        );

        // tx_fund confirmed → unlocks tx_chain (coin_x); tx_combine drops 2→1 (coin_y
        // still missing)
        tracker.tx_in_block(&TestTxId("tx_fund"));
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_chain", "tx_mint_a", "tx_mint_b"].into_iter().collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            ["tx_combine", "tx_settle"].into_iter().collect()
        );

        // tx_mint_a confirmed → tx_combine drops 1→0, promoted to ready
        tracker.tx_in_block(&TestTxId("tx_mint_a"));
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_chain", "tx_combine", "tx_mint_b"]
                .into_iter()
                .collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            std::iter::once("tx_settle").collect()
        );

        // tx_chain confirmed → coin_z satisfied; tx_settle drops 2→1 (coin_w still
        // missing)
        tracker.tx_in_block(&TestTxId("tx_chain"));
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_combine", "tx_mint_b"].into_iter().collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            std::iter::once("tx_settle").collect()
        );

        // tx_combine confirmed → coin_w satisfied; tx_settle drops 1→0, diamond
        // resolved
        tracker.tx_in_block(&TestTxId("tx_combine"));
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_settle", "tx_mint_b"].into_iter().collect()
        );
        assert!(orphan_names(&tracker).is_empty());

        // tx_settle confirmed → only unspent tx_mint_b remains
        tracker.tx_in_block(&TestTxId("tx_settle"));
        assert_eq!(ready_names(&tracker), vec!["tx_mint_b"]);
        assert!(orphan_names(&tracker).is_empty());
    }
}

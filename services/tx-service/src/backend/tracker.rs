use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    sync::Arc,
};

use lb_core::mantle::TxDependencies;
use lb_ledger::LedgerState;
use rpds::HashTrieMapSync as HashTrieMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxTrackerState<TxId>
where
    TxId: Eq + Hash,
{
    pub ready_txs: HashSet<TxId>,
    pub orphan_txs: HashSet<TxId>,
    pub tx_pending_count: HashMap<TxId, usize>,
}

#[derive(Clone, Debug)]
pub struct TxTracker<Tx, TxId>
where
    TxId: Eq + Hash,
{
    ready_txs: HashTrieMap<TxId, Arc<Tx>>,
    orphan_txs: HashTrieMap<TxId, Arc<Tx>>,
    tx_pending_count: HashTrieMap<TxId, usize>,
}

impl<Tx, TxId> Default for TxTracker<Tx, TxId>
where
    TxId: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Tx, TxId> TxTracker<Tx, TxId>
where
    TxId: Eq + Hash + Clone,
{
    pub fn new() -> Self {
        Self {
            ready_txs: HashTrieMap::new_sync(),
            orphan_txs: HashTrieMap::new_sync(),
            tx_pending_count: HashTrieMap::new_sync(),
        }
    }

    pub fn get_txs(&self) -> impl Iterator<Item = &Tx> + '_ {
        self.ready_txs
            .values()
            .chain(self.orphan_txs.values())
            .map(Arc::as_ref)
    }

    pub fn to_state(&self) -> TxTrackerState<TxId> {
        TxTrackerState {
            ready_txs: self.ready_txs.keys().cloned().collect(),
            orphan_txs: self.orphan_txs.keys().cloned().collect(),
            tx_pending_count: self
                .tx_pending_count
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    pub fn from_state_and_txs(
        TxTrackerState {
            ready_txs,
            orphan_txs,
            tx_pending_count,
        }: TxTrackerState<TxId>,
        txs: &HashMap<TxId, Arc<Tx>>,
    ) -> Self {
        let ready_txs = ready_txs
            .into_iter()
            .map(|id| {
                (
                    id.clone(),
                    Arc::clone(txs.get(&id).expect("Tx should be recovered from state")),
                )
            })
            .collect();

        let orphan_txs = orphan_txs
            .into_iter()
            .map(|id| {
                (
                    id.clone(),
                    Arc::clone(txs.get(&id).expect("Tx should be recovered from state")),
                )
            })
            .collect();

        let tx_pending_count = tx_pending_count.into_iter().collect();

        Self {
            ready_txs,
            orphan_txs,
            tx_pending_count,
        }
    }
}

impl<Tx> TxTracker<Tx, Tx::Hash>
where
    Tx: TxDependencies + Clone,
{
    pub fn process_tx(&mut self, tx: Arc<Tx>, frontier_deps: &LedgerState) {
        let mut pending_deps_count = 0;
        let consumes = tx.consumes();
        let ledger_channels = frontier_deps.mantle_ledger().channels();
        for (channel_id, msg_id) in consumes.channels {
            if let Some(state) = ledger_channels.channel_state(&channel_id)
                && msg_id == state.tip_message
            {
                continue;
            }
            pending_deps_count += 1;
        }
        for utxo in consumes.utxos {
            if !frontier_deps.latest_utxos().contains(&utxo) {
                pending_deps_count += 1;
            }
        }
        if pending_deps_count == 0 {
            self.ready_txs.insert_mut(tx.hash(), tx);
        } else {
            self.tx_pending_count
                .insert_mut(tx.hash(), pending_deps_count);
            self.orphan_txs.insert_mut(tx.hash(), tx);
        }
    }

    pub fn tx_in_block(&mut self, tx_id: &Tx::Hash) {
        if let Some(tx) = pop(&mut self.ready_txs, tx_id) {
            let produces = tx.produces();
            let free_channels_deps: HashSet<_> = produces.channels.values().collect();
            let free_utxos_deps: HashSet<_> = produces.utxos.iter().cloned().collect();
            // cheap clone to iterate through items while mutating original self struct if
            // necessary
            for (waiting_id, tx) in self.orphan_txs.clone().iter() {
                let depends = tx.consumes();
                let depends_channels: HashSet<_> = depends.channels.values().collect();
                let depends_utxos: HashSet<_> = depends.utxos.iter().cloned().collect();
                let free = {
                    depends_channels.difference(&free_channels_deps).count()
                        + depends_utxos.difference(&free_utxos_deps).count()
                };
                if let Some(pending_count) = self.tx_pending_count.get_mut(waiting_id) {
                    *pending_count -= free;
                    if *pending_count == 0
                        && let Some(orphan_tx) = pop(&mut self.orphan_txs, waiting_id)
                    {
                        self.ready_txs.insert_mut(waiting_id.clone(), orphan_tx);
                    }
                }
            }
        }
    }

    pub fn get_ready_txs(&self) -> Vec<Tx> {
        self.ready_txs.values().map(|tx| Tx::clone(tx)).collect()
    }

    pub fn force_remove_tx(&mut self, id: &Tx::Hash) -> bool {
        let will_remove = [
            self.ready_txs.contains_key(id),
            self.orphan_txs.contains_key(id),
            self.tx_pending_count.contains_key(id),
        ]
        .iter()
        .any(|b| *b);

        self.ready_txs.remove_mut(id);
        self.orphan_txs.remove_mut(id);
        self.tx_pending_count.remove_mut(id);
        // TODO: Remove smartly dependencies that requires of this tx
        will_remove
    }
}

#[cfg(test)]
impl<Tx, TxId> TxTracker<Tx, TxId>
where
    TxId: Eq + Hash + Clone,
{
    pub fn is_ready(&self, id: &TxId) -> bool {
        self.ready_txs.contains_key(id)
    }

    pub fn is_orphan(&self, id: &TxId) -> bool {
        self.orphan_txs.contains_key(id)
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
    use std::{collections::HashSet, sync::Arc};

    use bytes::Bytes;
    use lb_core::mantle::{Transaction, TransactionHasher, TxDependencies};

    use super::TxTracker;

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

    impl TxDependencies for TestTx {
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

    fn ready_names(tracker: &TxTracker<TestTx, TestTxId>) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = tracker.ready_txs.keys().map(|k| k.0).collect();
        names.sort_unstable();
        names
    }

    fn orphan_names(tracker: &TxTracker<TestTx, TestTxId>) -> HashSet<&'static str> {
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
    #[expect(
        clippy::too_many_lines,
        reason = "comprehensive integration test for dependency graph"
    )]
    #[test]
    fn test_diamond_dependency_graph() {
        let mut tracker: TxTracker<TestTx, TestTxId> = TxTracker::new();
        let mut frontier: HashSet<DependencyId> = HashSet::new();
        frontier.insert(dep("root"));

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
            tx_settle,
            tx_combine.clone(),
            tx_chain.clone(),
            tx_mint_b,
            tx_mint_a.clone(),
            tx_fund.clone(),
            tx_genesis.clone(),
        ] {
            tracker.process_tx(Arc::new(t), &frontier);
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
        // frontier gains "genesis" (produced by tx_genesis)
        tracker.tx_in_block(&TestTxId("tx_genesis"));
        for dep_id in tx_genesis.produces() {
            frontier.insert(dep_id);
        }
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
        // still missing); frontier gains "coin_x", "coin_y"
        tracker.tx_in_block(&TestTxId("tx_fund"));
        for dep_id in tx_fund.produces() {
            frontier.insert(dep_id);
        }
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_chain", "tx_mint_a", "tx_mint_b"].into_iter().collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            ["tx_combine", "tx_settle"].into_iter().collect()
        );

        // tx_mint_a confirmed → tx_combine drops 1→0, promoted to ready
        // frontier gains "token_a"
        tracker.tx_in_block(&TestTxId("tx_mint_a"));
        for dep_id in tx_mint_a.produces() {
            frontier.insert(dep_id);
        }
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
        // missing); frontier gains "coin_z"
        tracker.tx_in_block(&TestTxId("tx_chain"));
        for dep_id in tx_chain.produces() {
            frontier.insert(dep_id);
        }
        assert_eq!(
            ready_names(&tracker).into_iter().collect::<HashSet<_>>(),
            ["tx_combine", "tx_mint_b"].into_iter().collect()
        );
        assert_eq!(
            orphan_names(&tracker),
            std::iter::once("tx_settle").collect()
        );

        // tx_combine confirmed → coin_w satisfied; tx_settle drops 1→0, diamond
        // resolved; frontier gains "nft_1", "coin_w"
        tracker.tx_in_block(&TestTxId("tx_combine"));
        for dep_id in tx_combine.produces() {
            frontier.insert(dep_id);
        }
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

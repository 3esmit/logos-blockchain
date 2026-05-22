use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    pin::pin,
};

use futures::StreamExt;
use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo, api::ApiError};
use lb_core::{
    header::HeaderId,
    mantle::{
        DependencyId, GenesisTx, SignedMantleTx, Transaction, TxDependencies, TxRewardsRatio,
        gas::MainnetGasConstants,
    },
};
use lb_ledger::LedgerState;
use tracing::error;

use super::{history::TxHistory, tracker::TxTrackerState};

pub struct BlockInfo<Tx> {
    pub parent: HeaderId,
    pub transactions: Vec<Tx>,
}

#[async_trait::async_trait]
pub trait BlockInfoGetter<Tx> {
    async fn get_block(&self, header_id: &HeaderId) -> Result<BlockInfo<Tx>, ForksTrackerError>;
}

#[async_trait::async_trait]
pub trait LedgerStateGetter {
    async fn get_ledger_state(&self, header_id: HeaderId)
    -> Result<LedgerState, ForksTrackerError>;
    async fn get_ledger_deps(
        &self,
        header_id: &HeaderId,
    ) -> Result<HashSet<DependencyId>, ForksTrackerError>;
}

#[derive(thiserror::Error, Debug)]
pub enum ForksTrackerError {
    #[error("Block not found in block store")]
    BlockNotFound,
    #[error("Parent {0} not found in current_tips or states")]
    ParentNotFound(HeaderId),
    #[error("Ledger state not found for {0:?}")]
    LedgerStateNotFound(HeaderId),
    #[error(transparent)]
    CryptarchiaApi(#[from] ApiError),
}

struct BlockState<Tx, TxId>
where
    TxId: Eq + Hash,
{
    state: TxTrackerState<Tx, TxId>,
    version: u64,
}

pub struct ForksTracker<Tx, TxId, Adapter>
where
    TxId: Eq + Hash,
{
    block_states: HashMap<HeaderId, BlockState<Tx, TxId>>,
    tips: HashSet<HeaderId>,
    mempool_log: TxHistory<Tx, TxId>,
    adapter: Adapter,
}

impl<Tx, Adapter> ForksTracker<Tx, Tx::Hash, Adapter>
where
    Tx: TxDependencies + Clone,
    Adapter: BlockInfoGetter<Tx> + LedgerStateGetter + Clone + Send,
{
    pub fn new(adapter: Adapter) -> Self {
        Self {
            block_states: HashMap::new(),
            tips: HashSet::new(),
            mempool_log: TxHistory::new(),
            adapter,
        }
    }

    pub async fn get_frontier_txs(
        &self,
        parent_hint: HeaderId,
    ) -> Result<Vec<Tx>, ForksTrackerError>
    where
        Tx: TxRewardsRatio,
    {
        if !self.tips.contains(&parent_hint) {
            return Err(ForksTrackerError::ParentNotFound(parent_hint));
        }
        let mut txs: Vec<Tx> = self
            .block_states
            .get(&parent_hint)
            .map(|fork| fork.state.get_ready_txs())
            .ok_or_else(|| ForksTrackerError::ParentNotFound(parent_hint))?;
        let ledger_state: LedgerState = self.adapter.get_ledger_state(parent_hint).await?;
        let utxos = &ledger_state.epoch_state().utxos;
        let gas_prices = ledger_state.get_gas_prices();
        let cached_keys: HashMap<_, _> = txs
            .iter()
            .filter_map(|tx| {
                match TxRewardsRatio::rewards_ratio::<MainnetGasConstants>(tx, &gas_prices, utxos) {
                    Ok(ratio) => Some((tx.hash(), ratio)),
                    Err(e) => {
                        error!(
                            "Error computing rewards ratio for tx {:?}: {e:?}",
                            tx.hash()
                        );
                        None
                    }
                }
            })
            .collect();
        txs.sort_unstable_by_key(|tx| cached_keys[&tx.hash()]);
        Ok(txs)
    }

    pub fn force_remove_txs(&mut self, txs: &[Tx::Hash]) {
        for fork in self.block_states.values_mut() {
            for tx in txs {
                fork.state.force_remove_tx(tx);
            }
        }
        for tx in txs {
            self.mempool_log.forget_tx(tx);
        }
    }

    pub fn process_lib(&mut self, event: &LibUpdate) {
        let LibUpdate {
            pruned_blocks:
                PrunedBlocksInfo {
                    stale_blocks,
                    immutable_blocks,
                },
            ..
        } = event;

        for block in stale_blocks.iter().chain(immutable_blocks.values()) {
            self.block_states.remove(block);
            self.tips.remove(block);
            self.mempool_log.confirm_block(block);
        }
    }

    pub async fn process_new_block(
        &mut self,
        event: &ProcessedBlockEvent,
    ) -> Result<(), ForksTrackerError> {
        let ProcessedBlockEvent { block_id, .. } = event;
        let BlockInfo::<Tx> {
            parent,
            transactions,
        } = self.adapter.get_block(block_id).await?;

        let parent_fork = self
            .block_states
            .get(&parent)
            .ok_or(ForksTrackerError::ParentNotFound(parent))?;
        let mut block_state: TxTrackerState<_, _> = parent_fork.state.clone();
        let parent_version = parent_fork.version;

        // Stale-ancestor catch-up: when the parent's snapshot predates some
        // mempool arrivals, replay the missing suffix against the parent's
        // frontier deps so the new fork inherits the live mempool view rather
        // than the frozen one. Current-tip parents already have
        // parent_version == log.version(), so this is a no-op in the common
        // chain-extension case.
        if parent_version < self.mempool_log.version() {
            let replay_txs = self.mempool_log.txs_since(parent_version);
            if !replay_txs.is_empty() {
                let deps = self.adapter.get_ledger_deps(&parent).await?;
                for tx in replay_txs {
                    block_state.process_tx(tx, &deps);
                }
            }
        }

        let mut block_tx_hashes = Vec::with_capacity(transactions.len());
        for tx in transactions {
            let h = tx.hash();
            block_state.tx_in_block(&h);
            block_tx_hashes.push(h);
        }
        self.mempool_log.record_block(*block_id, block_tx_hashes);

        self.insert_new_tip(block_id, &parent, block_state);
        Ok(())
    }

    fn insert_new_tip(
        &mut self,
        block_id: &HeaderId,
        parent: &HeaderId,
        block_state: TxTrackerState<Tx, Tx::Hash>,
    ) {
        // Demote the parent off the frontier and promote the new block.
        // tips.remove is a no-op if a sibling already demoted the parent.
        self.tips.remove(&parent);
        self.block_states.insert(
            *block_id,
            BlockState {
                state: block_state,
                version: self.mempool_log.version(),
            },
        );
        self.tips.insert(*block_id);
    }

    pub async fn process_new_tx(&mut self, tx: &Tx) {
        // Record the arrival in the versioned log first so any fork that
        // emerges later can replay it onto its stale ancestor snapshot.
        self.mempool_log.record_tx(tx);
        let new_version = self.mempool_log.version();

        if self.tips.is_empty() {
            return;
        }
        let tips_len = self.tips.len();
        let tip_ids: Vec<HeaderId> = self.tips.iter().copied().collect();
        let ledger_getter: Adapter = self.adapter.clone();
        let mut ledger_states = pin!(
            tokio_stream::iter(
                tip_ids
                    .into_iter()
                    .zip(std::iter::repeat_with(|| ledger_getter.clone()))
            )
            .map(async |(header_id, ledger_getter)| {
                let ledger_state = ledger_getter.get_ledger_deps(&header_id).await;
                (header_id, ledger_state)
            })
            .buffer_unordered(tips_len)
        );
        while let Some((header_id, ledger_state)) = ledger_states.next().await {
            let fork = self
                .block_states
                .get_mut(&header_id)
                .expect("This header at this point is always present");
            match ledger_state {
                Ok(ledger_state_deps) => {
                    fork.state.process_tx(tx.clone(), &ledger_state_deps);
                }
                Err(e) => {
                    error!("Error getting ledger state for block {header_id}: {e:?}");
                }
            }
            fork.version = new_version;
        }
    }

    pub fn get_block_state(&self, header_id: &HeaderId) -> Option<TxTrackerState<Tx, Tx::Hash>> {
        self.block_states
            .get(header_id)
            .map(|fork| fork.state.clone())
    }
}

#[cfg(test)]
impl<Tx, Adapter> ForksTracker<Tx, Tx::Hash, Adapter>
where
    Tx: TxDependencies + Clone,
    Adapter: BlockInfoGetter<Tx> + LedgerStateGetter + Clone + Send,
{
    fn is_tip(&self, id: &HeaderId) -> bool {
        self.tips.contains(id)
    }

    fn is_historical(&self, id: &HeaderId) -> bool {
        self.block_states.contains_key(id) && !self.tips.contains(id)
    }

    fn tip_count(&self) -> usize {
        self.tips.len()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo, Slot};
    use lb_core::{
        header::HeaderId,
        mantle::{DependencyId, Transaction, TransactionHasher, TxDependencies},
    };
    use lb_ledger::LedgerState;

    use super::{
        BlockInfo, BlockInfoGetter, BlockState, ForksTracker, ForksTrackerError, LedgerStateGetter,
    };
    use crate::backend::tracker::TxTrackerState;

    // ── mock transaction ─────────────────────────────────────────────────────

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

    // ── mock adapter ─────────────────────────────────────────────────────────

    /// Adapter backed by a shared block store. Cloning shares the same store,
    /// so blocks registered on the original are visible through the clone held
    /// inside `ForksTracker`.
    #[derive(Clone, Default)]
    struct MockAdapter {
        blocks: Arc<Mutex<HashMap<HeaderId, (HeaderId, Vec<TestTx>)>>>,
    }

    impl MockAdapter {
        fn new() -> Self {
            Self::default()
        }

        fn add_block(&self, block_id: HeaderId, parent: HeaderId, txs: Vec<TestTx>) {
            self.blocks.lock().unwrap().insert(block_id, (parent, txs));
        }
    }

    #[async_trait]
    impl BlockInfoGetter<TestTx> for MockAdapter {
        async fn get_block(&self, id: &HeaderId) -> Result<BlockInfo<TestTx>, ForksTrackerError> {
            let (parent, transactions) = self
                .blocks
                .lock()
                .unwrap()
                .remove(id)
                .ok_or(ForksTrackerError::BlockNotFound)?;
            Ok(BlockInfo {
                parent,
                transactions,
            })
        }
    }

    #[async_trait]
    impl LedgerStateGetter for MockAdapter {
        async fn get_ledger_state(
            &self,
            header_id: HeaderId,
        ) -> Result<LedgerState, ForksTrackerError> {
            Err(ForksTrackerError::LedgerStateNotFound(header_id))
        }

        async fn get_ledger_deps(
            &self,
            _: &HeaderId,
        ) -> Result<HashSet<DependencyId>, ForksTrackerError> {
            Ok(HashSet::new())
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn id(n: u8) -> HeaderId {
        HeaderId::from([n; 32])
    }

    fn tx(name: &'static str, consumes: Vec<&'static str>, produces: Vec<&'static str>) -> TestTx {
        TestTx {
            id: name,
            consumes,
            produces,
        }
    }

    fn processed_event(block_id: HeaderId) -> ProcessedBlockEvent {
        ProcessedBlockEvent {
            block_id,
            tip: block_id,
            tip_slot: Slot::from(0u64),
            lib: block_id,
            lib_slot: Slot::from(0u64),
        }
    }

    // All pruned blocks go into stale_blocks to avoid needing Slot as a direct dep.
    fn lib_event(stale: Vec<HeaderId>) -> LibUpdate {
        LibUpdate {
            new_lib: stale.last().copied().unwrap_or_else(|| id(0)),
            pruned_blocks: PrunedBlocksInfo {
                stale_blocks: stale,
                immutable_blocks: BTreeMap::default(),
            },
        }
    }

    /// Seed the tracker with an initial genesis tip so all tests start from a
    /// known frontier entry.
    async fn seed_genesis(
        tracker: &mut ForksTracker<TestTx, TestTxId, MockAdapter>,
        genesis: HeaderId,
    ) {
        let root = id(255);
        let version = tracker.mempool_log.version();
        tracker.block_states.insert(
            root,
            BlockState {
                state: TxTrackerState::new(),
                version,
            },
        );
        tracker.tips.insert(root);
        tracker.adapter.add_block(genesis, root, vec![]);
        tracker
            .process_new_block(&processed_event(genesis))
            .await
            .unwrap();
    }

    /// Apply `tx` to all current tips via the async API.
    async fn broadcast_tx(tracker: &mut ForksTracker<TestTx, TestTxId, MockAdapter>, t: &TestTx) {
        tracker.process_new_tx(t).await;
    }

    // ── tests ────────────────────────────────────────────────────────────────

    /// A single chain genesis → A → B → C: the frontier always holds exactly
    /// one tip and historical states accumulate in `states`.
    #[tokio::test]
    async fn test_linear_chain_tip_tracking() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;

        assert_eq!(tracker.tip_count(), 1);
        assert!(tracker.is_tip(&genesis));

        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();
        assert_eq!(tracker.tip_count(), 1);
        assert!(tracker.is_tip(&a));
        assert!(tracker.is_historical(&genesis));

        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        assert!(tracker.is_tip(&b));
        assert!(tracker.is_historical(&a));

        tracker.adapter.add_block(c, b, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();
        assert!(tracker.is_tip(&c));
        assert!(tracker.is_historical(&b));

        assert!(tracker.get_block_state(&genesis).is_some());
        assert!(tracker.get_block_state(&a).is_some());
        assert!(tracker.get_block_state(&b).is_some());
        assert!(tracker.get_block_state(&c).is_some());
        assert!(tracker.get_block_state(&id(99)).is_none());
    }

    /// Mempool txs submitted while two fork tips exist must appear in both.
    #[tokio::test]
    async fn test_mempool_tx_propagates_to_all_tips() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();
        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        tracker.adapter.add_block(c, a, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();

        assert_eq!(tracker.tip_count(), 2);

        broadcast_tx(&mut tracker, &tx("mempool_tx", vec![], vec!["dep_x"])).await;

        for tip in &tracker.tips {
            let fork = tracker.block_states.get(tip).expect("tip is tracked");
            assert!(fork.state.is_ready(&TestTxId("mempool_tx")));
        }
    }

    /// Txs confirmed in block B are removed from the mempool view on fork B
    /// while remaining pending on fork C, and vice versa. Fork states are fully
    /// independent.
    #[tokio::test]
    async fn test_fork_states_are_independent() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // fork 1: A → B (confirms tx_b)
        let c = id(3); // fork 2: A → C (confirms tx_c)

        let tx_b = tx("tx_b", vec![], vec!["out_b"]);
        let tx_c = tx("tx_c", vec![], vec!["out_c"]);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;
        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();

        // Both txs arrive in the mempool before either block is processed.
        broadcast_tx(&mut tracker, &tx_b).await;
        broadcast_tx(&mut tracker, &tx_c).await;

        tracker.adapter.add_block(b, a, vec![tx_b.clone()]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        tracker.adapter.add_block(c, a, vec![tx_c.clone()]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();

        let state_b = tracker.get_block_state(&b).unwrap();
        let state_c = tracker.get_block_state(&c).unwrap();

        // Fork B: tx_b is confirmed (removed from ready, not orphan).
        //         tx_c is still pending (ready) — in the mempool but not in B.
        assert!(!state_b.is_ready(&TestTxId("tx_b")));
        assert!(!state_b.is_orphan(&TestTxId("tx_b")));
        assert!(state_b.is_ready(&TestTxId("tx_c")));

        // Fork C: tx_c is confirmed; tx_b is still pending (ready).
        assert!(!state_c.is_ready(&TestTxId("tx_c")));
        assert!(!state_c.is_orphan(&TestTxId("tx_c")));
        assert!(state_c.is_ready(&TestTxId("tx_b")));
    }

    /// An orphan tx gets resolved on the fork that confirms its dependency
    /// producer, but stays orphaned on the fork where the producer remains
    /// unconfirmed. This is the key fork-isolation property.
    #[tokio::test]
    async fn test_mempool_orphan_resolved_differently_per_fork() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // confirms tx_producer → promotes tx_consumer
        let c = id(3); // does NOT confirm tx_producer

        let tx_producer = tx("tx_producer", vec![], vec!["X"]);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;
        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();

        // Both txs arrive in the mempool; tx_consumer is orphaned until "X" is
        // produced.
        broadcast_tx(&mut tracker, &tx("tx_consumer", vec!["X"], vec!["Y"])).await;
        broadcast_tx(&mut tracker, &tx_producer).await;

        tracker.adapter.add_block(b, a, vec![tx_producer.clone()]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        tracker.adapter.add_block(c, a, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();

        let state_b = tracker.get_block_state(&b).unwrap();
        let state_c = tracker.get_block_state(&c).unwrap();

        // Fork B: tx_producer confirmed → dep "X" produced → tx_consumer promoted.
        assert!(!state_b.is_ready(&TestTxId("tx_producer")));
        assert!(!state_b.is_orphan(&TestTxId("tx_producer")));
        assert!(state_b.is_ready(&TestTxId("tx_consumer")));

        // Fork C: tx_producer still pending (ready); tx_consumer still orphaned.
        assert!(state_c.is_ready(&TestTxId("tx_producer")));
        assert!(state_c.is_orphan(&TestTxId("tx_consumer")));
    }

    /// Regression test for the stale-mempool re-org bug.
    ///
    /// Scenario:
    ///     genesis → A → B
    ///                ↘ C   (sibling fork branching from A)
    ///
    /// Timeline:
    ///   1. A is processed (becomes the current tip).
    ///   2. Block B (parent A) is processed → A is demoted into `states`.
    ///   3. A mempool tx arrives while A sits in `states`. `process_new_tx`
    ///      only updates `current_tips`, so B receives it but A's snapshot does
    ///      not.
    ///   4. Block C (parent A, sibling of B) is processed. C clones A's
    ///      snapshot, which never saw the mempool tx.
    ///
    /// Expected: the mempool tx must be ready on C as well — it is still a
    /// valid pending tx and the fork it lands on is an implementation
    /// detail. The fork tracker must keep historical states current with the
    /// mempool so newly-emerging branches inherit them.
    #[tokio::test]
    async fn test_mempool_tx_visible_on_fork_branching_from_historical_parent() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();

        // B becomes the tip; A is demoted into `states`.
        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        assert!(tracker.is_historical(&a));
        assert!(tracker.is_tip(&b));

        // Mempool tx arrives while A is no longer on the frontier.
        let late_tx = tx("late_tx", vec![], vec!["late_dep"]);
        broadcast_tx(&mut tracker, &late_tx).await;

        // C branches off A — it clones A's (currently stale) snapshot.
        tracker.adapter.add_block(c, a, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();

        let state_b = tracker.get_block_state(&b).unwrap();
        let state_c = tracker.get_block_state(&c).unwrap();

        // B picked it up directly via process_new_tx.
        assert!(state_b.is_ready(&TestTxId("late_tx")));
        // C must also see it — it's still a valid pending mempool tx.
        assert!(state_c.is_ready(&TestTxId("late_tx")));
    }

    /// LIB update removes pruned block ids from both `states` and
    /// `current_tips`.
    #[tokio::test]
    async fn test_lib_prunes_stale_and_immutable_blocks() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;
        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();
        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        tracker.adapter.add_block(c, b, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();

        // genesis and A are now pruned
        tracker.process_lib(&lib_event(vec![genesis, a]));

        assert!(!tracker.is_historical(&genesis));
        assert!(!tracker.is_tip(&genesis));
        assert!(!tracker.is_historical(&a));
        assert!(!tracker.is_tip(&a));

        assert!(tracker.is_historical(&b));
        assert!(tracker.is_tip(&c));
    }

    /// LIB update with a stale fork tip removes that tip from `current_tips`
    /// while the canonical tip is unaffected.
    #[tokio::test]
    async fn test_lib_prunes_stale_fork_tip() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // canonical tip
        let d = id(4); // stale fork tip

        let adapter = MockAdapter::new();
        let mut tracker = ForksTracker::new(adapter);
        seed_genesis(&mut tracker, genesis).await;
        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();
        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        tracker.adapter.add_block(d, a, vec![]);
        tracker
            .process_new_block(&processed_event(d))
            .await
            .unwrap();

        assert_eq!(tracker.tip_count(), 2);

        tracker.process_lib(&lib_event(vec![d]));

        assert!(!tracker.is_tip(&d));
        assert!(tracker.is_tip(&b));
    }

    /// Processing a block whose parent is not known returns `ParentNotFound`.
    #[tokio::test]
    async fn test_process_block_unknown_parent_returns_error() {
        let adapter = MockAdapter::new();
        // Register the block but with an unknown parent so the lookup succeeds
        // but the parent state is missing.
        adapter.add_block(id(77), id(50), vec![]);
        let mut tracker: ForksTracker<TestTx, TestTxId, MockAdapter> = ForksTracker::new(adapter);
        let result = tracker.process_new_block(&processed_event(id(77))).await;
        assert!(matches!(result, Err(ForksTrackerError::ParentNotFound(_))));
    }

    /// When `BlockGetter` cannot find the block the error propagates unchanged.
    #[tokio::test]
    async fn test_block_getter_failure_propagates() {
        let unknown = id(99);
        let getter = MockAdapter::new(); // empty — will return BlockNotFound

        let mut tracker = ForksTracker::new(MockAdapter::new());

        let result = tracker.process_new_block(&processed_event(unknown)).await;
        assert!(matches!(result, Err(ForksTrackerError::BlockNotFound)));
    }
}

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    pin::pin,
};

use futures::StreamExt;
use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo};
use lb_core::{
    header::HeaderId,
    mantle::{DependencyId, TransactionDependencies},
};
use tracing::error;

use super::tracker::TxTrackerState;

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
    async fn get_ledger_deps(
        &self,
        header_id: &HeaderId,
    ) -> Result<HashSet<DependencyId>, ForksTrackerError>;
}

#[derive(Debug)]
pub enum ForksTrackerError {
    BlockNotFound,
    ParentNotFound(HeaderId),
}

pub struct ForksTracker<Tx, TxId, Adapter>
where
    TxId: Eq + Hash,
{
    states: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    current_tips: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    adapter: Adapter,
}

impl<Tx, Adapter> ForksTracker<Tx, Tx::Hash, Adapter>
where
    Tx: TransactionDependencies + Clone,
    Adapter: BlockInfoGetter<Tx> + LedgerStateGetter + Clone + Send,
{
    pub fn new(adapter: Adapter) -> Self {
        Self {
            states: HashMap::new(),
            current_tips: HashMap::new(),
            adapter,
        }
    }

    pub fn process_lib(&mut self, event: &LibUpdate) {
        let LibUpdate {
            new_lib,
            pruned_blocks:
                PrunedBlocksInfo {
                    stale_blocks,
                    immutable_blocks,
                },
        } = event;

        for block in stale_blocks.iter().chain(immutable_blocks.values()) {
            // remove lib state
            drop(self.states.remove(block));
            drop(self.current_tips.remove(block));
        }
    }

    pub async fn process_new_block(
        &mut self,
        event: &ProcessedBlockEvent,
    ) -> Result<(), ForksTrackerError> {
        let ProcessedBlockEvent { block_id, tip, .. } = event;
        let BlockInfo::<Tx> {
            parent,
            transactions,
        } = self.adapter.get_block(block_id).await?;
        // Check current_tips first, then states: a fork sibling may have already
        // moved the shared parent out of current_tips into states.
        let parent_state = self
            .current_tips
            .get(&parent)
            .or_else(|| self.states.get(&parent))
            .ok_or(ForksTrackerError::ParentNotFound(parent))?;
        let mut block_state: TxTrackerState<_, _> = parent_state.clone();
        for tx in transactions {
            block_state.tx_in_block(&tx.hash());
        }
        // Move parent from tip frontier to historical states, preserving its own
        // accumulated state (not the child block's). No-op if it was already
        // moved by a sibling block.
        if let Some(tip_state) = self.current_tips.remove(&parent) {
            self.states.insert(parent, tip_state);
        }
        self.current_tips.insert(*block_id, block_state);
        Ok(())
    }

    pub async fn process_new_tx(&mut self, tx: &Tx) {
        let Self { current_tips, .. } = self;
        let tips_len = current_tips.len();
        let ledger_getter: Adapter = self.adapter.clone();
        let header_ids: Vec<_> = current_tips.keys().cloned().collect();
        let mut ledger_states = pin!(
            tokio_stream::iter(
                header_ids
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
            let state = current_tips
                .get_mut(&header_id)
                .expect("This header at this point is always present");
            match ledger_state {
                Ok(ledger_state_deps) => {
                    state.process_tx(tx.clone(), &ledger_state_deps);
                }
                Err(e) => {
                    error!(
                        "Error getting ledger state for block {header_id}:
        {e:?}"
                    );
                }
            }
        }
    }

    pub fn get_block_state(&self, header_id: &HeaderId) -> Option<TxTrackerState<Tx, Tx::Hash>> {
        self.states
            .get(header_id)
            .cloned()
            .or_else(|| self.current_tips.get(header_id).cloned())
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
        mantle::{DependencyId, Transaction, TransactionDependencies, TransactionHasher},
    };

    use super::{BlockInfo, BlockInfoGetter, ForksTracker, ForksTrackerError, LedgerStateGetter};
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
        tracker.current_tips.insert(root, TxTrackerState::new());
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

        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&genesis));

        tracker.adapter.add_block(a, genesis, vec![]);
        tracker
            .process_new_block(&processed_event(a))
            .await
            .unwrap();
        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&a));
        assert!(tracker.states.contains_key(&genesis));

        tracker.adapter.add_block(b, a, vec![]);
        tracker
            .process_new_block(&processed_event(b))
            .await
            .unwrap();
        assert!(tracker.current_tips.contains_key(&b));
        assert!(tracker.states.contains_key(&a));

        tracker.adapter.add_block(c, b, vec![]);
        tracker
            .process_new_block(&processed_event(c))
            .await
            .unwrap();
        assert!(tracker.current_tips.contains_key(&c));
        assert!(tracker.states.contains_key(&b));

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

        assert_eq!(tracker.current_tips.len(), 2);

        broadcast_tx(&mut tracker, &tx("mempool_tx", vec![], vec!["dep_x"])).await;

        for state in tracker.current_tips.values() {
            assert!(state.is_ready(&TestTxId("mempool_tx")));
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

        assert!(!tracker.states.contains_key(&genesis));
        assert!(!tracker.current_tips.contains_key(&genesis));
        assert!(!tracker.states.contains_key(&a));
        assert!(!tracker.current_tips.contains_key(&a));

        assert!(tracker.states.contains_key(&b));
        assert!(tracker.current_tips.contains_key(&c));
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

        assert_eq!(tracker.current_tips.len(), 2);

        tracker.process_lib(&lib_event(vec![d]));

        assert!(!tracker.current_tips.contains_key(&d));
        assert!(tracker.current_tips.contains_key(&b));
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

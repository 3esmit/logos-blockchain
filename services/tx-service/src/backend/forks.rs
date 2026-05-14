use std::{collections::HashMap, hash::Hash};

use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo};
use lb_core::{header::HeaderId, mantle::TransactionDependencies};

use super::tracker::TxTrackerState;

pub struct BlockInfo<Tx> {
    pub parent: HeaderId,
    pub transactions: Vec<Tx>,
}

#[async_trait::async_trait]
pub trait BlockInfoGetter<Tx> {
    async fn get_block(&self, header_id: &HeaderId) -> Result<BlockInfo<Tx>, ForksTrackerError>;
}

#[derive(Debug)]
pub enum ForksTrackerError {
    BlockNotFound,
    ParentNotFound(HeaderId),
}
pub struct ForksTracker<Tx, TxId, BlockGetter>
where
    TxId: Eq + Hash,
{
    states: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    current_tips: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    block_getter: BlockGetter,
}

impl<Tx, Getter> ForksTracker<Tx, Tx::Hash, Getter>
where
    Tx: TransactionDependencies + Clone,
    Getter: BlockInfoGetter<Tx> + Send,
{
    pub fn new(block_getter: Getter) -> Self {
        Self {
            states: HashMap::new(),
            current_tips: HashMap::new(),
            block_getter,
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
        } = self.block_getter.get_block(block_id).await?;
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

    pub fn process_new_tx(&mut self, tx: &Tx) {
        for state in self.current_tips.values_mut() {
            state.process_tx(tx.clone());
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
    use std::collections::{BTreeMap, HashMap};

    use async_trait::async_trait;
    use bytes::Bytes;
    use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo};
    use lb_core::{
        header::HeaderId,
        mantle::{DependencyId, Transaction, TransactionDependencies, TransactionHasher},
    };

    use super::{BlockInfo, BlockInfoGetter, ForksTracker, ForksTrackerError};
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

    // ── mock block getter ────────────────────────────────────────────────────

    struct MockGetter(HashMap<HeaderId, (HeaderId, Vec<TestTx>)>);

    impl MockGetter {
        fn new() -> Self {
            Self(HashMap::new())
        }

        fn insert(&mut self, id: HeaderId, parent: HeaderId, txs: Vec<TestTx>) {
            self.0.insert(id, (parent, txs));
        }
    }

    #[async_trait]
    impl BlockInfoGetter<TestTx> for MockGetter {
        async fn get_block(
            &self,
            header_id: &HeaderId,
        ) -> Result<BlockInfo<TestTx>, ForksTrackerError> {
            self.0
                .get(header_id)
                .map(|(parent, txs)| BlockInfo {
                    parent: *parent,
                    transactions: txs.clone(),
                })
                .ok_or(ForksTrackerError::BlockNotFound)
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

    // Slot is not a direct dep; use Into<Slot> inference from struct field types.
    fn block_event(block_id: HeaderId, tip: HeaderId) -> ProcessedBlockEvent {
        ProcessedBlockEvent {
            block_id,
            tip,
            tip_slot: 0u64.into(),
            lib: id(0),
            lib_slot: 0u64.into(),
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

    /// Seed the tracker with an initial genesis tip so all tests start with a
    /// known frontier entry.
    async fn seed_genesis(
        tracker: &mut ForksTracker<TestTx, TestTxId, MockGetter>,
        genesis: HeaderId,
    ) {
        let root = id(255);
        tracker.block_getter.insert(genesis, root, vec![]);
        tracker.current_tips.insert(root, TxTrackerState::new());
        tracker
            .process_new_block(&block_event(genesis, genesis))
            .await
            .unwrap();
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

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![]);
        getter.insert(c, b, vec![]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;

        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&genesis));

        tracker.process_new_block(&block_event(a, a)).await.unwrap();
        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&a));
        assert!(tracker.states.contains_key(&genesis));

        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        assert!(tracker.current_tips.contains_key(&b));
        assert!(tracker.states.contains_key(&a));

        tracker.process_new_block(&block_event(c, c)).await.unwrap();
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

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![]);
        getter.insert(c, a, vec![]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.process_new_block(&block_event(a, a)).await.unwrap();
        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        tracker.process_new_block(&block_event(c, c)).await.unwrap();

        assert_eq!(tracker.current_tips.len(), 2);

        tracker.process_new_tx(&tx("mempool_tx", vec![], vec!["dep_x"]));

        for state in tracker.current_tips.values() {
            assert!(state.is_ready(&TestTxId("mempool_tx")));
        }
    }

    /// Txs confirmed in block B are removed from the mempool view on fork B
    /// (`processed_deps` updated) while remaining pending on fork C; and vice
    /// versa for txs confirmed in C. Fork states are fully independent.
    #[tokio::test]
    async fn test_fork_states_are_independent() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // fork 1: A → B (confirms tx_b)
        let c = id(3); // fork 2: A → C (confirms tx_c)

        let tx_b = tx("tx_b", vec![], vec!["out_b"]);
        let tx_c = tx("tx_c", vec![], vec!["out_c"]);

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![tx_b.clone()]);
        getter.insert(c, a, vec![tx_c.clone()]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.process_new_block(&block_event(a, a)).await.unwrap();

        // Both txs arrive in the mempool before either block is processed.
        tracker.process_new_tx(&tx_b);
        tracker.process_new_tx(&tx_c);

        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        tracker.process_new_block(&block_event(c, c)).await.unwrap();

        let state_b = tracker.get_block_state(&b).unwrap();
        let state_c = tracker.get_block_state(&c).unwrap();

        // Fork B: tx_b is confirmed (removed from ready, dep recorded).
        //         tx_c is still pending (ready) — it was in the mempool but not
        // included in B.
        assert!(!state_b.is_ready(&TestTxId("tx_b")));
        assert!(!state_b.is_orphan(&TestTxId("tx_b")));
        assert!(state_b.has_processed_dep(&Bytes::from_static(b"out_b")));
        assert!(state_b.is_ready(&TestTxId("tx_c")));
        assert!(!state_b.has_processed_dep(&Bytes::from_static(b"out_c")));

        // Fork C: tx_c is confirmed; tx_b is still pending (ready).
        assert!(!state_c.is_ready(&TestTxId("tx_c")));
        assert!(!state_c.is_orphan(&TestTxId("tx_c")));
        assert!(state_c.has_processed_dep(&Bytes::from_static(b"out_c")));
        assert!(state_c.is_ready(&TestTxId("tx_b")));
        assert!(!state_c.has_processed_dep(&Bytes::from_static(b"out_b")));
    }

    /// An orphan tx gets resolved on the fork that confirms its dependency
    /// producer, but stays orphaned on the fork where the producer remains
    /// unconfirmed. This is the key fork-isolation property.
    #[tokio::test]
    async fn test_mempool_orphan_resolved_differently_per_fork() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // confirms tx_producer → dep "X" recorded
        let c = id(3); // does NOT confirm tx_producer

        let tx_producer = tx("tx_producer", vec![], vec!["X"]);

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![tx_producer.clone()]);
        getter.insert(c, a, vec![]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;
        tracker.process_new_block(&block_event(a, a)).await.unwrap();

        // Both txs arrive in the mempool; tx_consumer is orphaned until "X" is
        // produced.
        tracker.process_new_tx(&tx("tx_consumer", vec!["X"], vec!["Y"]));
        tracker.process_new_tx(&tx_producer);

        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        tracker.process_new_block(&block_event(c, c)).await.unwrap();

        let state_b = tracker.get_block_state(&b).unwrap();
        let state_c = tracker.get_block_state(&c).unwrap();

        // Fork B: tx_producer confirmed → dep "X" recorded → tx_consumer promoted to
        // ready.
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

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![]);
        getter.insert(c, b, vec![]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.process_new_block(&block_event(a, a)).await.unwrap();
        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        tracker.process_new_block(&block_event(c, c)).await.unwrap();

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

        let mut getter = MockGetter::new();
        getter.insert(a, genesis, vec![]);
        getter.insert(b, a, vec![]);
        getter.insert(d, a, vec![]);

        let mut tracker = ForksTracker::new(getter);
        seed_genesis(&mut tracker, genesis).await;

        tracker.process_new_block(&block_event(a, a)).await.unwrap();
        tracker.process_new_block(&block_event(b, b)).await.unwrap();
        tracker.process_new_block(&block_event(d, d)).await.unwrap();

        assert_eq!(tracker.current_tips.len(), 2);

        tracker.process_lib(&lib_event(vec![d]));

        assert!(!tracker.current_tips.contains_key(&d));
        assert!(tracker.current_tips.contains_key(&b));
    }

    /// Processing a block whose parent is not in `current_tips` returns
    /// `ParentNotFound`.
    #[tokio::test]
    async fn test_process_block_unknown_parent_returns_error() {
        let orphan = id(77);
        let mut getter = MockGetter::new();
        getter.insert(orphan, id(50), vec![]);

        let mut tracker = ForksTracker::new(getter);

        let result = tracker
            .process_new_block(&block_event(orphan, orphan))
            .await;
        assert!(matches!(result, Err(ForksTrackerError::ParentNotFound(_))));
    }

    /// When `BlockGetter` cannot find the block the error propagates unchanged.
    #[tokio::test]
    async fn test_block_getter_failure_propagates() {
        let unknown = id(99);
        let getter = MockGetter::new(); // empty — will return BlockNotFound

        let mut tracker = ForksTracker::new(getter);

        let result = tracker
            .process_new_block(&block_event(unknown, unknown))
            .await;
        assert!(matches!(result, Err(ForksTrackerError::BlockNotFound)));
    }
}

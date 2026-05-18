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
use lb_ledger::LedgerState;
use tracing::error;

use super::tracker::TxTrackerState;
use crate::backend::inspector::LedgerStateInspector;

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

    pub fn process_new_block(
        &mut self,
        block_id: &HeaderId,
        BlockInfo {
            parent,
            transactions,
        }: BlockInfo<Tx>,
    ) -> Result<(), ForksTrackerError> {
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

    pub fn process_new_tx(
        &mut self,
        tx: &Tx,
        header_id: &HeaderId,
        tip_deps: &HashSet<DependencyId>,
    ) {
        let state = self
            .current_tips
            .get_mut(header_id)
            .expect("This header at this point is always present");
        state.process_tx(tx.clone(), tip_deps);
        // let Self { current_tips, .. } = self;
        // let tips_len = current_tips.len();
        // let ledger_getter: Adapter = self.adapter.clone();
        // let header_ids: Vec<_> = current_tips.keys().cloned().collect();
        // let mut ledger_states = pin!(
        //     tokio_stream::iter(
        //         header_ids
        //             .into_iter()
        //             .zip(std::iter::repeat_with(|| ledger_getter.clone()))
        //     )
        //     .map(async |(header_id, ledger_getter)| {
        //         let ledger_state =
        // ledger_getter.get_ledger_deps(&header_id).await;
        //         (header_id, ledger_state)
        //     })
        //     .buffer_unordered(tips_len)
        // );
        // while let Some((header_id, ledger_state)) =
        // ledger_states.next().await {     let state = current_tips
        //         .get_mut(&header_id)
        //         .expect("This header at this point is always present");
        //     match ledger_state {
        //         Ok(ledger_state_deps) => {
        //             state.process_tx(tx.clone(), &ledger_state_deps);
        //         }
        //         Err(e) => {
        //             error!("Error getting ledger state for block {header_id}:
        // {e:?}");         }
        //     }
        // }
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
    use std::collections::{BTreeMap, HashSet};

    use async_trait::async_trait;
    use bytes::Bytes;
    use lb_chain_service::{LibUpdate, PrunedBlocksInfo};
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

    /// Stub adapter — satisfies trait bounds; neither method is called by the
    /// sync `process_new_block` / `process_new_tx` code paths exercised here.
    #[derive(Clone)]
    struct MockAdapter;

    #[async_trait]
    impl BlockInfoGetter<TestTx> for MockAdapter {
        async fn get_block(&self, _: &HeaderId) -> Result<BlockInfo<TestTx>, ForksTrackerError> {
            unimplemented!("not used in sync tests")
        }
    }

    #[async_trait]
    impl LedgerStateGetter for MockAdapter {
        async fn get_ledger_deps(
            &self,
            _: &HeaderId,
        ) -> Result<HashSet<DependencyId>, ForksTrackerError> {
            unimplemented!("not used in sync tests")
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

    fn block_info(parent: HeaderId, txs: Vec<TestTx>) -> BlockInfo<TestTx> {
        BlockInfo {
            parent,
            transactions: txs,
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
    fn seed_genesis(
        tracker: &mut ForksTracker<TestTx, TestTxId, MockAdapter>,
        genesis: HeaderId,
    ) {
        let root = id(255);
        tracker.current_tips.insert(root, TxTrackerState::new());
        tracker
            .process_new_block(&genesis, block_info(root, vec![]))
            .unwrap();
    }

    /// Apply `tx` to every current tip with an empty frontier (txs with no deps
    /// go ready; txs with unmet deps go orphan).
    fn broadcast_tx(
        tracker: &mut ForksTracker<TestTx, TestTxId, MockAdapter>,
        t: &TestTx,
        frontier: &HashSet<DependencyId>,
    ) {
        let tips: Vec<HeaderId> = tracker.current_tips.keys().cloned().collect();
        for hid in tips {
            tracker.process_new_tx(t, &hid, frontier);
        }
    }

    // ── tests ────────────────────────────────────────────────────────────────

    /// A single chain genesis → A → B → C: the frontier always holds exactly
    /// one tip and historical states accumulate in `states`.
    #[test]
    fn test_linear_chain_tip_tracking() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);

        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&genesis));

        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();
        assert_eq!(tracker.current_tips.len(), 1);
        assert!(tracker.current_tips.contains_key(&a));
        assert!(tracker.states.contains_key(&genesis));

        tracker
            .process_new_block(&b, block_info(a, vec![]))
            .unwrap();
        assert!(tracker.current_tips.contains_key(&b));
        assert!(tracker.states.contains_key(&a));

        tracker
            .process_new_block(&c, block_info(b, vec![]))
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
    #[test]
    fn test_mempool_tx_propagates_to_all_tips() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);

        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();
        tracker
            .process_new_block(&b, block_info(a, vec![]))
            .unwrap();
        tracker
            .process_new_block(&c, block_info(a, vec![]))
            .unwrap();

        assert_eq!(tracker.current_tips.len(), 2);

        let empty = HashSet::new();
        broadcast_tx(&mut tracker, &tx("mempool_tx", vec![], vec!["dep_x"]), &empty);

        for state in tracker.current_tips.values() {
            assert!(state.is_ready(&TestTxId("mempool_tx")));
        }
    }

    /// Txs confirmed in block B are removed from the mempool view on fork B
    /// while remaining pending on fork C, and vice versa. Fork states are fully
    /// independent.
    #[test]
    fn test_fork_states_are_independent() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // fork 1: A → B (confirms tx_b)
        let c = id(3); // fork 2: A → C (confirms tx_c)

        let tx_b = tx("tx_b", vec![], vec!["out_b"]);
        let tx_c = tx("tx_c", vec![], vec!["out_c"]);

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);
        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();

        // Both txs arrive in the mempool before either block is processed.
        let empty = HashSet::new();
        broadcast_tx(&mut tracker, &tx_b, &empty);
        broadcast_tx(&mut tracker, &tx_c, &empty);

        tracker
            .process_new_block(&b, block_info(a, vec![tx_b.clone()]))
            .unwrap();
        tracker
            .process_new_block(&c, block_info(a, vec![tx_c.clone()]))
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
    #[test]
    fn test_mempool_orphan_resolved_differently_per_fork() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // confirms tx_producer → promotes tx_consumer
        let c = id(3); // does NOT confirm tx_producer

        let tx_producer = tx("tx_producer", vec![], vec!["X"]);

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);
        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();

        // Both txs arrive in the mempool; tx_consumer is orphaned until "X" is
        // produced.
        let empty = HashSet::new();
        broadcast_tx(
            &mut tracker,
            &tx("tx_consumer", vec!["X"], vec!["Y"]),
            &empty,
        );
        broadcast_tx(&mut tracker, &tx_producer, &empty);

        tracker
            .process_new_block(&b, block_info(a, vec![tx_producer.clone()]))
            .unwrap();
        tracker
            .process_new_block(&c, block_info(a, vec![]))
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
    #[test]
    fn test_lib_prunes_stale_and_immutable_blocks() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2);
        let c = id(3);

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);
        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();
        tracker
            .process_new_block(&b, block_info(a, vec![]))
            .unwrap();
        tracker
            .process_new_block(&c, block_info(b, vec![]))
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
    #[test]
    fn test_lib_prunes_stale_fork_tip() {
        let genesis = id(0);
        let a = id(1);
        let b = id(2); // canonical tip
        let d = id(4); // stale fork tip

        let mut tracker = ForksTracker::new(MockAdapter);
        seed_genesis(&mut tracker, genesis);
        tracker
            .process_new_block(&a, block_info(genesis, vec![]))
            .unwrap();
        tracker
            .process_new_block(&b, block_info(a, vec![]))
            .unwrap();
        tracker
            .process_new_block(&d, block_info(a, vec![]))
            .unwrap();

        assert_eq!(tracker.current_tips.len(), 2);

        tracker.process_lib(&lib_event(vec![d]));

        assert!(!tracker.current_tips.contains_key(&d));
        assert!(tracker.current_tips.contains_key(&b));
    }

    /// Processing a block whose parent is not known returns `ParentNotFound`.
    #[test]
    fn test_process_block_unknown_parent_returns_error() {
        let mut tracker = ForksTracker::new(MockAdapter);
        let result = tracker.process_new_block(
            &id(77),
            block_info(id(50), vec![]),
        );
        assert!(matches!(result, Err(ForksTrackerError::ParentNotFound(_))));
    }
}

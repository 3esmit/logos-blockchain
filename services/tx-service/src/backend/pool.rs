use std::{
    collections::BTreeSet,
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures::{Stream, stream};
use lb_chain_service::{LibUpdate, ProcessedBlockEvent, storage::StorageAdapter};
use lb_core::{
    header::HeaderId,
    mantle::{Transaction, TransactionDependencies},
};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use super::Status;
use crate::{
    backend::{
        MemPool, MempoolError, RecoverableMempool,
        forks::{BlockInfoGetter, ForksTracker, LedgerStateGetter},
    },
    metrics,
    storage::MempoolStorageAdapter,
};

const REMOVED_ITEM_GRACE_PERIOD: Duration = Duration::from_mins(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolRecoveryState<Key>
where
    Key: Hash + Eq + Ord,
{
    // pub pending_items: BTreeSet<Key>,
    // pub removed_items: BTreeMap<Key, u64>,
    pub last_item_timestamp: u64,
    _phantom: PhantomData<Key>,
}

pub struct Mempool<Tx, TxHash, Adapter, RuntimeServiceId>
where
    TxHash: Eq + Hash,
{
    last_item_timestamp: u64,
    adapter: Adapter,
    forks_tracker: ForksTracker<Tx, TxHash, Adapter>,
    _phantom: PhantomData<RuntimeServiceId>,
}

impl<Tx, TxHash, Adapter, RuntimeServiceId> Debug for Mempool<Tx, TxHash, Adapter, RuntimeServiceId>
where
    TxHash: Eq + Hash,
    Tx: Debug,
    TxHash: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mempool")
            .field("last_item_timestamp", &self.last_item_timestamp)
            .field("storage_adapter", &"<StorageAdapter>")
            .finish()
    }
}

#[async_trait]
impl<Tx, Adapter, RuntimeServiceId> MemPool for Mempool<Tx, Tx::Hash, Adapter, RuntimeServiceId>
where
    Tx: TransactionDependencies
        + Clone
        + Send
        + Sync
        + 'static
        + Serialize
        + for<'de> Deserialize<'de>,
    <Tx as Transaction>::Hash: Hash + Eq + Ord + Clone + Send + Sync + 'static,
    Adapter: MempoolStorageAdapter<RuntimeServiceId, Tx = Tx> + Send + Sync + 'static,
    Adapter: BlockInfoGetter<Tx> + LedgerStateGetter + Clone,
    <Adapter as MempoolStorageAdapter<RuntimeServiceId>>::Error: Debug,
    RuntimeServiceId: Send + Sync,
{
    type Settings = ();
    type Tx = Tx;
    type TxHash = Tx::Hash;
    type BlockId = HeaderId;
    type Adapter = Adapter;

    fn new(_settings: Self::Settings, adapter: Self::Adapter) -> Self {
        Self {
            last_item_timestamp: 0,
            forks_tracker: ForksTracker::new(adapter.clone()),
            adapter,
            _phantom: PhantomData,
        }
    }

    async fn add_item<I: Into<Self::Tx> + Send>(&mut self, item: I) -> Result<(), MempoolError> {
        self.last_item_timestamp = current_timestamp_millis();
        self.forks_tracker.process_new_tx(&item.into()).await;
        Ok(())
    }

    async fn view(
        &self,
        ancestor_hint: HeaderId,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, MempoolError> {
        Ok(Box::pin(stream::iter(
            self.forks_tracker.get_frontier_txs(ancestor_hint),
        )))
    }

    async fn get_items_by_keys<I>(
        &self,
        keys: I,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, MempoolError>
    where
        I: IntoIterator<Item = Self::TxHash> + Send,
    {
        unimplemented!()
    }

    async fn remove(&mut self, keys: &[Self::TxHash]) {
        self.forks_tracker.force_remove_txs(keys);

        // TODO: Add metrics back
        // metrics::mempool_transactions_removed(removed_count);
        // metrics::mempool_transactions_pending(self.pending_items.len());
    }

    fn last_item_timestamp(&self) -> u64 {
        self.last_item_timestamp
    }

    fn status(&self, items: &[Self::TxHash]) -> Vec<Status> {
        todo!("What to check here?");
        vec![]
    }

    async fn process_new_block_event(&mut self, event: ProcessedBlockEvent) {
        if let Err(e) = self.forks_tracker.process_new_block(&event).await {
            error!("Failed to process new block event: {e:?}");
        }
    }

    fn process_lib_event(&mut self, event: LibUpdate) {
        self.forks_tracker.process_lib(&event);
    }
}

impl<Tx, Adapter, RuntimeServiceId> RecoverableMempool
    for Mempool<Tx, Tx::Hash, Adapter, RuntimeServiceId>
where
    Tx::Hash:
        Hash + Eq + Ord + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Tx: TransactionDependencies
        + Clone
        + Ord
        + Send
        + Sync
        + 'static
        + Serialize
        + for<'de> Deserialize<'de>,
    Adapter: MempoolStorageAdapter<RuntimeServiceId, Tx = Tx> + Clone + Send + Sync + 'static,
    Adapter: BlockInfoGetter<Tx>,
    Adapter: LedgerStateGetter,
    <Adapter as MempoolStorageAdapter<RuntimeServiceId>>::Error: Debug,
    RuntimeServiceId: Send + Sync,
{
    type RecoveryState = PoolRecoveryState<Tx::Hash>;

    fn save(&self) -> Self::RecoveryState {
        PoolRecoveryState {
            last_item_timestamp: self.last_item_timestamp,
            _phantom: PhantomData,
        }
    }

    fn recover(
        _settings: <Self as MemPool>::Settings,
        state: Self::RecoveryState,
        adapter: <Self as MemPool>::Adapter,
    ) -> Self {
        Self {
            last_item_timestamp: state.last_item_timestamp,
            forks_tracker: ForksTracker::new(adapter.clone()),
            adapter,
            _phantom: PhantomData,
        }
    }
}

fn current_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

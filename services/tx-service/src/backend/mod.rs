pub mod adapter;
mod forks;
mod history;
pub mod pool;
mod tracker;

use std::pin::Pin;

pub use adapter::TrackerAdapter;
pub use forks::{BlockInfo, BlockInfoGetter, ForksTrackerError, LedgerStateGetter};
use futures::Stream;
use lb_chain_service::{LibUpdate, ProcessedBlockEvent};
use overwatch::{DynError, overwatch::OverwatchHandle};
pub use pool::{Mempool, PoolRecoveryState};
use serde::{Deserialize, Serialize};

use crate::storage::MempoolStorageAdapter;

#[derive(thiserror::Error, Debug)]
pub enum MempoolError {
    #[error("Item already in mempool")]
    ExistingItem,
    #[error("Storage operation failed: {0}")]
    StorageError(String),
    #[error(transparent)]
    DynamicPoolError(#[from] DynError),
}

#[async_trait::async_trait]
pub trait MempoolAdapter<Tx, RuntimeServiceId>:
    MempoolStorageAdapter<RuntimeServiceId, Tx = Tx> + BlockInfoGetter<Tx> + LedgerStateGetter + Sized
{
    async fn new(handle: OverwatchHandle<RuntimeServiceId>) -> Result<Self, DynError>;
}

#[async_trait::async_trait]
pub trait MemPool {
    type Settings: Send;
    type Tx: Send;
    type TxHash: Send + Sync + Clone + Ord;
    type BlockId: Send;
    type Adapter: Send;

    /// Construct a new empty pool with storage
    fn new(settings: Self::Settings, storage: Self::Adapter) -> Self;

    /// Add a new item to the mempool, for example because we received it from
    /// the network. The item is stored in external storage.
    async fn add_item<I: Into<Self::Tx> + Send>(&mut self, item: I) -> Result<(), MempoolError>;

    /// Return a view over items contained in the mempool.
    /// Implementations should provide *at least* all the items which have not
    /// been marked as in a block.
    /// The hint on the ancestor *should* be used by the implementation to
    /// display additional items that were not included up to that point if
    /// available.
    async fn view(
        &self,
        ancestor_hint: Self::BlockId,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, MempoolError>;

    /// Get multiple items by their keys from the mempool via storage lookup
    async fn get_items_by_keys<I>(
        &self,
        keys: I,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, MempoolError>
    where
        I: IntoIterator<Item = Self::TxHash> + Send,
        <I as IntoIterator>::IntoIter: Send;

    /// Remove items from the mempool..
    async fn remove(&mut self, items: &[Self::TxHash]);

    async fn pending_item_count(&self) -> Result<usize, MempoolError>;
    fn last_item_timestamp(&self) -> u64;

    // Return the status of a set of items.
    // This is a best effort attempt, and implementations are free to return
    // `Unknown` for all of them.
    fn status(&self, items: &[Self::TxHash]) -> Vec<Status>;

    async fn process_new_block_event(&mut self, event: ProcessedBlockEvent);
    fn process_lib_event(&mut self, event: LibUpdate);
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Status {
    /// Unknown status
    Unknown,
    /// Pending status
    Pending,
}

/// Trait for mempools that can be recovered from saved state
pub trait RecoverableMempool: MemPool {
    type RecoveryState: Send + Sync + Serialize + for<'de> Deserialize<'de>;

    /// Save current state for recovery
    fn save(&self) -> Self::RecoveryState;

    /// Recover from saved state with storage
    fn recover(
        settings: <Self as MemPool>::Settings,
        state: Self::RecoveryState,
        storage: <Self as MemPool>::Adapter,
    ) -> Self;
}

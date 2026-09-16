use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    pin::Pin,
};

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use lb_core::{
    block::Block,
    codec::{DeserializeOp as _, SerializeOp as _},
    events::Events,
    header::HeaderId,
    mantle::{traits::Hashable, transactions::hash::TxHash},
};
use lb_cryptarchia_engine::Slot;
use lb_log_targets::chain;
use lb_storage_service::{
    StorageService,
    api::{StorageServiceApi, chain::StorageChainApi},
    backends::StorageBackend,
};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use serde::{Serialize, de::DeserializeOwned};

use crate::storage::StorageAdapter as StorageAdapterTrait;

const LOG_TARGET: &str = chain::service::STORAGE;

pub struct StorageAdapter<Storage, Tx, RuntimeServiceId>
where
    Storage: StorageBackend + Send + Sync + 'static,
{
    pub storage_relay:
        OutboundRelay<<StorageService<Storage, RuntimeServiceId> as ServiceData>::Message>,
    storage_api: StorageServiceApi<Storage, RuntimeServiceId>,
    _tx: PhantomData<Tx>,
}

impl<Storage, Tx, RuntimeServiceId> Clone for StorageAdapter<Storage, Tx, RuntimeServiceId>
where
    Storage: StorageBackend + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            storage_relay: self.storage_relay.clone(),
            storage_api: self.storage_api.clone(),
            _tx: PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<Storage, Tx, RuntimeServiceId> StorageAdapterTrait<RuntimeServiceId>
    for StorageAdapter<Storage, Tx, RuntimeServiceId>
where
    Storage: StorageBackend + Send + Sync + 'static,
    <Storage as StorageChainApi>::Block: TryFrom<Block<Tx>> + TryInto<Block<Tx>>,
    <Storage as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <Storage as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    Tx: Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
{
    type Backend = Storage;
    type Block = Block<Tx>;
    type Tx = Tx;
    type Events = Events;

    async fn new(
        storage_relay: OutboundRelay<
            <StorageService<Self::Backend, RuntimeServiceId> as ServiceData>::Message,
        >,
    ) -> Self {
        Self {
            storage_api: StorageServiceApi::new(storage_relay.clone()),
            storage_relay,
            _tx: PhantomData,
        }
    }

    async fn get_block(&self, header_id: &HeaderId) -> Option<Self::Block> {
        match self.storage_api.get_block(*header_id).await {
            Ok(Some(block)) => block.try_into().ok(),
            Ok(None) => None,
            Err(error) => {
                tracing::error!(target: LOG_TARGET, "Failed to receive block from storage API: {error}");
                None
            }
        }
    }

    async fn store_block_data(
        &self,
        header_id: HeaderId,
        parent_id: HeaderId,
        block: Self::Block,
        events: Self::Events,
        immutable_ids: BTreeMap<Slot, HeaderId>,
    ) -> Result<(), overwatch::DynError> {
        let block = block
            .try_into()
            .map_err(|_| "Failed to convert block to storage format")?;

        let events = events
            .try_into()
            .map_err(|_| "Failed to convert events to storage format")?;

        self.storage_api
            .store_block_data(header_id, parent_id, block, events, immutable_ids)
            .await
            .map_err(|e| format!("Failed to store block data in storage: {e}").into())
    }

    async fn get_block_parent(&self, header_id: &HeaderId) -> Option<HeaderId> {
        self.storage_api
            .get_block_parent(*header_id)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(target: LOG_TARGET, "Failed to receive block parent from storage API: {e}");
                None
            })
    }

    async fn get_block_events(&self, header_id: &HeaderId) -> Option<Self::Events> {
        let events = match self.storage_api.get_block_events(*header_id).await {
            Ok(Some(events)) => events,
            Ok(None) => return None,
            Err(error) => {
                tracing::error!(target: LOG_TARGET, "Failed to receive block events from storage API: {error}");
                return None;
            }
        };
        let Ok(events) = events.try_into() else {
            tracing::error!(target: LOG_TARGET, "Failed to convert block events loaded from storage");
            return None;
        };
        Some(events)
    }

    async fn remove_block(
        &self,
        header_id: HeaderId,
    ) -> Result<Option<Self::Block>, overwatch::DynError> {
        let Some(removed_block) = self
            .storage_api
            .remove_block(header_id)
            .await
            .map_err(|e| format!("Failed to remove block from storage: {e}"))?
        else {
            return Ok(None);
        };

        let deserialized_block = removed_block
            .try_into()
            .map_err(|_| "Failed to convert block to storage format.")?;

        Ok(Some(deserialized_block))
    }

    async fn store_immutable_block_ids(
        &self,
        blocks: BTreeMap<Slot, HeaderId>,
    ) -> Result<(), overwatch::DynError> {
        self.storage_api
            .store_immutable_block_ids(blocks)
            .await
            .map_err(|e| format!("Failed to store immutable block ids in storage: {e}").into())
    }

    async fn store_transactions(
        &self,
        transactions: Vec<Self::Tx>,
    ) -> Result<(), overwatch::DynError> {
        let storage_transactions: HashMap<TxHash, <Storage as StorageChainApi>::Tx> = transactions
            .into_iter()
            .map(|tx| {
                let hash = tx.hash();
                Tx::to_bytes(&tx)
                    .map(|bytes| (hash, bytes.into()))
                    .map_err(|_| "Failed to convert transaction to storage format".into())
            })
            .collect::<Result<HashMap<_, _>, overwatch::DynError>>()?;

        self.storage_api
            .store_transactions(storage_transactions)
            .await
            .map_err(|e| format!("Failed to send store transactions batch request: {e}"))?;

        Ok(())
    }

    async fn get_transactions(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, overwatch::DynError> {
        let storage_stream = self
            .storage_api
            .get_transactions(tx_hashes)
            .await
            .map_err(|e| format!("Failed to get transactions from storage: {e}"))?;

        let mapped_stream =
            storage_stream.filter_map(async |storage_tx| Tx::from_bytes(storage_tx.as_ref()).ok());

        Ok(Box::pin(mapped_stream))
    }

    async fn remove_transactions(&self, tx_hashes: &[TxHash]) -> Result<(), overwatch::DynError> {
        self.storage_api
            .remove_transactions(tx_hashes.to_vec())
            .await
            .map_err(|e| format!("Failed to send remove transactions batch request: {e}"))?;

        Ok(())
    }
}

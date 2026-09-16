use std::{collections::HashMap, marker::PhantomData, pin::Pin};

use async_trait::async_trait;
use futures::{Stream, StreamExt as _};
use lb_core::{
    codec::{DeserializeOp as _, SerializeOp as _},
    mantle::transactions::hash::TxHash,
};
use lb_storage_service::{StorageService, api::StorageServiceApi, backends::rocksdb::RocksBackend};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use serde::{Deserialize, Serialize};

use crate::{backend::MempoolError, storage::MempoolStorageAdapter};

/// A `RocksDB` storage adapter that stores transactions via storage service
/// relay
#[derive(Clone)]
pub struct RocksStorageAdapter<Item, Key> {
    storage_api: StorageServiceApi<RocksBackend, ()>,
    _phantom: PhantomData<(Item, Key)>,
}

#[async_trait]
impl<Item, Key, RuntimeServiceId> MempoolStorageAdapter<RuntimeServiceId>
    for RocksStorageAdapter<Item, Key>
where
    Item: Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Key: Clone + Send + Sync + 'static + Into<TxHash>,
{
    type Backend = RocksBackend;

    type Item = Item;

    type Key = Key;

    type Error = MempoolError;

    fn new(
        storage_relay: OutboundRelay<
            <StorageService<Self::Backend, RuntimeServiceId> as ServiceData>::Message,
        >,
    ) -> Self {
        Self {
            storage_api: StorageServiceApi::new(storage_relay),
            _phantom: PhantomData,
        }
    }

    async fn store_item(&mut self, key: Self::Key, item: Self::Item) -> Result<(), Self::Error> {
        let item_bytes = item
            .to_bytes()
            .map_err(|e| MempoolError::DynamicPoolError(e.into()))?;

        let tx_hash: TxHash = key.into();
        let mut transactions = HashMap::new();
        transactions.insert(tx_hash, item_bytes);

        self.storage_api
            .store_transactions(transactions)
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send store transactions request".into())
            })
    }

    async fn get_items(
        &self,
        keys: &[Self::Key],
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Item> + Send>>, Self::Error> {
        if keys.is_empty() {
            return Ok(Box::pin(futures::stream::empty()));
        }

        let tx_hashes: Vec<TxHash> = keys.iter().cloned().map(Into::into).collect();

        let tx_stream = self
            .storage_api
            .get_transactions(tx_hashes)
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send get transactions request".into())
            })?;

        let item_stream = tx_stream.filter_map(async |bytes| Self::Item::from_bytes(&bytes).ok());

        Ok(Box::pin(item_stream))
    }

    async fn remove_items(&mut self, keys: &[Self::Key]) -> Result<(), Self::Error> {
        let tx_hashes: Vec<TxHash> = keys.iter().cloned().map(Into::into).collect();

        self.storage_api
            .remove_transactions(tx_hashes)
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send remove transactions request".into())
            })
    }
}

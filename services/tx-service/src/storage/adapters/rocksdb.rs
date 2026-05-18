use std::{
    collections::{BTreeSet, HashMap},
    marker::PhantomData,
    pin::Pin,
};

use async_trait::async_trait;
use futures::{Stream, StreamExt as _};
use lb_core::{
    block::Block,
    codec::{DeserializeOp as _, SerializeOp as _},
    header::HeaderId,
    mantle::{Transaction, TxHash},
};
use lb_storage_service::{StorageMsg, StorageService, backends::rocksdb::RocksBackend};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use serde::{Deserialize, Serialize};

use crate::{backend::MempoolError, storage::MempoolStorageAdapter};

/// A `RocksDB` storage adapter that stores transactions via storage service
/// relay
#[derive(Clone)]
pub struct RocksStorageAdapter<Tx, TxId> {
    storage_relay: OutboundRelay<StorageMsg<RocksBackend>>,
    _phantom: PhantomData<(Tx, TxId)>,
}

#[async_trait]
impl<Tx, RuntimeServiceId> MempoolStorageAdapter<RuntimeServiceId>
    for RocksStorageAdapter<Tx, Tx::Hash>
where
    Tx: Transaction + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Tx::Hash: Clone + Send + Sync + 'static + Into<TxHash>,
{
    type Backend = RocksBackend;

    type Tx = Tx;

    type Error = MempoolError;

    fn new(
        storage_relay: OutboundRelay<
            <StorageService<Self::Backend, RuntimeServiceId> as ServiceData>::Message,
        >,
    ) -> Self {
        Self {
            storage_relay,
            _phantom: PhantomData,
        }
    }

    async fn store_tx(&mut self, tx: Self::Tx) -> Result<(), Self::Error> {
        let item_bytes = tx
            .to_bytes()
            .map_err(|e| MempoolError::DynamicPoolError(e.into()))?;

        let tx_hash = tx.hash();
        let mut transactions = HashMap::new();
        transactions.insert(tx_hash.into(), item_bytes);

        self.storage_relay
            .send(StorageMsg::store_transactions_request(transactions))
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send store transactions request".into())
            })
    }

    async fn get_tx(
        &self,
        keys: &BTreeSet<<Self::Tx as Transaction>::Hash>,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, Self::Error> {
        if keys.is_empty() {
            return Ok(Box::pin(futures::stream::empty()));
        }

        let tx_hashes: BTreeSet<TxHash> = keys.iter().cloned().map(Into::into).collect();

        let (reply_channel, reply_rx) = tokio::sync::oneshot::channel();
        self.storage_relay
            .send(StorageMsg::get_transactions_request(
                tx_hashes,
                reply_channel,
            ))
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send get transactions request".into())
            })?;

        let tx_stream = reply_rx.await.map_err(|_| {
            MempoolError::DynamicPoolError("Failed to receive transactions response".into())
        })?;

        let item_stream = tx_stream.filter_map(async |bytes| Self::Tx::from_bytes(&bytes).ok());

        Ok(Box::pin(item_stream))
    }

    async fn remove_txs(
        &mut self,
        keys: &[<Self::Tx as Transaction>::Hash],
    ) -> Result<(), Self::Error> {
        let tx_hashes: Vec<TxHash> = keys.iter().cloned().map(Into::into).collect();

        self.storage_relay
            .send(StorageMsg::remove_transactions_request(tx_hashes))
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send remove transactions request".into())
            })
    }

    async fn get_block(&self, header: HeaderId) -> Result<Option<Block<Self::Tx>>, Self::Error> {
        let (reply_channel, reply_rx) = tokio::sync::oneshot::channel();
        self.storage_relay
            .send(StorageMsg::get_block_request(header, reply_channel))
            .await
            .map_err(|_| {
                MempoolError::DynamicPoolError("Failed to send get block request".into())
            })?;
        match reply_rx.await {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => Ok(Some(
                Block::from_bytes(&bytes).map_err(|e| MempoolError::DynamicPoolError(e.into()))?,
            )),
            Err(_) => Err(MempoolError::DynamicPoolError(
                "Failed to receive block response".into(),
            )),
        }
    }
}

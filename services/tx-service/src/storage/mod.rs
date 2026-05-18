use std::{collections::BTreeSet, pin::Pin};

use async_trait::async_trait;
use futures::Stream;
use lb_core::{
    block::{Block, BlockNumber},
    header::HeaderId,
    mantle::Transaction,
};
use lb_storage_service::backends::StorageBackend;

pub mod adapters;

#[async_trait]
pub trait MempoolStorageAdapter<RuntimeServiceId>: Send + Sync {
    type Backend: StorageBackend + Send + Sync + 'static;

    type Tx: Transaction + Send;

    type Error: Send;

    fn new(
        storage_relay: overwatch::services::relay::OutboundRelay<
            <lb_storage_service::StorageService<Self::Backend, RuntimeServiceId> as overwatch::services::ServiceData>::Message,
        >,
    ) -> Self;

    async fn store_tx(&mut self, key: Self::Tx) -> Result<(), Self::Error>;

    async fn get_tx(
        &self,
        keys: &BTreeSet<<Self::Tx as Transaction>::Hash>,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, Self::Error>;

    async fn remove_txs(
        &mut self,
        keys: &[<Self::Tx as Transaction>::Hash],
    ) -> Result<(), Self::Error>;

    async fn get_block(&self, header: HeaderId) -> Result<Option<Block<Self::Tx>>, Self::Error>;
}

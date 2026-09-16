use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
};

use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use lb_core::{
    block::Block,
    codec::DeserializeOp as _,
    header::HeaderId,
    mantle::{
        TxHash,
        traits::{Hashable, StorageSize},
    },
};
use lb_storage_service::{StorageService, api::StorageServiceApi, backends::rocksdb::RocksBackend};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use serde::{Serialize, de::DeserializeOwned};

use crate::http::storage::StorageAdapter;

pub struct RocksAdapter<RuntimeServiceId> {
    _runtime_service_id: PhantomData<RuntimeServiceId>,
}

#[async_trait::async_trait]
impl<RuntimeServiceId> StorageAdapter<RuntimeServiceId> for RocksAdapter<RuntimeServiceId>
where
    RuntimeServiceId: Debug + Sync + Display + 'static,
{
    async fn get_block<Tx>(
        storage_relay: OutboundRelay<
            <StorageService<RocksBackend, RuntimeServiceId> as ServiceData>::Message,
        >,
        id: HeaderId,
    ) -> Result<Option<Block<Tx>>, crate::http::DynError>
    where
        Tx: Serialize
            + DeserializeOwned
            + Clone
            + Eq
            + Hashable<Hash = TxHash>
            + StorageSize
            + 'static,
    {
        let key: [u8; 32] = id.into();
        let storage_api = StorageServiceApi::<RocksBackend, RuntimeServiceId>::new(storage_relay);
        let Some(bytes) = storage_api
            .load(Bytes::copy_from_slice(&key))
            .await
            .map_err(|e| Box::new(e) as crate::http::DynError)?
        else {
            return Ok(None);
        };
        Block::from_bytes(&bytes)
            .map(Some)
            .map_err(|e| Box::new(e) as crate::http::DynError)
    }

    async fn get_transactions<Tx>(
        storage_relay: OutboundRelay<
            <StorageService<RocksBackend, RuntimeServiceId> as ServiceData>::Message,
        >,
        id: TxHash,
    ) -> Result<Vec<Tx>, crate::http::DynError>
    where
        Tx: DeserializeOwned + Send,
    {
        let storage_api = StorageServiceApi::<RocksBackend, RuntimeServiceId>::new(storage_relay);
        let bytes_stream = storage_api
            .get_transactions(vec![id])
            .await
            .map_err(|error| Box::new(error) as crate::http::DynError)?;

        bytes_stream
            .map(|bytes| {
                serde_json::from_slice::<Tx>(bytes.as_ref())
                    .map_err(|error| Box::new(error) as crate::http::DynError)
            })
            .try_collect::<Vec<_>>()
            .await
    }
}

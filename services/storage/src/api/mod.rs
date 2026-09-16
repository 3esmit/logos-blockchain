use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    num::NonZeroUsize,
    ops::RangeInclusive,
    pin::Pin,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use lb_core::{header::HeaderId, mantle::TxHash};
use lb_cryptarchia_engine::Slot;
use overwatch::services::{ServiceData, relay::OutboundRelay};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::{
    StorageMsg, StorageService, StorageServiceError,
    api::chain::{StorageChainApi, requests::ChainApiRequest},
    backends::StorageBackend,
};

pub mod backend;
pub mod chain;

#[async_trait]
pub trait StorageBackendApi: StorageChainApi {}

/// Errors returned by the typed storage-service API.
#[derive(Debug, Error)]
pub enum StorageApiError {
    #[error("Failed to establish connection to storage service: {0}")]
    CommsFailure(String),
    #[error("Storage request failed: {0}")]
    Backend(String),
}

/// Typed wrapper over a storage service relay.
pub struct StorageServiceApi<Backend, RuntimeServiceId>
where
    Backend: StorageBackend + Send + Sync + 'static,
{
    relay: OutboundRelay<StorageMsg<Backend>>,
    _id: PhantomData<fn() -> RuntimeServiceId>,
}

impl<Backend, RuntimeServiceId> Clone for StorageServiceApi<Backend, RuntimeServiceId>
where
    Backend: StorageBackend + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            relay: self.relay.clone(),
            _id: PhantomData,
        }
    }
}

impl<Backend, RuntimeServiceId> StorageServiceApi<Backend, RuntimeServiceId>
where
    Backend: StorageBackend + Send + Sync + 'static,
{
    #[must_use]
    pub const fn new(
        relay: OutboundRelay<<StorageService<Backend, RuntimeServiceId> as ServiceData>::Message>,
    ) -> Self {
        Self {
            relay,
            _id: PhantomData,
        }
    }

    /// Load a raw value by its backend key.
    pub async fn load(&self, key: Bytes) -> Result<Option<Bytes>, StorageApiError> {
        let (message, receiver) = StorageMsg::new_load_message(key);
        self.relay.send(message).await.map_err(|(relay_error, _)| {
            StorageApiError::CommsFailure(format!("{relay_error} while sending Load"))
        })?;
        receiver.into_inner().await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!("{relay_error} while receiving Load response"))
        })
    }

    pub async fn get_block(
        &self,
        header_id: HeaderId,
    ) -> Result<Option<Backend::Block>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::get_block_request(header_id, response_tx))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!("{relay_error} while sending GetBlock"))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving GetBlock response"
            ))
        })
    }

    pub async fn store_block_data(
        &self,
        header_id: HeaderId,
        parent_id: HeaderId,
        block: Backend::Block,
        events: Backend::Events,
        immutable_ids: BTreeMap<Slot, HeaderId>,
    ) -> Result<(), StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::store_block_data_request(
                header_id,
                parent_id,
                block,
                events,
                immutable_ids,
                response_tx,
            ))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!("{relay_error} while sending StoreBlockData"))
            })?;
        response_rx
            .await
            .map_err(|relay_error| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while receiving StoreBlockData response"
                ))
            })?
            .map_err(StorageApiError::Backend)
    }

    pub async fn remove_block(
        &self,
        header_id: HeaderId,
    ) -> Result<Option<Backend::Block>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::remove_block_request(header_id, response_tx))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!("{relay_error} while sending RemoveBlock"))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving RemoveBlock response"
            ))
        })
    }

    pub async fn get_block_parent(
        &self,
        header_id: HeaderId,
    ) -> Result<Option<HeaderId>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::get_block_parent_request(header_id, response_tx))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!("{relay_error} while sending GetBlockParent"))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving GetBlockParent response"
            ))
        })
    }

    pub async fn get_block_events(
        &self,
        header_id: HeaderId,
    ) -> Result<Option<Backend::Events>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::get_block_events_request(header_id, response_tx))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!("{relay_error} while sending GetBlockEvents"))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving GetBlockEvents response"
            ))
        })
    }

    pub async fn store_immutable_block_ids(
        &self,
        ids: BTreeMap<Slot, HeaderId>,
    ) -> Result<(), StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::store_immutable_block_ids_request(
                ids,
                response_tx,
            ))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending StoreImmutableBlockIds"
                ))
            })?;
        response_rx
            .await
            .map_err(|relay_error| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while receiving StoreImmutableBlockIds response"
                ))
            })?
            .map_err(StorageApiError::Backend)
    }

    pub async fn get_immutable_block_id(
        &self,
        slot: Slot,
    ) -> Result<Option<HeaderId>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::get_immutable_block_id_request(
                slot,
                response_tx,
            ))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending GetImmutableBlockId"
                ))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving GetImmutableBlockId response"
            ))
        })
    }

    pub async fn scan_immutable_block_ids(
        &self,
        slot_range: RangeInclusive<Slot>,
        limit: NonZeroUsize,
    ) -> Result<Vec<HeaderId>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::scan_immutable_block_ids_request(
                slot_range,
                limit,
                response_tx,
            ))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending ScanImmutableBlockIds"
                ))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving ScanImmutableBlockIds response"
            ))
        })
    }

    pub async fn scan_immutable_block_ids_reverse(
        &self,
        slot_range: RangeInclusive<Slot>,
        limit: NonZeroUsize,
    ) -> Result<Vec<HeaderId>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::Api {
                request: StorageApiRequest::Chain(ChainApiRequest::ScanImmutableBlockIdsReverse {
                    slot_range,
                    limit,
                    response_tx,
                }),
            })
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending ScanImmutableBlockIdsReverse"
                ))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving ScanImmutableBlockIdsReverse response"
            ))
        })
    }

    pub async fn store_transactions(
        &self,
        transactions: HashMap<TxHash, Backend::Tx>,
    ) -> Result<(), StorageApiError> {
        self.relay
            .send(StorageMsg::store_transactions_request(transactions))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending StoreTransactions"
                ))
            })
    }

    pub async fn get_transactions(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Pin<Box<dyn Stream<Item = Backend::Tx> + Send>>, StorageApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.relay
            .send(StorageMsg::get_transactions_request(tx_hashes, response_tx))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending GetTransactions"
                ))
            })?;
        response_rx.await.map_err(|relay_error| {
            StorageApiError::CommsFailure(format!(
                "{relay_error} while receiving GetTransactions response"
            ))
        })
    }

    pub async fn remove_transactions(&self, tx_hashes: Vec<TxHash>) -> Result<(), StorageApiError> {
        self.relay
            .send(StorageMsg::remove_transactions_request(tx_hashes))
            .await
            .map_err(|(relay_error, _)| {
                StorageApiError::CommsFailure(format!(
                    "{relay_error} while sending RemoveTransactions"
                ))
            })
    }
}

pub(crate) trait StorageOperation<Backend: StorageBackend> {
    async fn execute(self, api: &mut Backend) -> Result<(), StorageServiceError>;
}

pub enum StorageApiRequest<Backend: StorageBackend> {
    Chain(ChainApiRequest<Backend>),
}

impl<Backend: StorageBackend> StorageOperation<Backend> for StorageApiRequest<Backend> {
    async fn execute(self, backend: &mut Backend) -> Result<(), StorageServiceError> {
        match self {
            Self::Chain(request) => request.execute(backend).await,
        }
    }
}

use std::pin::Pin;

use futures::Stream;
use lb_core::mantle::transactions::hash::PrefixedKey;
use overwatch::services::relay::OutboundRelay;
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};

use crate::{MempoolError, MempoolMetrics, MempoolMsg, TxsWithCommonPrefix};

/// Errors returned by the typed mempool-service API.
#[derive(Debug, Error)]
pub enum MempoolApiError {
    #[error("Failed to establish connection to mempool service: {0}")]
    CommsFailure(String),
    #[error("Mempool request failed: {0}")]
    Mempool(#[from] MempoolError),
}

/// Typed wrapper over a mempool service relay.
pub struct MempoolServiceApi<BlockId, Payload, Item, Key>
where
    Key: PrefixedKey,
{
    relay: OutboundRelay<MempoolMsg<BlockId, Payload, Item, Key>>,
}

impl<BlockId, Payload, Item, Key> Clone for MempoolServiceApi<BlockId, Payload, Item, Key>
where
    Key: PrefixedKey,
{
    fn clone(&self) -> Self {
        Self {
            relay: self.relay.clone(),
        }
    }
}

impl<BlockId, Payload, Item, Key> MempoolServiceApi<BlockId, Payload, Item, Key>
where
    BlockId: Send,
    Payload: Send,
    Item: Send,
    Key: PrefixedKey + Send,
    Key::Prefix: Send,
{
    #[must_use]
    pub const fn new(relay: OutboundRelay<MempoolMsg<BlockId, Payload, Item, Key>>) -> Self {
        Self { relay }
    }

    /// Add an item to the mempool and wait for the service's validation result.
    pub async fn add(&self, key: Key, payload: Payload) -> Result<(), MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::Add {
                payload,
                key,
                reply_channel,
            })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!("{relay_error} while sending Add"))
            })?;

        receiver
            .await
            .map_err(|relay_error| {
                MempoolApiError::CommsFailure(format!("{relay_error} while receiving Add response"))
            })?
            .map_err(MempoolApiError::Mempool)
    }

    /// Return the mempool view selected by the service for an ancestor hint.
    pub async fn view(
        &self,
        ancestor_hint: BlockId,
    ) -> Result<Pin<Box<dyn Stream<Item = Item> + Send>>, MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::View {
                ancestor_hint,
                reply_channel,
            })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!("{relay_error} while sending View"))
            })?;

        receiver.await.map_err(|relay_error| {
            MempoolApiError::CommsFailure(format!("{relay_error} while receiving View response"))
        })
    }

    /// Remove items from the mempool.
    pub async fn remove(&self, ids: Vec<Key>) -> Result<(), MempoolApiError> {
        self.relay
            .send(MempoolMsg::Remove { ids })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!("{relay_error} while sending Remove"))
            })
    }

    /// Return transactions whose hashes share a proposal prefix.
    pub async fn get_transactions_by_prefix(
        &self,
        prefix: Key::Prefix,
    ) -> Result<TxsWithCommonPrefix<Item>, MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::GetTransactionsByPrefix {
                prefix,
                reply_channel,
            })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!(
                    "{relay_error} while sending GetTransactionsByPrefix"
                ))
            })?;

        receiver
            .await
            .map_err(|relay_error| {
                MempoolApiError::CommsFailure(format!(
                    "{relay_error} while receiving GetTransactionsByPrefix response"
                ))
            })?
            .map_err(MempoolApiError::Mempool)
    }

    /// Subscribe to items accepted by the mempool.
    pub async fn subscribe_to_accepted(
        &self,
    ) -> Result<broadcast::Receiver<Item>, MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::SubscribeToAccepted { reply_channel })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!(
                    "{relay_error} while sending SubscribeToAccepted"
                ))
            })?;

        receiver.await.map_err(|relay_error| {
            MempoolApiError::CommsFailure(format!(
                "{relay_error} while receiving SubscribeToAccepted response"
            ))
        })
    }

    /// Return aggregate mempool metrics.
    pub async fn metrics(&self) -> Result<MempoolMetrics, MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::Metrics { reply_channel })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!("{relay_error} while sending Metrics"))
            })?;

        receiver.await.map_err(|relay_error| {
            MempoolApiError::CommsFailure(format!("{relay_error} while receiving Metrics response"))
        })
    }

    /// Return status for a set of item keys.
    pub async fn status(
        &self,
        items: Vec<Key>,
    ) -> Result<Vec<crate::backend::Status>, MempoolApiError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.relay
            .send(MempoolMsg::Status {
                items,
                reply_channel,
            })
            .await
            .map_err(|(relay_error, _)| {
                MempoolApiError::CommsFailure(format!("{relay_error} while sending Status"))
            })?;

        receiver.await.map_err(|relay_error| {
            MempoolApiError::CommsFailure(format!("{relay_error} while receiving Status response"))
        })
    }
}

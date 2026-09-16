use std::marker::PhantomData;

use overwatch::services::relay::OutboundRelay;
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};

use crate::{BlockBroadcastMsg, BlockBroadcastService, BlockInfo};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Failed to establish connection to block-broadcast service: {0}")]
    CommsFailure(String),
}

pub struct BlockBroadcastServiceApi<RuntimeServiceId> {
    relay: OutboundRelay<BlockBroadcastMsg>,
    _id: PhantomData<fn() -> RuntimeServiceId>,
}

impl<RuntimeServiceId> Clone for BlockBroadcastServiceApi<RuntimeServiceId> {
    fn clone(&self) -> Self {
        Self {
            relay: self.relay.clone(),
            _id: PhantomData,
        }
    }
}

impl<RuntimeServiceId> BlockBroadcastServiceApi<RuntimeServiceId> {
    #[must_use]
    pub const fn new(
        relay: OutboundRelay<
            <BlockBroadcastService<RuntimeServiceId> as overwatch::services::ServiceData>::Message,
        >,
    ) -> Self {
        Self {
            relay,
            _id: PhantomData,
        }
    }

    pub async fn subscribe_to_finalized_blocks(
        &self,
    ) -> Result<broadcast::Receiver<BlockInfo>, ApiError> {
        let (result_sender, receiver) = oneshot::channel();
        self.relay
            .send(BlockBroadcastMsg::SubscribeToFinalizedBlocks { result_sender })
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!(
                    "{relay_error} while sending SubscribeToFinalizedBlocks"
                ))
            })?;

        receiver.await.map_err(|relay_error| {
            ApiError::CommsFailure(format!(
                "{relay_error} while receiving SubscribeToFinalizedBlocks response"
            ))
        })
    }
}

use std::{
    fmt::{self, Display, Formatter},
    marker::PhantomData,
};

use overwatch::services::{ServiceData, relay::OutboundRelay};
use tokio::sync::oneshot;
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    NetworkService,
    backends::NetworkBackend,
    message::{BackendNetworkMsg, NetworkMsg},
};

/// Errors returned by the typed network-service API.
#[derive(Debug)]
pub enum ApiError {
    CommsFailure(String),
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommsFailure(message) => {
                write!(
                    f,
                    "Failed to establish connection to network service: {message}"
                )
            }
        }
    }
}

impl std::error::Error for ApiError {}

/// Typed wrapper over a network service relay.
///
/// Backend-specific commands remain the responsibility of the caller, while
/// relay framing and subscription handshakes stay in one service-owned API.
pub struct NetworkServiceApi<Backend, RuntimeServiceId>
where
    Backend: NetworkBackend<RuntimeServiceId> + 'static,
{
    relay: OutboundRelay<BackendNetworkMsg<Backend, RuntimeServiceId>>,
    _id: PhantomData<fn() -> RuntimeServiceId>,
}

impl<Backend, RuntimeServiceId> Clone for NetworkServiceApi<Backend, RuntimeServiceId>
where
    Backend: NetworkBackend<RuntimeServiceId> + 'static,
{
    fn clone(&self) -> Self {
        Self {
            relay: self.relay.clone(),
            _id: PhantomData,
        }
    }
}

impl<Backend, RuntimeServiceId> NetworkServiceApi<Backend, RuntimeServiceId>
where
    Backend: NetworkBackend<RuntimeServiceId> + 'static,
{
    #[must_use]
    pub const fn new(
        relay: OutboundRelay<<NetworkService<Backend, RuntimeServiceId> as ServiceData>::Message>,
    ) -> Self {
        Self {
            relay,
            _id: PhantomData,
        }
    }

    /// Send a backend command through the network service.
    pub async fn process(&self, message: Backend::Message) -> Result<(), ApiError> {
        self.relay
            .send(NetworkMsg::Process(message))
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!("{relay_error} while processing network command"))
            })
    }

    /// Subscribe to events emitted by the backend's pub-sub transport.
    pub async fn subscribe_to_pubsub(
        &self,
    ) -> Result<BroadcastStream<Backend::PubSubEvent>, ApiError> {
        let (sender, receiver) = oneshot::channel();
        self.relay
            .send(NetworkMsg::SubscribeToPubSub { sender })
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!(
                    "{relay_error} while subscribing to network pub-sub events"
                ))
            })?;

        receiver.await.map_err(|relay_error| {
            ApiError::CommsFailure(format!(
                "{relay_error} while receiving network pub-sub subscription"
            ))
        })
    }

    /// Subscribe to events emitted by the backend's chain-sync transport.
    pub async fn subscribe_to_chainsync(
        &self,
    ) -> Result<BroadcastStream<Backend::ChainSyncEvent>, ApiError> {
        let (sender, receiver) = oneshot::channel();
        self.relay
            .send(NetworkMsg::SubscribeToChainSync { sender })
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!(
                    "{relay_error} while subscribing to network chain-sync events"
                ))
            })?;

        receiver.await.map_err(|relay_error| {
            ApiError::CommsFailure(format!(
                "{relay_error} while receiving network chain-sync subscription"
            ))
        })
    }
}

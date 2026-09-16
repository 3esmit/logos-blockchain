use std::marker::PhantomData;

use lb_core::{
    mantle::NoteId,
    sdp::{DeclarationId, Locator},
};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::{
    ServiceComponents,
    message::{DataPayload, NetworkInfo, ProxyServiceMessage, ServiceMessage},
};

/// Marker trait for the top-level blend service, used to parametrize
/// [`BlendServiceApi`] over the concrete blend service type while pinning its
/// message type.
pub trait BlendServiceData:
    ServiceData<Message = ProxyServiceMessage<ServiceMessage<<Self as ServiceComponents>::NodeId>>>
    + ServiceComponents
    + Send
    + 'static
{
}
impl<T> BlendServiceData for T where
    T: ServiceData<Message = ProxyServiceMessage<ServiceMessage<<T as ServiceComponents>::NodeId>>>
        + ServiceComponents
        + Send
        + 'static
{
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Failed to establish connection to blend-service: {0}")]
    CommsFailure(String),
}

/// Typed wrapper over the blend service relay, exposing the blend queries and
/// the payload-publishing entry point as async methods instead of raw
/// [`ProxyServiceMessage`]s.
pub struct BlendServiceApi<Blend, RuntimeServiceId>
where
    Blend: BlendServiceData,
{
    relay: OutboundRelay<Blend::Message>,
    _id: PhantomData<RuntimeServiceId>,
}

impl<Blend, RuntimeServiceId> Clone for BlendServiceApi<Blend, RuntimeServiceId>
where
    Blend: BlendServiceData,
{
    fn clone(&self) -> Self {
        Self {
            relay: self.relay.clone(),
            _id: PhantomData,
        }
    }
}

impl<Blend, RuntimeServiceId> BlendServiceApi<Blend, RuntimeServiceId>
where
    Blend: BlendServiceData,
{
    #[must_use]
    pub const fn new(relay: OutboundRelay<Blend::Message>) -> Self {
        Self {
            relay,
            _id: PhantomData,
        }
    }
}

impl<Blend, RuntimeServiceId> BlendServiceApi<Blend, RuntimeServiceId>
where
    Blend: BlendServiceData,
    Blend::NodeId: Send,
    RuntimeServiceId: Sync,
{
    /// Publish a payload to the blend network. The exit node hands it over to
    /// whichever local service owns that kind of payload. Fire-and-forget.
    pub async fn publish(&self, payload: DataPayload) -> Result<(), ApiError> {
        self.relay
            .send(ServiceMessage::Blend(payload).into())
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!("{relay_error} while sending Blend"))
            })
    }

    pub async fn network_info(&self) -> Result<Option<NetworkInfo<Blend::NodeId>>, ApiError> {
        let (reply, receiver) = oneshot::channel();
        self.relay
            .send(ServiceMessage::GetNetworkInfo { reply }.into())
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!("{relay_error} while sending GetNetworkInfo"))
            })?;

        receiver.await.map_err(|relay_error| {
            ApiError::CommsFailure(format!(
                "{relay_error} while receiving GetNetworkInfo response"
            ))
        })
    }

    pub async fn join_as_core(
        &self,
        locator: Locator,
        service_note_id: NoteId,
    ) -> Result<DeclarationId, ApiError> {
        let (reply, receiver) = oneshot::channel();
        self.relay
            .send(ProxyServiceMessage::JoinAsCore {
                locator,
                service_note_id,
                reply,
            })
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!("{relay_error} while sending JoinAsCore"))
            })?;

        receiver
            .await
            .map_err(|relay_error| {
                ApiError::CommsFailure(format!("{relay_error} while receiving JoinAsCore response"))
            })?
            .map_err(|error| ApiError::CommsFailure(error.to_string()))
    }

    pub async fn pending_transactions(&self) -> Result<Vec<Vec<u8>>, ApiError> {
        let (reply, receiver) = oneshot::channel();
        self.relay
            .send(ServiceMessage::GetPendingTransactions { reply }.into())
            .await
            .map_err(|(relay_error, _)| {
                ApiError::CommsFailure(format!(
                    "{relay_error} while sending GetPendingTransactions"
                ))
            })?;

        receiver.await.map_err(|relay_error| {
            ApiError::CommsFailure(format!(
                "{relay_error} while receiving GetPendingTransactions response"
            ))
        })
    }
}

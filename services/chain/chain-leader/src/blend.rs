#![cfg_attr(
    feature = "testing-disable-proposal-publish",
    allow(
        dead_code,
        reason = "with proposal publishing disabled for testing, some functions and struct fields are unused"
    )
)]

use std::marker::PhantomData;

use lb_blend_service::{api::BlendServiceApi, message::DataPayload};
use lb_codec::BinaryEncode as _;
use lb_core::block::Proposal;
use lb_log_targets::chain;
use overwatch::services::{ServiceData, relay::OutboundRelay};
use tracing::error;

const LOG_TARGET: &str = chain::leader::BLEND;

pub struct BlendAdapter<BlendService, RuntimeServiceId>
where
    BlendService: lb_blend_service::api::BlendServiceData,
{
    api: BlendServiceApi<BlendService, RuntimeServiceId>,
    // Service and runtime IDs are type-level tags; the adapter is held by
    // shared reference across awaits in the leader run loop.
    _phantom: PhantomData<fn() -> (BlendService, RuntimeServiceId)>,
}

impl<BlendService, RuntimeServiceId> BlendAdapter<BlendService, RuntimeServiceId>
where
    BlendService: lb_blend_service::api::BlendServiceData,
{
    pub const fn new(relay: OutboundRelay<<BlendService as ServiceData>::Message>) -> Self {
        Self {
            api: BlendServiceApi::new(relay),
            _phantom: PhantomData,
        }
    }
}

impl<BlendService, RuntimeServiceId> BlendAdapter<BlendService, RuntimeServiceId>
where
    BlendService: lb_blend_service::api::BlendServiceData,
    BlendService::NodeId: Send,
    RuntimeServiceId: Sync,
{
    pub async fn publish_proposal(&self, proposal: Proposal) {
        if let Err(e) = self
            .api
            .publish(DataPayload::BlockProposal(proposal.encode_to_vec()))
            .await
        {
            error!(target: LOG_TARGET, "Failed to relay proposal to blend service: {e:?}");
        }
    }
}

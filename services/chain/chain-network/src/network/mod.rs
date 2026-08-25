pub mod adapters;

use std::collections::HashSet;

use futures::Stream;
use lb_core::header::HeaderId;
use lb_cryptarchia_sync::GetTipResponse;
use lb_network_service::{NetworkService, backends::NetworkBackend, message::ChainSyncEvent};
use overwatch::{
    DynError,
    services::{ServiceData, relay::OutboundRelay},
};

pub(crate) type BoxedStream<T> = Box<dyn Stream<Item = T> + Send + Unpin>;

#[async_trait::async_trait]
pub trait NetworkAdapter<RuntimeServiceId> {
    type Backend: NetworkBackend<RuntimeServiceId> + 'static;
    type Settings: Clone + 'static;
    type PeerId;
    type Block;
    type Proposal;

    async fn new(
        settings: Self::Settings,
        network_relay: OutboundRelay<
            <NetworkService<Self::Backend, RuntimeServiceId> as ServiceData>::Message,
        >,
    ) -> Self;
    async fn proposals_stream(&self) -> Result<BoxedStream<Self::Proposal>, DynError>;

    async fn chainsync_events_stream(&self) -> Result<BoxedStream<ChainSyncEvent>, DynError>;

    async fn request_tip(&self, peer: Self::PeerId) -> Result<GetTipResponse, DynError>;

    /// Identities already learned from configured peerless initial addresses.
    ///
    /// This snapshot is non-blocking. Call [`Self::wait_for_initial_peers`]
    /// when IBD must wait for another identity or for all initial dials to
    /// settle.
    fn resolved_initial_peers(&self) -> HashSet<Self::PeerId> {
        HashSet::new()
    }

    /// Wait until the network backend resolves an initial address to a peer
    /// not present in `known_peers`, or until every initial-address dial has
    /// exhausted its retry lifecycle.
    ///
    /// The default implementation returns the current snapshot immediately;
    /// backends with peerless initial addresses must override this method.
    async fn wait_for_initial_peers(
        &self,
        _known_peers: &HashSet<Self::PeerId>,
    ) -> Result<HashSet<Self::PeerId>, DynError> {
        Ok(self.resolved_initial_peers())
    }

    /// Sample up to `max_peers` currently-connected peers and request their
    /// chain tip via `GetTip`, concurrently. The returned stream yields each
    /// successful response as it resolves; per-peer failures are dropped.
    ///
    /// Used by the proactive tip-polling lag watchdog.
    async fn sample_tips(&self, max_peers: usize) -> BoxedStream<GetTipResponse>;

    async fn request_blocks_from_peer(
        &self,
        peer: Self::PeerId,
        target_block: HeaderId,
        local_tip: HeaderId,
        latest_immutable_block: HeaderId,
        additional_blocks: HashSet<HeaderId>,
    ) -> Result<BoxedStream<Result<(HeaderId, Self::Block), DynError>>, DynError>;

    async fn request_blocks_from_peers(
        &self,
        target_block: HeaderId,
        local_tip: HeaderId,
        latest_immutable_block: HeaderId,
        additional_blocks: HashSet<HeaderId>,
    ) -> Result<BoxedStream<Result<(HeaderId, Self::Block), DynError>>, DynError>;

    /// Requests blocks while allowing peers that advertised the target to be
    /// included as extra candidates.
    ///
    /// The default implementation delegates to
    /// [`Self::request_blocks_from_peers`]; source-aware adapters should
    /// override it.
    async fn request_blocks_from_preferred_peers(
        &self,
        preferred_peers: HashSet<Self::PeerId>,
        target_block: HeaderId,
        local_tip: HeaderId,
        latest_immutable_block: HeaderId,
        additional_blocks: HashSet<HeaderId>,
    ) -> Result<BoxedStream<Result<(HeaderId, Self::Block), DynError>>, DynError>
    where
        Self::PeerId: Send + 'static,
    {
        drop(preferred_peers);
        self.request_blocks_from_peers(
            target_block,
            local_tip,
            latest_immutable_block,
            additional_blocks,
        )
        .await
    }
}

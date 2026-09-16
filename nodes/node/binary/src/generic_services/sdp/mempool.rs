use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
};

use lb_core::{
    header::HeaderId,
    mantle::{
        SignedOps,
        ledger::verification_mode::StandardMode,
        traits::Hashable as _,
        transactions::{hash::TxHash, states::Preverified},
    },
};
use lb_sdp_service::mempool::{MempoolAdapterError, SdpMempoolAdapter as SdpMempoolAdapterTrait};
use lb_storage_service::StorageService;
use lb_tx_service::{
    MempoolMsg, TxMempoolService,
    api::{MempoolApiError, MempoolServiceApi},
    backend::{MemPool, RecoverableMempool},
    network::NetworkAdapter as MempoolNetworkAdapter,
    storage::MempoolStorageAdapter,
};
use overwatch::services::{AsServiceId, ServiceData, relay::OutboundRelay};
use serde::{Deserialize, Serialize};

type MempoolRelay<Item, Key> = OutboundRelay<MempoolMsg<HeaderId, Item, Item, Key>>;

pub struct SdpMempoolAdapter<MempoolNetAdapter, Mempool, RuntimeServiceId>
where
    Mempool: MemPool<BlockId = HeaderId, Key = TxHash>,
    MempoolNetAdapter: MempoolNetworkAdapter<RuntimeServiceId, Key = Mempool::Key>,
    Mempool::Item: Clone + Eq + Debug + 'static,
    Mempool::Key: Debug + 'static,
{
    pub mempool_relay: MempoolRelay<Mempool::Item, Mempool::Key>,
    mempool_api: MempoolServiceApi<HeaderId, Mempool::Item, Mempool::Item, Mempool::Key>,
    _phantom: PhantomData<(MempoolNetAdapter, RuntimeServiceId)>,
}

#[async_trait::async_trait]
impl<MempoolNetAdapter, Mempool, RuntimeServiceId> SdpMempoolAdapterTrait
    for SdpMempoolAdapter<MempoolNetAdapter, Mempool, RuntimeServiceId>
where
    Mempool: RecoverableMempool<
            BlockId = HeaderId,
            Key = TxHash,
            Item = SignedOps<Preverified, StandardMode>,
        > + Send
        + Sync,
    Mempool::RecoveryState: Serialize + for<'de> Deserialize<'de>,
    Mempool::Settings: Clone + Send + Sync,
    Mempool::Storage: MempoolStorageAdapter<RuntimeServiceId> + Send + Sync + Clone,
    MempoolNetAdapter: MempoolNetworkAdapter<RuntimeServiceId, Payload = Mempool::Item, Key = Mempool::Key>
        + Send
        + Sync,
    MempoolNetAdapter::Settings: Send + Sync,
    RuntimeServiceId: Clone
        + Debug
        + Display
        + Send
        + Sync
        + 'static
        + AsServiceId<
            StorageService<
                <Mempool::Storage as MempoolStorageAdapter<RuntimeServiceId>>::Backend,
                RuntimeServiceId,
            >,
        >,
{
    type MempoolService =
        TxMempoolService<MempoolNetAdapter, Mempool, Mempool::Storage, RuntimeServiceId>;
    type Tx = SignedOps<Preverified, StandardMode>;

    fn new(mempool_relay: OutboundRelay<<Self::MempoolService as ServiceData>::Message>) -> Self {
        Self {
            mempool_relay: mempool_relay.clone(),
            mempool_api: MempoolServiceApi::new(mempool_relay),
            _phantom: PhantomData,
        }
    }

    async fn post_tx(&self, tx: Self::Tx) -> Result<(), MempoolAdapterError> {
        self.mempool_api
            .add(tx.hash(), tx)
            .await
            .map_err(|error| match error {
                MempoolApiError::Mempool(error) => MempoolAdapterError::Mempool(Box::new(error)),
                MempoolApiError::CommsFailure(message) => {
                    MempoolAdapterError::Other(message.into())
                }
            })
    }
}

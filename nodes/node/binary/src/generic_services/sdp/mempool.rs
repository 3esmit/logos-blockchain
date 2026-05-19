use std::{fmt::Debug, marker::PhantomData};

use lb_core::{
    header::HeaderId,
    mantle::{SignedMantleTx, Transaction as _, TxHash},
};
use lb_sdp_service::mempool::{MempoolAdapterError, SdpMempoolAdapter as SdpMempoolAdapterTrait};
use lb_tx_service::{
    MempoolMsg, TxMempoolService,
    backend::{MemPool, RecoverableMempool},
    network::NetworkAdapter as MempoolNetworkAdapter,
    storage::MempoolStorageAdapter,
};
use overwatch::services::{ServiceData, relay::OutboundRelay};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

type MempoolRelay<Item, Key> = OutboundRelay<MempoolMsg<HeaderId, Item, Key>>;

pub struct SdpMempoolAdapter<MempoolNetAdapter, Mempool, ChainService, RuntimeServiceId>
where
    Mempool: MemPool<BlockId = HeaderId, TxHash = TxHash>,
    MempoolNetAdapter: MempoolNetworkAdapter<RuntimeServiceId, Key = Mempool::TxHash>,
    Mempool::Tx: Clone + Eq + Debug + 'static,
    Mempool::TxHash: Debug + 'static,
    ChainService: Send + Sync,
    RuntimeServiceId: Send + Sync,
{
    pub mempool_relay: MempoolRelay<Mempool::Tx, Mempool::TxHash>,
    _phantom: PhantomData<(MempoolNetAdapter, ChainService, RuntimeServiceId)>,
}

#[async_trait::async_trait]
impl<MempoolNetAdapter, Mempool, ChainService, RuntimeServiceId> SdpMempoolAdapterTrait
    for SdpMempoolAdapter<MempoolNetAdapter, Mempool, ChainService, RuntimeServiceId>
where
    Mempool:
        RecoverableMempool<BlockId = HeaderId, TxHash = TxHash, Tx = SignedMantleTx> + Send + Sync,
    Mempool::RecoveryState: Serialize + for<'de> Deserialize<'de>,
    Mempool::Settings: Clone + Send + Sync,
    Mempool::Adapter: MempoolStorageAdapter<RuntimeServiceId> + Send + Sync + Clone,
    MempoolNetAdapter: MempoolNetworkAdapter<RuntimeServiceId, Payload = Mempool::Tx, Key = Mempool::TxHash>
        + Send
        + Sync,
    MempoolNetAdapter::Settings: Send + Sync,
    ChainService: Send + Sync + 'static,
    RuntimeServiceId: Send + Sync,
{
    type MempoolService = TxMempoolService<
        MempoolNetAdapter,
        Mempool,
        Mempool::Adapter,
        ChainService,
        RuntimeServiceId,
    >;
    type Tx = SignedMantleTx;

    fn new(mempool_relay: OutboundRelay<<Self::MempoolService as ServiceData>::Message>) -> Self {
        Self {
            mempool_relay,
            _phantom: PhantomData,
        }
    }

    async fn post_tx(&self, tx: Self::Tx) -> Result<(), MempoolAdapterError> {
        let (reply_channel, receiver) = oneshot::channel();
        self.mempool_relay
            .send(MempoolMsg::Add {
                key: tx.hash(),
                payload: tx,
                reply_channel,
            })
            .await
            .map_err(|(e, _)| MempoolAdapterError::Other(Box::new(e)))?;

        receiver
            .await?
            .map_err(|e| MempoolAdapterError::Mempool(Box::new(e)))
    }
}

use core::{fmt::Debug, hash::Hash};
use std::fmt::Display;

use lb_core::mantle::{Transaction, TransactionDependencies};
use lb_network_service::backends::NetworkBackend;
use lb_tx_service::{
    MempoolMsg, TxMempoolService,
    backend::{BlockInfoGetter, LedgerStateGetter, Mempool},
    network::NetworkAdapter,
};
use overwatch::{DynError, services::AsServiceId};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::oneshot;

pub async fn add_tx<
    MempoolNetworkBackend,
    MempoolNetworkAdapter,
    StorageAdapter,
    Item,
    Key,
    CryptarchiaService,
    RuntimeServiceId,
>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    item: Item,
    converter: impl Fn(&Item) -> Key,
) -> Result<(), DynError>
where
    MempoolNetworkBackend: NetworkBackend<RuntimeServiceId>,
    MempoolNetworkAdapter: NetworkAdapter<RuntimeServiceId, Backend = MempoolNetworkBackend, Payload = Item, Key = Key>
        + Send
        + Sync
        + 'static,
    MempoolNetworkAdapter::Settings: Send + Sync,
    StorageAdapter: lb_tx_service::storage::MempoolStorageAdapter<RuntimeServiceId, Tx = Item>
        + BlockInfoGetter<Item>
        + LedgerStateGetter
        + Clone
        + 'static,
    StorageAdapter::Error: Debug,
    Item: TransactionDependencies
        + Transaction<Hash = Key>
        + Clone
        + Debug
        + Send
        + Sync
        + Serialize
        + DeserializeOwned
        + 'static,
    Key: Clone + Debug + Eq + Ord + Hash + Send + Sync + Serialize + DeserializeOwned + 'static,
    RuntimeServiceId: Debug
        + Sync
        + Send
        + Display
        + AsServiceId<
            TxMempoolService<
                MempoolNetworkAdapter,
                Mempool<Item, Key, StorageAdapter, RuntimeServiceId>,
                StorageAdapter,
                CryptarchiaService,
                RuntimeServiceId,
            >,
        >,
{
    let relay = handle.relay().await?;
    let (sender, receiver) = oneshot::channel();

    relay
        .send(MempoolMsg::Add {
            key: converter(&item),
            payload: item,
            reply_channel: sender,
        })
        .await
        .map_err(|(e, _)| e)?;

    receiver
        .await
        .map_err(|_| DynError::from("Failed to add tx"))?
        .map_err(DynError::from)
}

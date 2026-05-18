/// Re-export for `OpenAPI`
#[cfg(feature = "openapi")]
pub mod openapi {
    pub use crate::backend::Status;
}

use std::{
    collections::BTreeSet,
    fmt::{Debug, Display},
    marker::PhantomData,
    pin::Pin,
    time::Duration,
};

use futures::StreamExt as _;
use lb_chain_service::{
    Cryptarchia, CryptarchiaConsensus,
    api::{CryptarchiaServiceApi, CryptarchiaServiceData},
};
use lb_core::mantle::{AuthenticatedMantleTx, Transaction};
use lb_network_service::{NetworkService, message::BackendNetworkMsg};
use lb_services_utils::{
    overwatch::{
        JsonFileBackend, RecoveryOperator,
        recovery::operators::RecoveryBackend as RecoveryBackendTrait,
    },
    wait_until_services_are_ready,
};
use lb_storage_service::StorageService;
use overwatch::{
    OpaqueServiceResourcesHandle,
    services::{AsServiceId, ServiceCore, ServiceData, relay::OutboundRelay},
};
use tokio::sync::oneshot;

use crate::{
    MempoolMetrics, MempoolMsg, TransactionsByHashesResponse, backend,
    backend::{MemPool as MemPoolTrait, MempoolError, RecoverableMempool, adapter::TrackerAdapter},
    network::NetworkAdapter as NetworkAdapterTrait,
    storage::MempoolStorageAdapter,
    tx::{settings::TxMempoolSettings, state::TxMempoolState},
};

type MempoolStateUpdater<Pool, NetworkAdapter, RuntimeServiceId> =
    overwatch::services::state::StateUpdater<
        Option<
            TxMempoolState<
                <Pool as RecoverableMempool>::RecoveryState,
                <Pool as MemPoolTrait>::Settings,
                <NetworkAdapter as NetworkAdapterTrait<RuntimeServiceId>>::Settings,
            >,
        >,
    >;

type TxMempoolRecoveryState<Pool, NetworkAdapter, RuntimeServiceId> = TxMempoolState<
    <Pool as RecoverableMempool>::RecoveryState,
    <Pool as MemPoolTrait>::Settings,
    <NetworkAdapter as NetworkAdapterTrait<RuntimeServiceId>>::Settings,
>;

type TxMempoolRecoverySettings<Pool, NetworkAdapter, RuntimeServiceId> = TxMempoolSettings<
    <Pool as MemPoolTrait>::Settings,
    <NetworkAdapter as NetworkAdapterTrait<RuntimeServiceId>>::Settings,
>;

type TxMempoolRecoveryBackend<Pool, NetworkAdapter, RuntimeServiceId> = JsonFileBackend<
    TxMempoolRecoveryState<Pool, NetworkAdapter, RuntimeServiceId>,
    TxMempoolRecoverySettings<Pool, NetworkAdapter, RuntimeServiceId>,
>;

/// A tx mempool service that uses a [`JsonFileBackend`] as a recovery
/// mechanism.
pub type TxMempoolService<
    MempoolNetworkAdapter,
    Pool,
    StorageAdapter,
    Cryptarchia,
    RuntimeServiceId,
> = GenericTxMempoolService<
    Pool,
    MempoolNetworkAdapter,
    TxMempoolRecoveryBackend<Pool, MempoolNetworkAdapter, RuntimeServiceId>,
    StorageAdapter,
    Cryptarchia,
    RuntimeServiceId,
>;

/// A generic tx mempool service which wraps around a mempool, a network
/// adapter, and a recovery backend.
pub struct GenericTxMempoolService<
    Pool,
    NetworkAdapter,
    RecoveryBackend,
    Adapter,
    ChainService,
    RuntimeServiceId,
> where
    Pool: MemPoolTrait<Adapter = Adapter> + RecoverableMempool + Send + Sync,
    Adapter: MempoolStorageAdapter<RuntimeServiceId> + Clone + Send + Sync,
    <Pool as MemPoolTrait>::Settings: Clone,
    NetworkAdapter: NetworkAdapterTrait<RuntimeServiceId> + Send + Sync,
    NetworkAdapter::Settings: Clone,
    RecoveryBackend: RecoveryBackendTrait + Send + Sync,
{
    service_resources_handle: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
    initial_state: <Self as ServiceData>::State,
    _phantom: PhantomData<(Pool, NetworkAdapter, RecoveryBackend, Adapter, ChainService)>,
}

impl<Pool, NetworkAdapter, RecoveryBackend, Adapter, ChainService, RuntimeServiceId>
    GenericTxMempoolService<
        Pool,
        NetworkAdapter,
        RecoveryBackend,
        Adapter,
        ChainService,
        RuntimeServiceId,
    >
where
    Pool: MemPoolTrait<Adapter = Adapter> + RecoverableMempool + Send + Sync,
    Adapter: MempoolStorageAdapter<RuntimeServiceId> + Clone + Send + Sync,
    <Pool as MemPoolTrait>::Settings: Clone,
    NetworkAdapter: NetworkAdapterTrait<RuntimeServiceId> + Send + Sync,
    NetworkAdapter::Settings: Clone,
    RecoveryBackend: RecoveryBackendTrait + Send + Sync,
{
    pub const fn new(
        service_resources_handle: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
        initial_state: <Self as ServiceData>::State,
    ) -> Self {
        Self {
            service_resources_handle,
            initial_state,
            _phantom: PhantomData,
        }
    }
}

impl<Pool, NetworkAdapter, RecoveryBackend, Adapter, ChainService, RuntimeServiceId> ServiceData
    for GenericTxMempoolService<
        Pool,
        NetworkAdapter,
        RecoveryBackend,
        Adapter,
        ChainService,
        RuntimeServiceId,
    >
where
    Pool: MemPoolTrait<Adapter = Adapter> + RecoverableMempool + Send + Sync,
    Adapter: MempoolStorageAdapter<RuntimeServiceId> + Clone + Send + Sync,
    <Pool as MemPoolTrait>::Settings: Clone,
    NetworkAdapter: NetworkAdapterTrait<RuntimeServiceId> + Send + Sync,
    NetworkAdapter::Settings: Clone,
    RecoveryBackend: RecoveryBackendTrait + Send + Sync,
{
    type Settings = TxMempoolSettings<<Pool as MemPoolTrait>::Settings, NetworkAdapter::Settings>;
    type State = TxMempoolState<
        <Pool as RecoverableMempool>::RecoveryState,
        <Pool as MemPoolTrait>::Settings,
        NetworkAdapter::Settings,
    >;
    type StateOperator = RecoveryOperator<RecoveryBackend>;
    type Message = MempoolMsg<Pool::BlockId, Pool::Tx, Pool::TxHash>;
}

#[async_trait::async_trait]
impl<Pool, NetworkAdapter, RecoveryBackend, Adapter, ChainService, RuntimeServiceId>
    ServiceCore<RuntimeServiceId>
    for GenericTxMempoolService<
        Pool,
        NetworkAdapter,
        RecoveryBackend,
        Adapter,
        ChainService,
        RuntimeServiceId,
    >
where
    Pool: MemPoolTrait<Adapter = Adapter> + RecoverableMempool + Send + Sync,
    Adapter: MempoolStorageAdapter<RuntimeServiceId> + Clone + Send + Sync,
    <Pool as RecoverableMempool>::RecoveryState: Debug + Send + Sync,
    Pool::TxHash: Send + Sync + 'static,
    Pool::Tx: Transaction<Hash = Pool::TxHash> + Debug + Eq + Clone + Send + Sync + 'static,
    Pool::Settings: Clone + Sync + Send,
    NetworkAdapter:
        NetworkAdapterTrait<RuntimeServiceId, Payload = Pool::Tx, Key = Pool::TxHash> + Send + Sync,
    NetworkAdapter::Settings: Clone + Send + Sync + 'static,
    RecoveryBackend: RecoveryBackendTrait + Send + Sync,
    RuntimeServiceId: Display
        + Debug
        + Sync
        + Send
        + 'static
        + AsServiceId<Self>
        + AsServiceId<NetworkService<NetworkAdapter::Backend, RuntimeServiceId>>
        + AsServiceId<
            StorageService<
                <Adapter as MempoolStorageAdapter<RuntimeServiceId>>::Backend,
                RuntimeServiceId,
            >,
        >
        + AsServiceId<ChainService>,
    ChainService: CryptarchiaServiceData<Tx = Pool::Tx>,
{
    fn init(
        service_resources_handle: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
        initial_state: Self::State,
    ) -> Result<Self, overwatch::DynError> {
        tracing::trace!(
            "Initializing TxMempoolService with initial state {:#?}",
            initial_state.pool
        );
        Ok(Self::new(service_resources_handle, initial_state))
    }

    async fn run(mut self) -> Result<(), overwatch::DynError> {
        let settings_handle = &self.service_resources_handle.settings_handle;
        let settings = settings_handle.notifier().get_updated_settings();

        let overwatch_handle = &self.service_resources_handle.overwatch_handle;

        let storage_relay = overwatch_handle
            .relay::<StorageService<
                <Adapter as MempoolStorageAdapter<RuntimeServiceId>>::Backend,
                RuntimeServiceId,
            >>()
            .await
            .expect("Storage service relay should be available");

        let storage_adapter =
            <Adapter as MempoolStorageAdapter<RuntimeServiceId>>::new(storage_relay);

        let cryptarchia_api: CryptarchiaServiceApi<ChainService, RuntimeServiceId> =
            CryptarchiaServiceApi::new(
                overwatch_handle
                    .relay::<ChainService>()
                    .await
                    .expect("Cryptarchia service relay should be available"),
            );

        let adapter = TrackerAdapter::new(cryptarchia_api, storage_adapter.clone());

        let pool_state = self.initial_state.pool.take();

        let mut pool = match pool_state {
            None => <Pool as MemPoolTrait>::new(settings.pool.clone(), storage_adapter),
            Some(recovered_pool_state) => <Pool as RecoverableMempool>::recover(
                settings.pool.clone(),
                recovered_pool_state,
                storage_adapter,
            ),
        };

        let network_service_relay = overwatch_handle
            .relay::<NetworkService<_, _>>()
            .await
            .expect("Relay connection with NetworkService should succeed");

        // Queue for network messages
        let mut network_items = NetworkAdapter::new(
            settings_handle
                .notifier()
                .get_updated_settings()
                .network_adapter,
            network_service_relay.clone(),
        )
        .await
        .payload_stream()
        .await;

        self.service_resources_handle.status_updater.notify_ready();
        tracing::info!(
            "Service '{}' is ready.",
            <RuntimeServiceId as AsServiceId<Self>>::SERVICE_ID
        );

        wait_until_services_are_ready!(
            &overwatch_handle,
            Some(Duration::from_mins(1)),
            NetworkService<_, _>
        )
        .await?;

        self.run_event_loop(&mut pool, network_service_relay, &mut network_items)
            .await
    }
}

impl<Pool, NetworkAdapter, RecoveryBackend, Adapter, Cryptarchia, RuntimeServiceId>
    GenericTxMempoolService<
        Pool,
        NetworkAdapter,
        RecoveryBackend,
        Adapter,
        Cryptarchia,
        RuntimeServiceId,
    >
where
    Pool: MemPoolTrait<Adapter = Adapter> + RecoverableMempool + Send + Sync,
    Adapter: MempoolStorageAdapter<RuntimeServiceId> + Clone + Send + Sync,
    Pool::Tx: Transaction<Hash = Pool::TxHash> + Clone + Send + 'static,
    Pool::Settings: Clone,
    NetworkAdapter: NetworkAdapterTrait<RuntimeServiceId, Payload = Pool::Tx> + Send + Sync,
    NetworkAdapter::Settings: Clone + Send + 'static,
    RecoveryBackend: RecoveryBackendTrait + Send + Sync,
    RuntimeServiceId: 'static,
{
    async fn run_event_loop(
        &mut self,
        pool: &mut Pool,
        network_service_relay: OutboundRelay<
            BackendNetworkMsg<NetworkAdapter::Backend, RuntimeServiceId>,
        >,
        network_items: &mut Box<
            dyn futures::Stream<Item = (Pool::TxHash, Pool::Tx)> + Unpin + Send,
        >,
    ) -> Result<(), overwatch::DynError>
    where
        Pool::Settings: Send + Sync,
        NetworkAdapter::Settings: Send + Sync,
    {
        loop {
            tokio::select! {
                // Queue for relay messages
                Some(relay_msg) = self.service_resources_handle.inbound_relay.recv() => {
                    let state_updater = self.service_resources_handle.state_updater.clone();
                    let settings = self
                        .service_resources_handle
                        .settings_handle
                        .notifier()
                        .get_updated_settings()
                        .network_adapter;

                    Self::handle_mempool_message(pool, relay_msg, network_service_relay.clone(), state_updater, settings).await;
                }
                Some((_key, item)) = network_items.next() => {
                    Self::handle_network_item(pool, item, &self.service_resources_handle.state_updater).await;
                }
            }
        }
    }

    async fn handle_mempool_message(
        pool: &mut Pool,
        message: MempoolMsg<Pool::BlockId, Pool::Tx, Pool::TxHash>,
        network_relay: OutboundRelay<BackendNetworkMsg<NetworkAdapter::Backend, RuntimeServiceId>>,
        state_updater: MempoolStateUpdater<Pool, NetworkAdapter, RuntimeServiceId>,
        settings: NetworkAdapter::Settings,
    ) where
        Pool::Settings: Send + Sync,
        NetworkAdapter::Settings: Send + Sync,
    {
        match message {
            MempoolMsg::Add {
                payload,
                reply_channel,
                ..
            } => {
                Self::handle_add_message(
                    pool,
                    payload,
                    reply_channel,
                    network_relay,
                    state_updater,
                    settings,
                )
                .await;
            }
            MempoolMsg::View {
                ancestor_hint,
                reply_channel,
            } => {
                Self::handle_view_message(pool, ancestor_hint, reply_channel).await;
            }
            MempoolMsg::GetTransactionsByHashes {
                hashes,
                reply_channel,
            } => {
                let result = Self::partition_transactions_by_availability(pool, hashes).await;

                if let Err(_e) = reply_channel.send(result) {
                    tracing::debug!("Failed to send transactions reply");
                }
            }
            MempoolMsg::Remove { ids } => {
                pool.remove(&ids).await;
            }
            MempoolMsg::Metrics { reply_channel } => {
                Self::handle_metrics_message(pool, reply_channel);
            }
            MempoolMsg::Status {
                items,
                reply_channel,
            } => {
                Self::handle_status_message(pool, &items, reply_channel);
            }
        }
    }

    async fn handle_add_message(
        pool: &mut Pool,
        item: Pool::Tx,
        reply_channel: oneshot::Sender<Result<(), MempoolError>>,
        network_relay: OutboundRelay<BackendNetworkMsg<NetworkAdapter::Backend, RuntimeServiceId>>,
        state_updater: MempoolStateUpdater<Pool, NetworkAdapter, RuntimeServiceId>,
        settings: NetworkAdapter::Settings,
    ) where
        Pool::Settings: Send + Sync,
        NetworkAdapter::Settings: Send + Sync,
    {
        let item_for_broadcast = item.clone();

        match pool.add_item(item).await {
            Ok(_id) => {
                Self::handle_add_success(
                    pool,
                    &state_updater,
                    settings,
                    network_relay,
                    item_for_broadcast,
                    reply_channel,
                );
            }
            Err(MempoolError::ExistingItem) => {
                // Tx already in pool, but since this came from a local submission
                // (not gossip), re-gossip it so leader nodes can pick it up.
                tokio::spawn(async move {
                    let adapter = NetworkAdapter::new(settings, network_relay).await;
                    adapter.send(item_for_broadcast).await;
                });
                if let Err(e) = reply_channel.send(Ok(())) {
                    tracing::debug!("Failed to send add reply: {:?}", e);
                }
            }
            Err(e) => Self::handle_add_error(e, reply_channel),
        }
    }

    async fn handle_view_message(
        pool: &Pool,
        ancestor_hint: Pool::BlockId,
        reply_channel: oneshot::Sender<Pin<Box<dyn futures::Stream<Item = Pool::Tx> + Send>>>,
    ) {
        let pending_items = pool.pending_item_count();
        tracing::trace!(pending_items, "Handling mempool View message");

        let items = pool
            .view(ancestor_hint)
            .await
            .unwrap_or_else(|_| Box::pin(futures::stream::iter(Vec::new())));

        if let Err(_e) = reply_channel.send(Box::pin(items)) {
            tracing::debug!("Failed to send view reply");
        }
    }

    fn handle_metrics_message(pool: &Pool, reply_channel: oneshot::Sender<MempoolMetrics>) {
        let info = MempoolMetrics {
            pending_items: pool.pending_item_count(),
            last_item_timestamp: pool.last_item_timestamp(),
        };

        if let Err(_e) = reply_channel.send(info) {
            tracing::debug!("Failed to send metrics reply");
        }
    }

    fn handle_status_message(
        pool: &Pool,
        items: &[Pool::TxHash],
        reply_channel: oneshot::Sender<Vec<backend::Status>>,
    ) {
        let statuses = pool.status(items);

        if let Err(_e) = reply_channel.send(statuses) {
            tracing::debug!("Failed to send status reply");
        }
    }

    async fn partition_transactions_by_availability(
        pool: &Pool,
        hashes: Vec<Pool::TxHash>,
    ) -> Result<TransactionsByHashesResponse<Pool::Tx, Pool::TxHash>, MempoolError> {
        let keys_set: BTreeSet<Pool::TxHash> = hashes.into_iter().collect();

        let items_stream = pool
            .get_items_by_keys(keys_set.iter().cloned())
            .await
            .map_err(|e| {
                MempoolError::StorageError(format!("Failed to get items by keys: {e:?}"))
            })?;

        let found_transactions: Vec<Pool::Tx> = items_stream.collect().await;

        if found_transactions.len() == keys_set.len() {
            return Ok(TransactionsByHashesResponse::new(
                found_transactions,
                BTreeSet::new(),
            ));
        }

        let found_hashes: BTreeSet<Pool::TxHash> =
            found_transactions.iter().map(Transaction::hash).collect();

        let not_found_hashes: BTreeSet<Pool::TxHash> = &keys_set - &found_hashes;

        Ok(TransactionsByHashesResponse::new(
            found_transactions,
            not_found_hashes,
        ))
    }

    fn handle_add_success(
        pool: &Pool,
        state_updater: &MempoolStateUpdater<Pool, NetworkAdapter, RuntimeServiceId>,
        settings: NetworkAdapter::Settings,
        network_relay: OutboundRelay<BackendNetworkMsg<NetworkAdapter::Backend, RuntimeServiceId>>,
        item_for_broadcast: Pool::Tx,
        reply_channel: oneshot::Sender<Result<(), MempoolError>>,
    ) {
        state_updater.update(Some(<Pool as RecoverableMempool>::save(pool).into()));

        tokio::spawn(async move {
            let adapter = NetworkAdapter::new(settings, network_relay).await;
            adapter.send(item_for_broadcast).await;
        });

        if let Err(e) = reply_channel.send(Ok(())) {
            tracing::debug!("Failed to send add reply: {:?}", e);
        }
    }

    fn handle_add_error(
        error: MempoolError,
        reply_channel: oneshot::Sender<Result<(), MempoolError>>,
    ) {
        tracing::debug!("Could not add item to the pool: {}", error);
        if let Err(e) = reply_channel.send(Err(error)) {
            tracing::debug!("Failed to send error reply: {:?}", e);
        }
    }

    async fn handle_network_item(
        pool: &mut Pool,
        item: Pool::Tx,
        state_updater: &MempoolStateUpdater<Pool, NetworkAdapter, RuntimeServiceId>,
    ) where
        Pool::Settings: Send + Sync,
        NetworkAdapter::Settings: Send + Sync,
    {
        if let Err(e) = pool.add_item(item).await {
            tracing::debug!("could not add item to the pool due to: {e}");
            return;
        }

        tracing::trace!(counter.tx_mempool_pending_items = pool.pending_item_count());

        state_updater.update(Some(<Pool as RecoverableMempool>::save(pool).into()));
    }
}

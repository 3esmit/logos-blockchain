use std::{
    collections::{BTreeSet, HashSet},
    fmt::{Debug, Display},
    pin::Pin,
};

use async_trait::async_trait;
use futures::Stream;
use lb_chain_service::api::{CryptarchiaServiceApi, CryptarchiaServiceData};
use lb_core::{
    block::Block,
    header::HeaderId,
    mantle::{DependencyId, Transaction},
};
use lb_ledger::LedgerState;
use lb_storage_service::StorageService;
use overwatch::{DynError, overwatch::OverwatchHandle, services::AsServiceId};
use tracing::error;

use crate::{
    backend::{
        MempoolAdapter,
        forks::{BlockInfo, BlockInfoGetter, ForksTrackerError, LedgerStateGetter},
        inspector::LedgerStateInspector,
    },
    storage::{MempoolStorageAdapter, MempoolStorageAdapterNew},
};

pub struct TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
{
    storage: Storage,
    crypatarchia_api: CryptarchiaServiceApi<Cryptarchia, RuntimeServiceId>,
}

impl<Cryptarchia, Storage, RuntimeServiceId> Clone
    for TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
    Storage: Clone,
{
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            crypatarchia_api: self.crypatarchia_api.clone(),
        }
    }
}

impl<Cryptarchia, Storage, RuntimeServiceId> TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
{
    pub const fn new(
        crypatarchia_api: CryptarchiaServiceApi<Cryptarchia, RuntimeServiceId>,
        storage: Storage,
    ) -> Self {
        Self {
            storage,
            crypatarchia_api,
        }
    }
}

#[async_trait::async_trait]
impl<Cryptarchia, Storage, RuntimeServiceId>
    BlockInfoGetter<<Storage as MempoolStorageAdapter<RuntimeServiceId>>::Tx>
    for TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
    Cryptarchia::Tx: Send + Clone,
    Storage: MempoolStorageAdapter<RuntimeServiceId, Tx = Cryptarchia::Tx>,
    Storage::Error: Debug,
    RuntimeServiceId: Send + Sync,
{
    async fn get_block(
        &self,
        header_id: &HeaderId,
    ) -> Result<
        BlockInfo<<Storage as MempoolStorageAdapter<RuntimeServiceId>>::Tx>,
        ForksTrackerError,
    > {
        match self.storage.get_block(*header_id).await {
            Ok(Some(block)) => Ok(BlockInfo {
                parent: block.header().parent(),
                transactions: block.transactions().cloned().collect(),
            }),
            Ok(None) => Err(ForksTrackerError::ParentNotFound(*header_id)),
            Err(e) => {
                error!("{e:?}");
                Err(ForksTrackerError::ParentNotFound(*header_id))
            }
        }
    }
}

#[async_trait::async_trait]
impl<Cryptarchia, Storage, RuntimeServiceId> LedgerStateGetter
    for TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
    Cryptarchia::Tx: Send + Sync,
    Storage: MempoolStorageAdapter<RuntimeServiceId, Tx = Cryptarchia::Tx>,
    RuntimeServiceId: Send + Sync,
{
    async fn get_ledger_state(
        &self,
        header_id: HeaderId,
    ) -> Result<LedgerState, ForksTrackerError> {
        self.crypatarchia_api
            .get_ledger_state(header_id)
            .await
            .map_err(|_| ForksTrackerError::ParentNotFound(header_id))?
            .ok_or(ForksTrackerError::ParentNotFound(header_id))
    }

    async fn get_ledger_deps(
        &self,
        header_id: &HeaderId,
    ) -> Result<HashSet<DependencyId>, ForksTrackerError> {
        match self.crypatarchia_api.get_ledger_state(*header_id).await {
            Ok(Some(state)) => Ok(LedgerStateInspector::new(state).dependencies()),
            Ok(None) => Err(ForksTrackerError::ParentNotFound(*header_id)),
            Err(e) => {
                error!("{e:?}");
                Err(ForksTrackerError::ParentNotFound(*header_id))
            }
        }
    }
}

#[async_trait]
impl<Cryptarchia, Storage, RuntimeServiceId> MempoolStorageAdapter<RuntimeServiceId>
    for TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData + Send + Sync,
    Cryptarchia::Tx: Send + Sync,
    Storage: MempoolStorageAdapter<RuntimeServiceId> + Send + Sync,
    Storage::Tx: Send,
    <Storage::Tx as Transaction>::Hash: Sync,
    RuntimeServiceId: Send + Sync,
{
    type Backend = Storage::Backend;
    type Tx = Storage::Tx;
    type Error = Storage::Error;

    async fn store_tx(&mut self, tx: Self::Tx) -> Result<(), Self::Error> {
        self.storage.store_tx(tx).await
    }

    async fn get_tx(
        &self,
        keys: &BTreeSet<<Self::Tx as Transaction>::Hash>,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Tx> + Send>>, Self::Error> {
        self.storage.get_tx(keys).await
    }

    async fn remove_txs(
        &mut self,
        keys: &[<Self::Tx as Transaction>::Hash],
    ) -> Result<(), Self::Error> {
        self.storage.remove_txs(keys).await
    }

    async fn get_block(&self, header: HeaderId) -> Result<Option<Block<Self::Tx>>, Self::Error> {
        self.storage.get_block(header).await
    }
}

#[async_trait::async_trait]
impl<Cryptarchia, Storage, RuntimeServiceId> MempoolAdapter<Cryptarchia::Tx, RuntimeServiceId>
    for TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia::Tx: Transaction + Clone + Send + Sync,
    <Cryptarchia::Tx as Transaction>::Hash: Sync,
    Storage: MempoolStorageAdapterNew<RuntimeServiceId, Tx = Cryptarchia::Tx>
        + MempoolStorageAdapter<RuntimeServiceId, Tx = Cryptarchia::Tx>
        + Send
        + Sync,
    Cryptarchia: CryptarchiaServiceData + Send + Sync,
    Cryptarchia::Tx: Send + Sync,
    Storage::Error: Debug,
    RuntimeServiceId: Debug
        + Display
        + Send
        + Sync
        + AsServiceId<
            StorageService<
                <Storage as MempoolStorageAdapter<RuntimeServiceId>>::Backend,
                RuntimeServiceId,
            >,
        > + AsServiceId<Cryptarchia>,
{
    async fn new(overwatch_handle: OverwatchHandle<RuntimeServiceId>) -> Result<Self, DynError> {
        let storage_relay = overwatch_handle
            .relay::<StorageService<
                <Storage as MempoolStorageAdapter<RuntimeServiceId>>::Backend,
                RuntimeServiceId,
            >>()
            .await?;
        let storage_adapter =
            <Storage as MempoolStorageAdapterNew<RuntimeServiceId>>::new(storage_relay);
        let cryptarchia_api: CryptarchiaServiceApi<Cryptarchia, RuntimeServiceId> =
            CryptarchiaServiceApi::new(
                overwatch_handle
                    .relay::<Cryptarchia>()
                    .await
                    .expect("Cryptarchia service relay should be available"),
            );
        Ok(Self::new(cryptarchia_api, storage_adapter))
    }
}

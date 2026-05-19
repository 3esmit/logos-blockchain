use std::{collections::HashSet, fmt::Debug};

use futures::FutureExt;
use lb_chain_service::api::{CryptarchiaServiceApi, CryptarchiaServiceData};
use lb_core::{
    block::Block,
    header::HeaderId,
    mantle::{DependencyId, Transaction},
};
use tokio_stream::StreamExt;
use tracing::error;

use crate::{
    backend::{
        forks::{BlockInfo, BlockInfoGetter, ForksTrackerError, LedgerStateGetter},
        inspector::LedgerStateInspector,
    },
    storage::MempoolStorageAdapter,
};

pub struct TrackerAdapter<Cryptarchia, Storage, RuntimeServiceId>
where
    Cryptarchia: CryptarchiaServiceData,
{
    storage: Storage,
    crypatarchia_api: CryptarchiaServiceApi<Cryptarchia, RuntimeServiceId>,
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

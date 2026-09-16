use std::pin::Pin;

use futures::Stream;
use lb_core::{
    header::HeaderId,
    mantle::{traits::Hashable, transactions::hash::TxHash},
};
use lb_tx_service::{MempoolMsg, api::MempoolServiceApi};
use overwatch::services::relay::OutboundRelay;

use super::MempoolAdapter as MempoolAdapterTrait;

pub struct MempoolAdapter<Tx> {
    mempool_api: MempoolServiceApi<HeaderId, Tx, Tx, TxHash>,
}

impl<Tx: Send> MempoolAdapter<Tx> {
    #[must_use]
    pub const fn new(mempool_relay: OutboundRelay<MempoolMsg<HeaderId, Tx, Tx, TxHash>>) -> Self {
        Self {
            mempool_api: MempoolServiceApi::new(mempool_relay),
        }
    }
}

#[async_trait::async_trait]
impl<Tx> MempoolAdapterTrait<Tx> for MempoolAdapter<Tx>
where
    Tx: Hashable<Hash = TxHash> + Send + Sync + 'static,
{
    async fn get_mempool_view(
        &self,
        ancestor_hint: HeaderId,
    ) -> Result<Pin<Box<dyn Stream<Item = Tx> + Send>>, overwatch::DynError> {
        self.mempool_api
            .view(ancestor_hint)
            .await
            .map_err(|e| overwatch::DynError::from(format!("Could not get mempool view: {e}")))
    }

    async fn remove_transactions(&self, ids: &[TxHash]) -> Result<(), overwatch::DynError> {
        self.mempool_api.remove(ids.to_vec()).await.map_err(|e| {
            overwatch::DynError::from(format!("Could not remove transactions from mempool: {e}"))
        })?;

        Ok(())
    }

    async fn post_tx(&self, tx: Tx) -> Result<(), overwatch::DynError> {
        self.mempool_api.add(tx.hash(), tx).await.map_err(|e| {
            overwatch::DynError::from(format!("Failed to post transaction to mempool: {e}"))
        })
    }
}

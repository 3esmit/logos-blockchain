use lb_core::{
    header::HeaderId,
    mantle::{
        traits::Hashable,
        transactions::hash::{TxHash, TxHashPrefix},
    },
};
use lb_tx_service::{MempoolMsg, TxsWithCommonPrefix, api::MempoolServiceApi};
use overwatch::services::relay::OutboundRelay;

use super::MempoolAdapter as MempoolAdapterTrait;

#[derive(Clone)]
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
    async fn add_transaction(&self, tx: Tx) -> Result<(), overwatch::DynError> {
        self.mempool_api
            .add(tx.hash(), tx)
            .await
            .map_err(|e| format!("Could not add transactions to mempool: {e}"))?;
        Ok(())
    }

    async fn remove_transactions(&self, ids: &[TxHash]) -> Result<(), overwatch::DynError> {
        self.mempool_api
            .remove(ids.to_vec())
            .await
            .map_err(|e| format!("Could not remove transactions from mempool: {e}"))?;

        Ok(())
    }

    async fn get_transactions_by_prefix(
        &self,
        prefix: TxHashPrefix,
    ) -> Result<TxsWithCommonPrefix<Tx>, overwatch::DynError> {
        self.mempool_api
            .get_transactions_by_prefix(prefix)
            .await
            .map_err(|e| format!("Could not get transactions by prefix: {e}").into())
    }
}

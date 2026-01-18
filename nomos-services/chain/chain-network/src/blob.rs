use std::{marker::PhantomData, num::NonZero};

use cryptarchia_engine::Slot;
use nomos_core::{
    block::Block,
    mantle::{AuthenticatedMantleTx, Op},
};
use nomos_time::TimeServiceMessage;
use tokio::sync::oneshot;
use tracing::debug;

use crate::{LOG_TARGET, relays::TimeRelay};

/// An instance for validating blobs in blocks.
#[derive(Clone)]
pub struct Validation<S: Strategy> {
    consensus_base_period_length: NonZero<u64>,
    time_relay: TimeRelay,
    _phantom: PhantomData<S>,
}

impl<S: Strategy> Validation<S> {
    pub const fn new(consensus_base_period_length: NonZero<u64>, time_relay: TimeRelay) -> Self {
        Self {
            consensus_base_period_length,
            time_relay,
            _phantom: PhantomData,
        }
    }
}

impl<S: Strategy + Sync> Validation<S> {
    /// Validate blobs in the given block.
    ///
    /// If the block is outside the blob validation window, which is calculated
    /// based on the current slot and the consensus base period length,
    /// no validation is performed and `Ok(())` is returned.
    pub async fn validate<Tx>(&self, block: &Block<Tx>) -> Result<(), Error>
    where
        Tx: AuthenticatedMantleTx + Sync,
    {
        if !should_validate_blobs(
            block.header().slot(),
            get_current_slot(&self.time_relay).await?,
            self.consensus_base_period_length,
        ) {
            return Ok(());
        }

        S::validate(block).await
    }
}

async fn get_current_slot(time_relay: &TimeRelay) -> Result<Slot, Error> {
    let (sender, receiver) = oneshot::channel();
    time_relay
        .send(TimeServiceMessage::CurrentSlot { sender })
        .await
        .map_err(|(e, _)| e)?;
    Ok(receiver.await?.slot)
}

fn should_validate_blobs(
    block_slot: Slot,
    current_slot: Slot,
    consensus_base_period_length: NonZero<u64>,
) -> bool {
    current_slot.saturating_sub(block_slot)
        <= blob_validation_window_in_slots(consensus_base_period_length)
}

const fn blob_validation_window_in_slots(consensus_base_period_length: NonZero<u64>) -> Slot {
    Slot::new(consensus_base_period_length.get() / 2)
}

#[async_trait::async_trait]
pub trait Strategy {
    async fn validate<Tx>(block: &Block<Tx>) -> Result<(), Error>
    where
        Tx: AuthenticatedMantleTx + Sync;
}

/// Validation strategy for blobs in blocks received through recent block
/// propagation, under the assumption that the DA sampling service has already
/// sampled and validated the blobs.
pub struct RecentBlobStrategy;

#[async_trait::async_trait]
impl Strategy for RecentBlobStrategy {
    async fn validate<Tx>(block: &Block<Tx>) -> Result<(), Error>
    where
        Tx: AuthenticatedMantleTx + Sync,
    {
        debug!(target = LOG_TARGET, "Validating recent blobs");

        // Check if block contains any DA blob operations
        let has_blob_ops = block
            .transactions()
            .flat_map(|tx| tx.mantle_tx().ops.iter())
            .any(|op| matches!(op, Op::ChannelBlob(_)));

        if has_blob_ops {
            // DA is not supported in this version - reject block containing DA blobs
            tracing::error!(target: LOG_TARGET, "Found DA blobs in block but DA is not supported in this version");
            return Err(Error::DaNotSupported);
        }

        Ok(())
    }
}

/// Validation strategy for blobs in blocks retrieved manually (e.g. chain
/// bootstrapping or orphan handling), under the assumption that the DA sampling
/// service has not yet sampled and validated the blobs.
#[derive(Clone)]
pub struct HistoricBlobStrategy;

#[async_trait::async_trait]
impl Strategy for HistoricBlobStrategy {
    async fn validate<Tx>(block: &Block<Tx>) -> Result<(), Error>
    where
        Tx: AuthenticatedMantleTx + Sync,
    {
        debug!(target = LOG_TARGET, "Validating historic blobs");

        // Check if block contains any DA blob operations
        let has_blob_ops = block
            .transactions()
            .flat_map(|tx| tx.mantle_tx().ops.iter())
            .any(|op| matches!(op, Op::ChannelBlob(_)));

        if has_blob_ops {
            // DA is not supported in this version - reject block containing DA blobs
            tracing::error!(target: LOG_TARGET, "Found DA blobs in block but DA is not supported in this version");
            return Err(Error::DaNotSupported);
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Block contains invalid blobs")]
    InvalidBlobs,
    #[error("DA operations are not supported")]
    DaNotSupported,
    #[error("Relay error: {0}")]
    Relay(#[from] overwatch::services::relay::RelayError),
    #[error("Reply channel error: {0}")]
    ReplyRecv(#[from] oneshot::error::RecvError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_validation_window_in_slots() {
        assert_eq!(
            blob_validation_window_in_slots(10.try_into().unwrap()),
            5.into()
        );
        assert_eq!(
            blob_validation_window_in_slots(7.try_into().unwrap()),
            3.into() // Should round down
        );
        assert_eq!(
            blob_validation_window_in_slots(1.try_into().unwrap()),
            0.into() // Should round down
        );
    }

    #[test]
    fn test_should_validate_blobs() {
        // (103 - 100) <= (10 / 2)
        assert!(should_validate_blobs(
            100.into(),
            103.into(),
            10.try_into().unwrap()
        ));
        // (105 - 100) <= (10 / 2)
        assert!(should_validate_blobs(
            100.into(),
            105.into(),
            10.try_into().unwrap()
        ));
        // (106 - 100) > (10 / 2)
        assert!(!should_validate_blobs(
            100.into(),
            106.into(),
            10.try_into().unwrap()
        ));
        // saturating(100 - 101) <= (10 / 2)
        assert!(should_validate_blobs(
            101.into(),
            100.into(),
            10.try_into().unwrap()
        ));
    }
}

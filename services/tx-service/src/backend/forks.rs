use std::{collections::HashMap, hash::Hash};

use lb_chain_service::{LibUpdate, ProcessedBlockEvent, PrunedBlocksInfo};
use lb_core::{block::Block, header::HeaderId, mantle::TransactionDependencies};

use super::tracker::TxTrackerState;

pub enum ForksTrackerError {
    BlockNotFound,
    ParentNotFound(HeaderId),
}
pub struct ForksTracker<Tx, TxId>
where
    TxId: Eq + Hash,
{
    states: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    current_tips: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
}

impl<Tx> ForksTracker<Tx, Tx::Hash>
where
    Tx: TransactionDependencies + Clone,
{
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            current_tips: HashMap::new(),
        }
    }

    pub fn process_lib(&mut self, event: &LibUpdate) {
        let LibUpdate {
            new_lib,
            pruned_blocks:
                PrunedBlocksInfo {
                    stale_blocks,
                    immutable_blocks,
                },
        } = event;

        for block in stale_blocks.iter().chain(immutable_blocks.values()) {
            // remove lib state
            drop(self.states.remove(block));
            drop(self.current_tips.remove(block));
        }
    }

    pub async fn process_new_block(
        &mut self,
        event: &ProcessedBlockEvent,
    ) -> Result<(), ForksTrackerError> {
        let ProcessedBlockEvent { block_id, tip, .. } = event;
        let block: Block<Tx> = self.get_block(block_id).await?;
        let parent = block.header().parent();
        if let Some(state) = self.current_tips.get(&parent) {
            let mut state: TxTrackerState<_, _> = state.clone();
            for tx in block.into_transactions() {
                state.process_tx(tx);
            }
            drop(self.current_tips.remove(&parent));
            self.current_tips.insert(*block_id, state);
            Ok(())
        } else {
            Err(ForksTrackerError::ParentNotFound(parent))
        }
    }

    pub fn process_new_tx(&mut self, tx: &Tx) {
        for state in self.current_tips.values_mut() {
            state.process_tx(tx.clone())
        }
    }

    async fn get_block(&self, header_id: &HeaderId) -> Result<Block<Tx>, ForksTrackerError> {
        unimplemented!()
    }
}

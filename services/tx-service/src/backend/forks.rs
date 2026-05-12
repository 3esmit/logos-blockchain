use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

use lb_core::mantle::TransactionDependencies;

use super::tracker::TxTrackerState;
pub struct ForksTracker<HeaderId, Tx, TxId>
where
    TxId: Eq + Hash,
    HeaderId: Eq + Hash,
{
    indexes: HashMap<HeaderId, TxTrackerState<Tx, TxId>>,
    current_chain: HashMap<HeaderId, HeaderId>,
}

impl<HeaderId, Tx> ForksTracker<HeaderId, Tx, Tx::Hash>
where
    Tx: TransactionDependencies,
    HeaderId: Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            indexes: HashMap::new(),
        }
    }

    pub fn process_lib(&mut self, lib: &HeaderId) {
        // remove lib state
        self.indexes.remove(lib);
    }

    pub fn process_new_block(&mut self) {}

    pub fn process_reorg(&mut self) {}
}

use std::collections::HashSet;

use lb_core::mantle::{DependencyId, NoteId, ops::channel::MsgId};
use lb_ledger::LedgerState;

pub struct LedgerStateInspector(LedgerState);

impl LedgerStateInspector {
    pub fn new(ledger_state: LedgerState) -> Self {
        Self(ledger_state)
    }
    pub fn dependencies(&self) -> HashSet<DependencyId> {
        let state = &self.0;
        let utxos = state
            .latest_utxos()
            .utxos()
            .keys()
            .map(|note_id: &NoteId| DependencyId::copy_from_slice(&note_id.as_bytes()));
        let inscriptions_tips = state
            .mantle_ledger()
            .inscriptions_tips()
            .map(|id: MsgId| DependencyId::copy_from_slice(id.as_ref()));
        utxos.chain(inscriptions_tips).collect()
    }
}

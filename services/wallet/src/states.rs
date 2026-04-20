use lb_core::{
    header::HeaderId,
    mantle::ops::leader_claim::{VoucherCm, VoucherNullifier},
};
use lb_ledger::LedgerState;
use lb_wallet::{Vouchers, WalletBlock, WalletError, WalletState};
use overwatch::services::state::StateUpdater;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{KeyId, WalletServiceError, WalletServiceSettings};

type VoucherIndex = u64;
type VoucherId = (KeyId, VoucherIndex);
pub type Wallet = lb_wallet::Wallet<KeyId, VoucherId>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryState {
    next_new_voucher_index: VoucherIndex,
    vouchers: Vouchers<VoucherId>,
    /// Persisted wallet state at the last known LIB.
    /// `None` on fresh start; populated after the first LIB update.
    lib_wallet_state: Option<(HeaderId, WalletState)>,
}

impl overwatch::services::state::ServiceState for RecoveryState {
    type Settings = WalletServiceSettings;
    type Error = WalletServiceError;

    fn from_settings(_settings: &Self::Settings) -> Result<Self, Self::Error> {
        Ok(Self {
            next_new_voucher_index: 0,
            vouchers: Vouchers::default(),
            lib_wallet_state: None,
        })
    }
}

/// Provides operations on the states that must be synced to [`RecoveryState`].
pub struct ServiceState<'u> {
    next_new_voucher_index: VoucherIndex,
    wallet: Wallet,
    current_lib: HeaderId,
    updater: &'u StateUpdater<Option<RecoveryState>>,
}

impl<'u> ServiceState<'u> {
    pub fn new(
        state: RecoveryState,
        settings: &WalletServiceSettings,
        lib: HeaderId,
        lib_ledger: &LedgerState,
        updater: &'u StateUpdater<Option<RecoveryState>>,
    ) -> Self {
        let known_keys = settings
            .known_keys
            .clone()
            .into_iter()
            .map(|(key_id, pk)| (pk, key_id));

        let (wallet, current_lib) =
            if let Some((persisted_lib, wallet_state)) = state.lib_wallet_state {
                let wallet = Wallet::from_lib_wallet_state(
                    known_keys,
                    state.vouchers,
                    persisted_lib,
                    wallet_state,
                );
                (wallet, persisted_lib)
            } else {
                let wallet =
                    Wallet::from_lib_ledger_state(known_keys, state.vouchers, lib, lib_ledger);
                (wallet, lib)
            };

        Self {
            next_new_voucher_index: state.next_new_voucher_index,
            wallet,
            current_lib,
            updater,
        }
    }

    pub fn get_and_inc_next_new_voucher_index(&mut self) -> VoucherIndex {
        let index = self.next_new_voucher_index;
        self.next_new_voucher_index += 1;
        self.update_state();
        index
    }

    pub fn add_known_voucher(&mut self, cm: VoucherCm, nf: VoucherNullifier, id: VoucherId) {
        self.wallet.add_known_voucher(cm, nf, id);
        self.update_state();
    }

    pub fn apply_block(&mut self, block: &WalletBlock) -> Result<(), WalletError> {
        self.wallet.apply_block(block)?;
        self.update_state();
        Ok(())
    }

    pub fn prune_states(&mut self, pruned_blocks: impl IntoIterator<Item = HeaderId>) {
        self.wallet.prune_states(pruned_blocks);
        self.update_state();
    }

    pub fn prune_vouchers(
        &mut self,
        pruned_nullifiers: impl IntoIterator<Item = VoucherNullifier>,
    ) {
        self.wallet.prune_vouchers(pruned_nullifiers);
        self.update_state();
    }

    pub fn advance_lib(&mut self, new_lib: HeaderId) {
        self.current_lib = new_lib;
        self.update_state();
    }

    pub const fn wallet(&self) -> &Wallet {
        &self.wallet
    }

    fn update_state(&self) {
        let lib_wallet_state = self
            .wallet
            .wallet_state_at(self.current_lib)
            .map(|ws| (self.current_lib, ws))
            .inspect_err(|e| {
                warn!(lib=?self.current_lib, err=%e, "Could not snapshot wallet state at LIB");
            })
            .ok();

        self.updater.update(Some(RecoveryState {
            next_new_voucher_index: self.next_new_voucher_index,
            vouchers: self.wallet.vouchers().clone(),
            lib_wallet_state,
        }));
    }
}

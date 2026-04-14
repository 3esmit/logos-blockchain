use std::cmp::Ordering;

use lb_core::{
    crypto::ZkHasher,
    mantle::{
        Value,
        ops::leader_claim::{LeaderClaimOp, RewardsRoot, VoucherCm, VoucherNullifier},
    },
};
use lb_cryptarchia_engine::Epoch;
use lb_mmr::MerkleMountainRange;

/// A leader state in the mantle ledger.
///
/// NOTE: Most collection fields in this struct should use `rpds`
/// since we keep a copy of this state for each block.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderState {
    // current epoch
    epoch: Epoch,
    // Root of vouchers that can be claimed in this epoch.
    // This is updated once at the start of each epoch.
    claimable_vouchers_root: RewardsRoot,
    n_claimable_vouchers: u64,
    // nullifiers of vouchers that have been claimed since genesis
    nfs: rpds::HashTrieSetSync<VoucherNullifier>,
    // rewards to be distributed
    // at the start of each epoch this is increased by the amount of rewards
    // that have been collected in the previous epoch.
    // unclaimed rewards are carried over to the next epoch.
    claimable_rewards: Value,
    /// Rewards that are being collected during the current epoch.
    /// This will be added to the `claimable_rewards` when a new epoch starts.
    pending_rewards: Value,
    // All vouchers collected up to the current block.
    // This does not always match `claimable_vouchers_root`.
    vouchers: MerkleMountainRange<VoucherCm, ZkHasher>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("voucher nullifier already used")]
    DuplicatedVoucherNullifier,
    #[error("voucher not found")]
    VoucherNotFound,
    #[error("Cannot time travel to the past")]
    InvalidEpoch { current: Epoch, incoming: Epoch },
}

impl Default for LeaderState {
    fn default() -> Self {
        Self::new()
    }
}

impl LeaderState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: 0.into(),
            claimable_vouchers_root: RewardsRoot::default(),
            n_claimable_vouchers: 0,
            nfs: rpds::HashTrieSetSync::new_sync(),
            pending_rewards: 0,
            claimable_rewards: 0,
            vouchers: MerkleMountainRange::new(),
        }
    }

    pub fn try_apply_header(self, epoch: Epoch, voucher_cm: VoucherCm) -> Result<Self, Error> {
        Ok(self.update_epoch_state(epoch)?.push_voucher(voucher_cm))
    }

    fn update_epoch_state(mut self, epoch: Epoch) -> Result<Self, Error> {
        match epoch.cmp(&self.epoch) {
            Ordering::Equal => Ok(self),
            Ordering::Less => Err(Error::InvalidEpoch {
                current: self.epoch,
                incoming: epoch,
            }),
            Ordering::Greater => {
                self = self.snapshot_claimable_vouchers();
                self = self.update_claimable_rewards();
                self.epoch = epoch;
                Ok(self)
            }
        }
    }

    /// Add a block reward to the pending rewards that are added to the pool
    /// during epoch transition
    #[must_use]
    pub const fn add_pending_rewards(mut self, rewards: Value) -> Self {
        self.pending_rewards += rewards;
        self
    }

    fn push_voucher(mut self, voucher_cm: VoucherCm) -> Self {
        self.vouchers = self.vouchers.push(voucher_cm).expect("MMR is full");
        self
    }

    /// Snapshot the current MMR root as the claimable root for the new epoch.
    fn snapshot_claimable_vouchers(mut self) -> Self {
        self.claimable_vouchers_root = self.vouchers.frontier_root().into();
        self.n_claimable_vouchers = self.vouchers.len() as u64;
        self
    }

    /// Insert all pending rewards into the reward pool and reset it
    fn update_claimable_rewards(mut self) -> Self {
        self.claimable_rewards += self.pending_rewards;
        self.pending_rewards = Value::default();
        self
    }

    pub(crate) const fn claimable_vouchers_root(&self) -> RewardsRoot {
        self.claimable_vouchers_root
    }

    /// Compute the per-voucher reward given current state.
    #[must_use]
    pub fn reward_amount(&self) -> Value {
        let n_unclaimed_vouchers = self
            .n_claimable_vouchers
            .saturating_sub(self.nfs.size() as u64);
        if n_unclaimed_vouchers > 0 {
            self.claimable_rewards / n_unclaimed_vouchers
        } else {
            0
        }
    }

    /// Claim the reward associated with a voucher.
    /// Any cryptographic proof of correct derivation of the voucher nullifier
    /// and membership proof in the merkle tree is expected to happen
    /// outside of this function.
    pub fn claim(&self, op: &LeaderClaimOp) -> Result<(Self, Value), Error> {
        if self.nfs.contains(&op.voucher_nullifier) {
            return Err(Error::DuplicatedVoucherNullifier);
        }

        if self.claimable_vouchers_root != op.rewards_root {
            return Err(Error::VoucherNotFound);
        }

        let reward_amount = self.reward_amount();
        let nfs = self.nfs.insert(op.voucher_nullifier);
        let claimable_rewards = self.claimable_rewards - reward_amount;
        Ok((
            Self {
                nfs,
                claimable_rewards,
                ..self.clone()
            },
            reward_amount,
        ))
    }
}

#[cfg(test)]
mod tests {
    use lb_groth16::{Field as _, Fr};

    use super::*;

    impl LeaderState {
        #[cfg(test)]
        #[must_use]
        pub fn get_pending_rewards(&self) -> Value {
            self.pending_rewards
        }
    }

    #[test]
    fn test_reward_amounts() {
        let state = LeaderState::new();
        let state = state.try_apply_header(1.into(), Fr::ZERO.into()).unwrap();
        let state = state.try_apply_header(1.into(), Fr::ONE.into()).unwrap();
        let state = state
            .try_apply_header(1.into(), Fr::from(2u64).into())
            .unwrap();
        let state = state
            .try_apply_header(2.into(), Fr::from(3u64).into())
            .unwrap();
        let state = LeaderState {
            claimable_rewards: 300,
            ..state
        };
        let op1 = LeaderClaimOp {
            rewards_root: state.claimable_vouchers_root,
            voucher_nullifier: Fr::ZERO.into(),
        };
        let (state, bal) = state.claim(&op1).unwrap();
        assert_eq!(bal, 100);
        assert_eq!(state.claimable_rewards, 200);
        let op2 = LeaderClaimOp {
            rewards_root: state.claimable_vouchers_root,
            voucher_nullifier: Fr::ONE.into(),
        };
        let (state, bal) = state.claim(&op2).unwrap();
        assert_eq!(bal, 100);
        assert_eq!(state.claimable_rewards, 100);
        let op3 = LeaderClaimOp {
            rewards_root: state.claimable_vouchers_root,
            voucher_nullifier: Fr::from(2u64).into(),
        };
        let (state, bal) = state.claim(&op3).unwrap();
        assert_eq!(bal, 100);
        assert_eq!(state.claimable_rewards, 0);
    }

    #[test]
    fn test_epoch_transition() {
        let state = LeaderState::new();
        let state = state.try_apply_header(1.into(), Fr::ZERO.into()).unwrap();
        assert_eq!(state.epoch, 1.into());
        assert_eq!(state.n_claimable_vouchers, 0);
        let state = state.try_apply_header(2.into(), Fr::ONE.into()).unwrap();
        assert_eq!(state.epoch, 2.into());
        assert_eq!(state.n_claimable_vouchers, 1);
        let state = state
            .try_apply_header(2.into(), Fr::from(2u64).into())
            .unwrap();
        assert_eq!(state.epoch, 2.into());
        assert_eq!(state.n_claimable_vouchers, 1);
        let state = state
            .try_apply_header(3.into(), Fr::from(3u64).into())
            .unwrap();
        assert_eq!(state.epoch, 3.into());
        assert_eq!(state.n_claimable_vouchers, 3);
        let err = state
            .clone()
            .try_apply_header(2.into(), Fr::from(4u64).into())
            .unwrap_err();
        assert_eq!(
            err,
            Error::InvalidEpoch {
                current: 3.into(),
                incoming: 2.into()
            }
        );
        let state = state
            .try_apply_header(4.into(), Fr::from(5u64).into())
            .unwrap();
        assert_eq!(state.epoch, 4.into());
        assert_eq!(state.n_claimable_vouchers, 4);
    }

    #[test]
    fn test_cannot_claim_reward_twice() {
        let state = LeaderState::new();
        let op = LeaderClaimOp {
            voucher_nullifier: Fr::ZERO.into(),
            rewards_root: state.claimable_vouchers_root,
        };
        let (state, balance) = state.claim(&op).unwrap();
        assert_eq!(balance, 0);
        let err = state.claim(&op).unwrap_err();
        assert_eq!(err, Error::DuplicatedVoucherNullifier);
    }
}

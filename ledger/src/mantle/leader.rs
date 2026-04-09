use std::{cmp::Ordering, collections::HashMap};

use lb_core::{
    crypto::ZkHasher,
    mantle::{
        Value,
        ops::leader_claim::{LeaderClaimOp, RewardsRoot, VoucherCm, VoucherNullifier},
    },
};
use lb_cryptarchia_engine::Epoch;
use lb_mmr::{MerkleMountainRange, MerklePath};
use rpds::VectorSync;

pub type VoucherMmr = MerkleMountainRange<VoucherCm, ZkHasher>;

/// Tracked voucher merkle paths, keyed by commitment.
///
/// - `Some(path)`: an existing path that is kept up-to-date across pushes.
/// - `None`: the commitment is tracked but has not yet been flushed into the
///   MMR; a path will be created when the voucher is flushed during an epoch
///   transition.
pub type TrackedVoucherPaths = HashMap<VoucherCm, Option<MerklePath>>;

/// A leader state in the mantle ledger.
///
/// NOTE: Most collection fields in this struct should use `rpds`
/// since we keep a copy of this state for each block.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderState {
    // current epoch
    epoch: Epoch,
    // vouchers that can be claimed in this epoch
    // this is updated once at the start of each epoch
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
    // MMR of vouchers that can be claimed in this epoch.
    // Updated once at the start of each epoch by flushing pending_vouchers.
    claimable_vouchers: VoucherMmr,
    // List of vouchers that are waiting to be added at the start of
    // the next epoch
    pending_vouchers: VectorSync<VoucherCm>,
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
            claimable_vouchers: VoucherMmr::new(),
            pending_vouchers: VectorSync::new_sync(),
        }
    }

    pub fn try_apply_header(
        self,
        epoch: Epoch,
        voucher_cm: VoucherCm,
        tracked: &mut TrackedVoucherPaths,
    ) -> Result<Self, Error> {
        Ok(self
            .update_epoch_state(epoch, tracked)?
            .add_pending_voucher(voucher_cm))
    }

    fn update_epoch_state(
        mut self,
        epoch: Epoch,
        tracked: &mut TrackedVoucherPaths,
    ) -> Result<Self, Error> {
        match epoch.cmp(&self.epoch) {
            Ordering::Equal => Ok(self),
            Ordering::Less => Err(Error::InvalidEpoch {
                current: self.epoch,
                incoming: epoch,
            }),
            Ordering::Greater => {
                self = self.update_claimable_vouchers(tracked);
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

    /// Add a voucher to be included in the Merkle tree at the start of the
    /// next epoch
    fn add_pending_voucher(mut self, voucher_cm: VoucherCm) -> Self {
        self.pending_vouchers.push_back_mut(voucher_cm);
        self
    }

    /// Flush all pending vouchers into the MMR and update the root.
    ///
    /// Existing paths in `tracked` are kept up-to-date. If a flushed voucher
    /// is tracked with `None`, a new path is created and stored.
    fn update_claimable_vouchers(mut self, tracked: &mut TrackedVoucherPaths) -> Self {
        for &voucher_cm in &self.pending_vouchers {
            let (new_mmr, new_path) = self
                .claimable_vouchers
                .push_with_paths(voucher_cm, tracked.values_mut().filter_map(Option::as_mut))
                .expect("MMR should not be full");
            self.claimable_vouchers = new_mmr;

            if let Some(path) = tracked.get_mut(&voucher_cm) {
                *path = Some(new_path);
            }
        }

        self.pending_vouchers = VectorSync::new_sync();
        self.claimable_vouchers_root = self.claimable_vouchers.frontier_root().into();
        self.n_claimable_vouchers = self.claimable_vouchers.len() as u64;
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
        let tracked = &mut TrackedVoucherPaths::new();
        let state = LeaderState::new();
        let state = state
            .try_apply_header(1.into(), Fr::ZERO.into(), tracked)
            .unwrap();
        let state = state
            .try_apply_header(1.into(), Fr::ONE.into(), tracked)
            .unwrap();
        let state = state
            .try_apply_header(1.into(), Fr::from(2u64).into(), tracked)
            .unwrap();
        let state = state
            .try_apply_header(2.into(), Fr::from(3u64).into(), tracked)
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
        let tracked = &mut TrackedVoucherPaths::new();
        let state = LeaderState::new();
        let state = state
            .try_apply_header(1.into(), Fr::ZERO.into(), tracked)
            .unwrap();
        assert_eq!(state.epoch, 1.into());
        assert_eq!(state.n_claimable_vouchers, 0);
        let state = state
            .try_apply_header(2.into(), Fr::ONE.into(), tracked)
            .unwrap();
        assert_eq!(state.epoch, 2.into());
        assert_eq!(state.n_claimable_vouchers, 1);
        let state = state
            .try_apply_header(2.into(), Fr::from(2u64).into(), tracked)
            .unwrap();
        assert_eq!(state.epoch, 2.into());
        assert_eq!(state.n_claimable_vouchers, 1);
        let state = state
            .try_apply_header(3.into(), Fr::from(3u64).into(), tracked)
            .unwrap();
        assert_eq!(state.epoch, 3.into());
        assert_eq!(state.n_claimable_vouchers, 3);
        let err = state
            .clone()
            .try_apply_header(2.into(), Fr::from(4u64).into(), tracked)
            .unwrap_err();
        assert_eq!(
            err,
            Error::InvalidEpoch {
                current: 3.into(),
                incoming: 2.into()
            }
        );
        let state = state
            .try_apply_header(4.into(), Fr::from(5u64).into(), tracked)
            .unwrap();
        assert_eq!(state.epoch, 4.into());
        assert_eq!(state.n_claimable_vouchers, 4);
    }

    #[test]
    fn test_tracked_paths_created_on_flush() {
        let cm0: VoucherCm = Fr::ZERO.into();
        let cm1: VoucherCm = Fr::ONE.into();
        let cm2: VoucherCm = Fr::from(2u64).into();

        // Track cm0 and cm1 (but not cm2)
        let mut tracked = TrackedVoucherPaths::new();
        tracked.insert(cm0, None);
        tracked.insert(cm1, None);

        // Epoch 1: add three vouchers as pending
        let state = LeaderState::new();
        let state = state.try_apply_header(1.into(), cm0, &mut tracked).unwrap();
        let state = state.try_apply_header(1.into(), cm1, &mut tracked).unwrap();
        let state = state.try_apply_header(1.into(), cm2, &mut tracked).unwrap();

        // Paths not yet created (still pending)
        assert!(tracked[&cm0].is_none());
        assert!(tracked[&cm1].is_none());

        // Epoch 2: flush pending vouchers into the MMR
        let cm3: VoucherCm = Fr::from(3u64).into();
        let state = state.try_apply_header(2.into(), cm3, &mut tracked).unwrap();

        // Now tracked vouchers should have valid paths
        let root = state.claimable_vouchers_root;
        let path0 = tracked[&cm0].as_ref().expect("path for cm0");
        let path1 = tracked[&cm1].as_ref().expect("path for cm1");
        assert!(path0.verify::<ZkHasher>(*cm0.as_ref(), root.into()));
        assert!(path1.verify::<ZkHasher>(*cm1.as_ref(), root.into()));

        // cm2 and cm3 were not tracked
        assert!(!tracked.contains_key(&cm2));
        assert!(!tracked.contains_key(&cm3));

        // Verify paths against the MMR root

        // Create cm4 and track it. Add it to state at epoch 2 (not 3)
        let cm4: VoucherCm = Fr::from(4u64).into();
        tracked.insert(cm4, None);
        let state = state.try_apply_header(2.into(), cm4, &mut tracked).unwrap();

        // Epoch 3: flush pending vouchers into the MMR
        let cm5: VoucherCm = Fr::from(5u64).into();
        let state = state.try_apply_header(3.into(), cm5, &mut tracked).unwrap();

        // Now tracked vouchers should have updated valid paths
        let root = state.claimable_vouchers_root;
        let path0 = tracked[&cm0].as_ref().expect("path for cm0");
        let path1 = tracked[&cm1].as_ref().expect("path for cm1");
        let path4 = tracked[&cm4].as_ref().expect("path for cm4");
        assert!(path0.verify::<ZkHasher>(*cm0.as_ref(), root.into()));
        assert!(path1.verify::<ZkHasher>(*cm1.as_ref(), root.into()));
        assert!(path4.verify::<ZkHasher>(*cm4.as_ref(), root.into()));
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

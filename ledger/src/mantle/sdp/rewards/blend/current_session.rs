use lb_blend_crypto::merkle::sort_nodes_and_build_merkle_tree;
use lb_blend_message::{
    crypto::proofs::PoQVerificationInputsMinusSigningKey,
    encap::ProofsVerifier as ProofsVerifierTrait,
};
use lb_blend_proofs::quota::inputs::prove::public::{CoreInputs, LeaderInputs};
use lb_core::{
    crypto::ZkHash,
    mantle::Value,
    sdp::{ProviderId, blend::EpochRandomness},
};
use lb_cryptarchia_engine::Epoch;
use lb_key_management_system_keys::keys::ZkPublicKey;
use rpds::HashTrieMapSync;
use tracing::debug;

use crate::{
    EpochState,
    mantle::sdp::{
        Snapshot,
        rewards::blend::{LOG_TARGET, RewardsParameters, target_session::TargetEpochState},
    },
};

/// Immutable state of the current epoch.
/// The current epoch is E if E-1 is the target epoch for which rewards
/// are being calculated.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CurrentEpochState {
    /// Current epoch randomness
    epoch_randomness: EpochRandomness,
    /// Leader inputs of the current epoch, which will be used to create
    /// a proof verifier when the current epoch becomes the target epoch.
    leader_inputs: LeaderInputs,
}

impl CurrentEpochState {
    pub fn new(epoch_state: &EpochState, settings: &RewardsParameters) -> Self {
        Self {
            epoch_randomness: epoch_state.nonce().into(),
            leader_inputs: settings.leader_inputs(epoch_state),
        }
    }

    pub const fn epoch_randomness(&self) -> EpochRandomness {
        self.epoch_randomness
    }

    pub const fn leader_inputs(&self) -> LeaderInputs {
        self.leader_inputs
    }
}

/// Collects income seen in the current epoch.
/// The current epoch is E if E-1 is the target epoch for which rewards
/// are being calculated.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CurrentEpochTracker {
    /// Collecting service rewards over the epoch
    income: Value,
}

impl CurrentEpochTracker {
    pub fn new() -> Self {
        Self {
            income: Value::default(),
        }
    }

    pub(crate) const fn add_block_rewards(&self, block_rewards: Value) -> Self {
        Self {
            income: self.income + block_rewards,
        }
    }

    /// Finalizes the current epoch tracker.
    ///
    /// This should be called at the end of the current epoch: E -> E+1
    ///
    /// It returns [`CurrentEpochTrackerOutput::WithNewTargetEpoch`] by
    /// creating a [`TargetEpochState`] using the collected information,
    /// if the network size of the new target epoch is not below the
    /// minimum required. Otherwise, it returns
    /// [`CurrentEpochTrackerOutput::WithoutNewTargetEpoch`].
    pub fn finalize<ProofsVerifier>(
        &self,
        // Active snapshot of the epoch E that just ended
        last_active_snapshot: &Snapshot,
        // State of the epoch E that just ended.
        last_epoch_state: &CurrentEpochState,
        // State of the new epoch E+1
        new_epoch_state: &EpochState,
        settings: &RewardsParameters,
    ) -> CurrentEpochTrackerOutput<ProofsVerifier>
    where
        ProofsVerifier: ProofsVerifierTrait,
    {
        if last_active_snapshot.declarations.size() < settings.minimum_network_size.get() as usize {
            debug!(target: LOG_TARGET, "Declaration count({}) is below minimum network size({}). Switching to WithoutTargetSession mode",
                last_active_snapshot.declarations.size(),
                settings.minimum_network_size.get()
            );
            return CurrentEpochTrackerOutput::WithoutNewTargetEpoch {
                current_epoch_state: CurrentEpochState::new(new_epoch_state, settings),
                current_epoch_tracker: Self::new(),
            };
        }

        let (providers, zk_root) = Self::providers_and_zk_root(last_active_snapshot);

        let (core_quota, token_evaluation) = settings.core_quota_and_token_evaluation(
            providers.size() as u64,
        ).expect("evaluation parameters shouldn't overflow. panicking since we can't process the new session");

        let proof_verifier = Self::create_proof_verifier(
            last_epoch_state.leader_inputs(),
            last_active_snapshot.epoch,
            zk_root,
            core_quota,
        );

        CurrentEpochTrackerOutput::WithNewTargetEpoch {
            target_epoch_state: TargetEpochState::new(
                last_active_snapshot.epoch,
                providers,
                token_evaluation,
                proof_verifier,
                self.income,
            ),
            current_epoch_state: CurrentEpochState::new(new_epoch_state, settings),
            current_epoch_tracker: Self::new(),
        }
    }

    fn providers_and_zk_root(
        snapshot: &Snapshot,
    ) -> (HashTrieMapSync<ProviderId, (ZkPublicKey, u64)>, ZkHash) {
        let mut providers = snapshot
            .declarations
            .values()
            .map(|declaration| (declaration.provider_id, declaration.zk_id))
            .collect::<Vec<_>>();

        let zk_root =
            sort_nodes_and_build_merkle_tree(&mut providers, |(_, zk_id)| zk_id.into_inner())
                .expect("Should not fail to build merkle tree of core nodes' zk public keys")
                .root();

        let providers = providers
            .into_iter()
            .enumerate()
            .map(|(i, (provider_id, zk_id))| {
                (
                    provider_id,
                    (
                        zk_id,
                        u64::try_from(i).expect("provider index must fit in u64"),
                    ),
                )
            })
            .collect();

        (providers, zk_root)
    }

    fn create_proof_verifier<ProofsVerifier: ProofsVerifierTrait>(
        leader_inputs: LeaderInputs,
        epoch: Epoch,
        zk_root: ZkHash,
        core_quota: u64,
    ) -> ProofsVerifier {
        ProofsVerifier::new(PoQVerificationInputsMinusSigningKey {
            session: epoch.into_inner().into(), // TODO: pass epoch directly
            core: CoreInputs {
                zk_root,
                quota: core_quota,
            },
            leader: leader_inputs,
        })
    }
}

/// Result of finalizing the [`CurrentSessionTracker`].
pub enum CurrentEpochTrackerOutput<ProofsVerifier> {
    /// The new target epoch has been built with the information collected by
    /// the current epoch tracker.
    /// Also, the new current peoch state and tracker have been initialized.
    WithNewTargetEpoch {
        target_epoch_state: TargetEpochState<ProofsVerifier>,
        current_epoch_state: CurrentEpochState,
        current_epoch_tracker: CurrentEpochTracker,
    },
    /// No new target epoch has been built because the network size in the
    /// epoch is below the minimum required.
    WithoutNewTargetEpoch {
        current_epoch_state: CurrentEpochState,
        current_epoch_tracker: CurrentEpochTracker,
    },
}

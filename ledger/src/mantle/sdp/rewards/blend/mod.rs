mod current_session;
mod target_session;

use std::{fmt::Debug, num::NonZeroU64};

use lb_blend_message::{
    encap::ProofsVerifier as ProofsVerifierTrait, reward::BlendingTokenEvaluation,
};
use lb_blend_proofs::quota::inputs::prove::public::LeaderInputs;
use lb_core::{
    blend::core_quota,
    mantle::{Utxo, Value},
    sdp::{ActivityMetadata, ProviderId, ServiceParameters},
};
use lb_cryptarchia_engine::Slot;
use lb_utils::math::NonNegativeF64;

use crate::{
    EpochState,
    mantle::sdp::{
        Snapshot,
        rewards::{
            Error,
            blend::{
                current_session::{
                    CurrentEpochState, CurrentEpochTracker, CurrentEpochTrackerOutput,
                },
                target_session::{TargetEpochState, TargetEpochTracker},
            },
        },
    },
};

const LOG_TARGET: &str = "ledger::mantle::rewards::blend";

/// Tracks Blend rewards based on activity proofs submitted by providers.
/// Activity proofs for the epoch E-1 must be submitted during the epoch E.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Rewards<ProofsVerifier> {
    /// State during the epoch 0 (when no target epoch exists),
    /// or when the target epoch E-1 has less than the minimum required number
    /// of declarations.
    /// No activity messages are accepted in this state.
    WithoutTargetEpoch {
        current_epoch_state: CurrentEpochState,
        current_epoch_tracker: CurrentEpochTracker,
    },
    /// State after a new target epoch E-1 finishes.
    /// This tracks activity proofs for the target epoch E-1 submitted
    /// during the current epoch E.
    WithTargetEpoch {
        target_epoch_state: TargetEpochState<ProofsVerifier>,
        target_epoch_tracker: Box<TargetEpochTracker>,
        current_epoch_state: CurrentEpochState,
        current_epoch_tracker: CurrentEpochTracker,
    },
}

impl<ProofsVerifier> super::Rewards for Rewards<ProofsVerifier>
where
    ProofsVerifier: ProofsVerifierTrait + Clone + Debug + PartialEq + Send + Sync,
{
    type Params = RewardsParameters;

    fn update_active(
        &self,
        provider_id: ProviderId,
        metadata: &ActivityMetadata,
        params: &Self::Params,
    ) -> Result<Self, Error> {
        match self {
            Self::WithoutTargetEpoch { .. } => {
                // Reject all activity messages.
                Err(Error::TargetSessionNotSet)
            }
            Self::WithTargetEpoch {
                target_epoch_state,
                target_epoch_tracker,
                current_epoch_state,
                current_epoch_tracker,
            } => {
                let ActivityMetadata::Blend(proof) = metadata;

                let (zk_id, hamming_distance) = target_epoch_state.verify_proof(
                    &provider_id,
                    proof,
                    current_epoch_state,
                    params,
                )?;

                let target_epoch_tracker = target_epoch_tracker.insert(
                    provider_id,
                    target_epoch_state.epoch(),
                    zk_id,
                    hamming_distance,
                )?;

                Ok(Self::WithTargetEpoch {
                    target_epoch_state: target_epoch_state.clone(),
                    target_epoch_tracker: Box::new(target_epoch_tracker),
                    current_epoch_state: current_epoch_state.clone(),
                    current_epoch_tracker: current_epoch_tracker.clone(),
                })
            }
        }
    }

    fn update_epoch(
        &self,
        // Active snapshot of the epoch that just ended (E-1)
        last_active: &Snapshot,
        // State of the new epoch E
        new_epoch_state: &EpochState,
        _config: &ServiceParameters,
        params: &Self::Params,
    ) -> (Self, Vec<Utxo>) {
        match self {
            Self::WithoutTargetEpoch {
                current_epoch_state,
                current_epoch_tracker,
            } => (
                Self::from_current_epoch_tracker_output(
                    current_epoch_tracker.finalize(
                        last_active,
                        current_epoch_state,
                        new_epoch_state,
                        params,
                    ),
                    TargetEpochTracker::new(),
                ),
                Vec::new(),
            ),
            Self::WithTargetEpoch {
                target_epoch_state,
                target_epoch_tracker,
                current_epoch_state,
                current_epoch_tracker,
            } => {
                let (target_epoch_tracker, rewards) =
                    target_epoch_tracker.finalize(target_epoch_state);

                let new_state = Self::from_current_epoch_tracker_output(
                    current_epoch_tracker.finalize(
                        last_active,
                        current_epoch_state,
                        new_epoch_state,
                        params,
                    ),
                    target_epoch_tracker,
                );

                (new_state, rewards)
            }
        }
    }

    fn add_income(&self, block_rewards: Value) -> Self {
        match self {
            Self::WithoutTargetEpoch {
                current_epoch_state,
                current_epoch_tracker,
            } => Self::WithoutTargetEpoch {
                current_epoch_state: current_epoch_state.clone(),
                current_epoch_tracker: current_epoch_tracker.add_block_rewards(block_rewards),
            },
            Self::WithTargetEpoch {
                target_epoch_state,
                target_epoch_tracker,
                current_epoch_state,
                current_epoch_tracker,
            } => Self::WithTargetEpoch {
                target_epoch_state: target_epoch_state.clone(),
                target_epoch_tracker: target_epoch_tracker.clone(),
                current_epoch_state: current_epoch_state.clone(),
                current_epoch_tracker: current_epoch_tracker.add_block_rewards(block_rewards),
            },
        }
    }
}

impl<ProofsVerifier> Rewards<ProofsVerifier> {
    /// Create a new uninitialized [`Rewards`] that doesn't accept activity
    /// messages until the first epoch update: 0->1.
    #[must_use]
    pub fn new(settings: &RewardsParameters, epoch_state: &EpochState) -> Self {
        Self::WithoutTargetEpoch {
            current_epoch_state: CurrentEpochState::new(epoch_state, settings),
            current_epoch_tracker: CurrentEpochTracker::new(),
        }
    }
}

impl<ProofsVerifier> Rewards<ProofsVerifier>
where
    ProofsVerifier: ProofsVerifierTrait + Clone + Debug + PartialEq + Send + Sync,
{
    fn from_current_epoch_tracker_output(
        current_epoch_output: CurrentEpochTrackerOutput<ProofsVerifier>,
        target_epoch_tracker: TargetEpochTracker,
    ) -> Self {
        match current_epoch_output {
            CurrentEpochTrackerOutput::WithNewTargetEpoch {
                target_epoch_state,
                current_epoch_state,
                current_epoch_tracker,
            } => Self::WithTargetEpoch {
                target_epoch_state,
                target_epoch_tracker: Box::new(target_epoch_tracker),
                current_epoch_state,
                current_epoch_tracker,
            },
            CurrentEpochTrackerOutput::WithoutNewTargetEpoch {
                current_epoch_state,
                current_epoch_tracker,
            } => Self::WithoutTargetEpoch {
                current_epoch_state,
                current_epoch_tracker,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RewardsParameters {
    pub epoch_length: Slot,
    pub message_frequency_per_slot: NonNegativeF64,
    pub num_blend_layers: NonZeroU64,
    pub data_replication_factor: u64,
    pub minimum_network_size: NonZeroU64,
    pub activity_threshold_sensitivity: u64,
}

impl RewardsParameters {
    fn core_quota_and_token_evaluation(
        &self,
        num_core_nodes: u64,
    ) -> Result<(u64, BlendingTokenEvaluation), lb_blend_message::reward::Error> {
        let core_quota = core_quota(
            self.epoch_length
                .into_inner()
                .try_into()
                .expect("must be non-zero"),
            self.message_frequency_per_slot,
            self.num_blend_layers,
            num_core_nodes as usize,
        );
        Ok((
            core_quota,
            BlendingTokenEvaluation::new(
                core_quota,
                num_core_nodes,
                self.activity_threshold_sensitivity,
            )?,
        ))
    }

    fn leader_inputs(&self, epoch_state: &EpochState) -> LeaderInputs {
        let num_blend_layers = self.num_blend_layers.get();
        let message_quota = num_blend_layers + (num_blend_layers * self.data_replication_factor);
        LeaderInputs {
            pol_ledger_aged: epoch_state.utxos.root(),
            pol_epoch_nonce: epoch_state.nonce,
            message_quota,
            lottery_0: epoch_state.lottery_0,
            lottery_1: epoch_state.lottery_1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, convert::Infallible};

    use lb_blend_message::crypto::proofs::PoQVerificationInputsMinusSigningKey;
    use lb_blend_proofs::{
        quota::{ProofOfQuota, VerifiedProofOfQuota},
        selection::{ProofOfSelection, VerifiedProofOfSelection, inputs::VerifyInputs},
    };
    use lb_core::sdp::{ServiceType, blend};
    use lb_key_management_system_keys::keys::{Ed25519Key, Ed25519PublicKey};

    use super::*;
    use crate::mantle::sdp::rewards::{
        Rewards as _,
        test_utils::{
            create_provider_id, create_service_parameters, create_test_snapshot, dummy_epoch_state,
            dummy_epoch_state_with,
        },
    };

    fn create_blend_rewards_params(
        epoch_length: Slot,
        minimum_network_size: u64,
    ) -> RewardsParameters {
        RewardsParameters {
            epoch_length,
            message_frequency_per_slot: NonNegativeF64::try_from(1.0).unwrap(),
            num_blend_layers: NonZeroU64::new(3).unwrap(),
            minimum_network_size: minimum_network_size.try_into().unwrap(),
            data_replication_factor: 0,
            activity_threshold_sensitivity: 1,
        }
    }

    fn new_proof_of_quota_unchecked(byte: u8) -> ProofOfQuota {
        VerifiedProofOfQuota::from_bytes_unchecked([byte; _]).into()
    }

    fn new_signing_key(byte: u8) -> Ed25519PublicKey {
        Ed25519Key::from_bytes(&[byte; _]).public_key()
    }

    fn new_proof_of_selection_unchecked(byte: u8) -> ProofOfSelection {
        VerifiedProofOfSelection::from_bytes_unchecked([byte; _]).into()
    }

    #[test]
    fn test_blend_no_reward_calculated_after_epoch_0() {
        // Create a reward tracker
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(864_000.into(), 1);
        let rewards_tracker = Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state);

        // Create the epoch-0 snapshot with providers
        let session_0 = create_test_snapshot(
            &[create_provider_id(1), create_provider_id(2)],
            ServiceType::BlendNetwork,
            epoch_state.epoch,
        );

        // Update epoch from 0 to 1
        let (_, rewards) = rewards_tracker.update_epoch(
            &session_0,
            &dummy_epoch_state(1.into()),
            &create_service_parameters(),
            &params,
        );

        // No rewards should be returned yet because epoch-0 just ended,
        // and the reward calculation for the epoch-0 just began.
        assert_eq!(rewards.len(), 0);
    }

    #[test]
    fn test_rewards_with_no_activity_proofs() {
        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(864_000.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(
                    &[create_provider_id(1), create_provider_id(2)],
                    ServiceType::BlendNetwork,
                    epoch_state.epoch,
                ),
                &dummy_epoch_state(1.into()),
                &config,
                &params,
            );

        // Update epoch from 1 to 2 without any activity proofs submitted.
        let (_, rewards) = rewards_tracker.update_epoch(
            &create_test_snapshot(
                &[create_provider_id(1), create_provider_id(2)],
                ServiceType::BlendNetwork,
                2.into(),
            ),
            &epoch_state,
            &config,
            &params,
        );
        assert_eq!(rewards.len(), 0);
    }

    #[test]
    fn test_rewards_calculation() {
        let provider1 = create_provider_id(1);
        let provider2 = create_provider_id(2);
        let provider3 = create_provider_id(3);
        let provider4 = create_provider_id(4);

        // Create a reward tracker, accumulate income during epoch 0,
        // and update session from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(864_000.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state)
                .add_income(1000)
                .update_epoch(
                    &create_test_snapshot(
                        &[provider1, provider2, provider3, provider4],
                        ServiceType::BlendNetwork,
                        epoch_state.epoch(),
                    ),
                    &dummy_epoch_state(1.into()),
                    &config,
                    &params,
                );

        // provider1 submits an activity proof, which has the minimum
        // Hamming distance in the current test configs.
        let rewards_tracker = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap();

        // provider2 submits an activity proof, which has a larger
        // Hamming distance than provider1's proof in the current test configs.
        let rewards_tracker = rewards_tracker
            .update_active(
                provider2,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(2),
                    signing_key: new_signing_key(2),
                    proof_of_selection: new_proof_of_selection_unchecked(2),
                })),
                &params,
            )
            .unwrap();

        // provider3 submits an activity proof, which has the minimum
        // Hamming distance in the current test configs.
        let rewards_tracker = rewards_tracker
            .update_active(
                provider3,
                // Use the same proof as provider1 just for testing
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap();

        // provider4 doesn't submit an activity proof.

        // Update epoch from 1 to 2.
        let (_, reward_utxos) = rewards_tracker.update_epoch(
            &create_test_snapshot(
                &[provider1, provider2, provider3, provider4],
                ServiceType::BlendNetwork,
                1.into(),
            ),
            &dummy_epoch_state(2.into()),
            &config,
            &params,
        );

        assert_eq!(reward_utxos.len(), 3); // except provider4

        let Rewards::WithTargetEpoch {
            target_epoch_state: target_session_state,
            ..
        } = rewards_tracker
        else {
            panic!("rewards_tracker should be in Initialized state");
        };
        let zk_id_to_provider_id = target_session_state
            .providers()
            .map(|(provider_id, (zk_id, _))| (*zk_id, *provider_id))
            .collect::<HashMap<_, _>>();
        let rewards: HashMap<ProviderId, u64> = reward_utxos
            .iter()
            .map(|utxo| {
                let provider_id = zk_id_to_provider_id
                    .get(&utxo.note.pk)
                    .expect("provider should exist");
                (*provider_id, utxo.note.value)
            })
            .collect();

        // Provider2 gets 1/2 rewards compared to provider1 and provider3.
        assert_eq!(
            *rewards.get(&provider1).unwrap(),
            rewards.get(&provider2).unwrap() * 2
        );
        assert_eq!(
            *rewards.get(&provider3).unwrap(),
            rewards.get(&provider2).unwrap() * 2
        );
        // Provider4 should get no rewards.
        assert_eq!(rewards.get(&provider4), None);
    }

    #[test]
    fn test_blend_duplicate_active_messages() {
        let provider1 = create_provider_id(1);

        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(864_000.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, epoch_state.epoch()),
                &dummy_epoch_state(1.into()),
                &config,
                &params,
            );

        // provider1 submits an activity proof.
        let rewards_tracker = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap();

        // provider1 submits another activity proof in the same session,
        // which should error.
        let err = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(2),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(2),
                })),
                &params,
            )
            .unwrap_err();
        assert_eq!(
            err,
            Error::DuplicateActiveMessage {
                epoch: 0.into(),
                provider_id: Box::new(provider1)
            }
        );
    }

    #[test]
    fn test_blend_invalid_session() {
        let provider1 = create_provider_id(1);

        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(864_000.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, epoch_state.epoch()),
                &dummy_epoch_state(1.into()),
                &config,
                &params,
            );

        // provider1 submits an activity proof with invalid session.
        let err = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 99.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap_err();
        assert_eq!(
            err,
            Error::InvalidEpoch {
                expected: 0.into(),
                got: 99.into()
            }
        );

        // No reward should be calculated after session 1.
        let (_, rewards) = rewards_tracker.update_epoch(
            &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, 1.into()),
            &epoch_state,
            &config,
            &params,
        );
        assert_eq!(rewards.len(), 0);
    }

    #[test]
    fn test_blend_network_too_small() {
        let provider1 = create_provider_id(1);

        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        // Set minimum network size to 2
        let params = create_blend_rewards_params(864_000.into(), 2);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, epoch_state.epoch()),
                &dummy_epoch_state(1.into()),
                &config,
                &params,
            );

        // provider1 submits an activity proof, but it should be rejected
        // since the network is too small.
        let err = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap_err();
        assert_eq!(err, Error::TargetSessionNotSet);

        // No reward should be calculated after session 1.
        let (_, rewards) = rewards_tracker.update_epoch(
            &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, 1.into()),
            &epoch_state,
            &config,
            &params,
        );
        assert_eq!(rewards.len(), 0);
    }

    #[test]
    fn test_blend_proof_distance_larger_than_activity_threshold() {
        let provider1 = create_provider_id(1);

        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(10.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysSuccessProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, epoch_state.epoch()),
                &dummy_epoch_state_with(1.into(), 99999),
                &config,
                &params,
            );

        // provider1 submits an activity proof that is larger than activity threshold.
        let err = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(2),
                    signing_key: new_signing_key(2),
                    proof_of_selection: new_proof_of_selection_unchecked(2),
                })),
                &params,
            )
            .unwrap_err();
        assert_eq!(err, Error::InvalidProof);

        // No reward should be calculated after session 1.
        let (_, rewards) = rewards_tracker.update_epoch(
            &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, 1.into()),
            &epoch_state,
            &config,
            &params,
        );
        assert_eq!(rewards.len(), 0);
    }

    #[test]
    fn test_blend_invalid_proofs() {
        let provider1 = create_provider_id(1);

        // Create a reward tracker, and update epoch from 0 to 1.
        let config = create_service_parameters();
        let epoch_state = dummy_epoch_state(0.into());
        let params = create_blend_rewards_params(1000.into(), 1);
        let (rewards_tracker, _) =
            Rewards::<AlwaysFailureProofsVerifier>::new(&params, &epoch_state).update_epoch(
                &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, epoch_state.epoch()),
                &dummy_epoch_state(1.into()),
                &config,
                &params,
            );

        // provider1 submits an activity proof, but PoQ/PoSel verification fails.
        let err = rewards_tracker
            .update_active(
                provider1,
                &ActivityMetadata::Blend(Box::new(blend::ActivityProof {
                    epoch: 0.into(),
                    proof_of_quota: new_proof_of_quota_unchecked(1),
                    signing_key: new_signing_key(1),
                    proof_of_selection: new_proof_of_selection_unchecked(1),
                })),
                &params,
            )
            .unwrap_err();
        assert_eq!(err, Error::InvalidProof);

        // No reward should be calculated after epoch 1.
        let (_, rewards) = rewards_tracker.update_epoch(
            &create_test_snapshot(&[provider1], ServiceType::BlendNetwork, 1.into()),
            &dummy_epoch_state(2.into()),
            &config,
            &params,
        );
        assert_eq!(rewards.len(), 0);
    }

    #[derive(Debug, Clone, PartialEq)]
    struct AlwaysSuccessProofsVerifier;

    impl ProofsVerifierTrait for AlwaysSuccessProofsVerifier {
        type Error = Infallible;

        fn new(_public_inputs: PoQVerificationInputsMinusSigningKey) -> Self {
            Self
        }

        fn start_epoch_transition(&mut self, _new_pol_inputs: LeaderInputs) {}

        fn complete_epoch_transition(&mut self) {}

        fn verify_proof_of_quota(
            &self,
            proof: ProofOfQuota,
            _signing_key: &Ed25519PublicKey,
        ) -> Result<VerifiedProofOfQuota, Self::Error> {
            Ok(VerifiedProofOfQuota::from_bytes_unchecked((&proof).into()))
        }

        fn verify_proof_of_selection(
            &self,
            proof: ProofOfSelection,
            _inputs: &VerifyInputs,
        ) -> Result<VerifiedProofOfSelection, Self::Error> {
            Ok(VerifiedProofOfSelection::from_bytes_unchecked(
                (&proof).into(),
            ))
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct AlwaysFailureProofsVerifier;

    impl ProofsVerifierTrait for AlwaysFailureProofsVerifier {
        type Error = ();

        fn new(_public_inputs: PoQVerificationInputsMinusSigningKey) -> Self {
            Self
        }

        fn start_epoch_transition(&mut self, _new_pol_inputs: LeaderInputs) {}

        fn complete_epoch_transition(&mut self) {}

        fn verify_proof_of_quota(
            &self,
            _proof: ProofOfQuota,
            _signing_key: &Ed25519PublicKey,
        ) -> Result<VerifiedProofOfQuota, Self::Error> {
            Err(())
        }

        fn verify_proof_of_selection(
            &self,
            _proof: ProofOfSelection,
            _inputs: &VerifyInputs,
        ) -> Result<VerifiedProofOfSelection, Self::Error> {
            Err(())
        }
    }
}

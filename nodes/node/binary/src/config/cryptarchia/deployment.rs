use core::num::{NonZero, NonZeroU32};
use std::collections::HashMap;

use lb_core::{
    block::BlockNumber,
    mantle::genesis_tx::GenesisTx,
    sdp::{MinStake, ServiceType},
};
use lb_cryptarchia_engine::Config as ConsensusConfig;
use lb_key_management_system_service::keys::ZkPublicKey;
use lb_utils::math::{NonNegativeF64, NonNegativeRatio};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Settings {
    pub epoch_config: EpochConfig,
    pub security_param: NonZeroU32,
    pub slot_activation_coeff: NonNegativeRatio,
    pub learning_rate: NonNegativeF64,
    pub sdp_config: SdpConfig,
    pub gossipsub_protocol: String,
    pub genesis_state: GenesisTx,
    #[serde(default)]
    pub faucet_pk: Option<ZkPublicKey>,
}

impl Settings {
    #[must_use]
    pub const fn slots_per_epoch(&self) -> u64 {
        (self.blocks_per_epoch() as f64 / self.slot_activation_coeff.as_f64()).ceil() as u64
    }

    // Session duration is given by epoch schedule * `k` (security parameter).
    #[must_use]
    pub const fn blocks_per_epoch(&self) -> u64 {
        self.epoch_schedule() * self.security_param.get() as u64
    }

    #[must_use]
    pub const fn average_slots_per_block(&self) -> u64 {
        (self.slot_activation_coeff.denominator.get() * self.slot_activation_coeff.numerator) as u64
    }

    const fn epoch_schedule(&self) -> u64 {
        (self.epoch_config.epoch_period_nonce_buffer.get()
            + self.epoch_config.epoch_period_nonce_stabilization.get()
            + self
                .epoch_config
                .epoch_stake_distribution_stabilization
                .get()) as u64
    }

    #[must_use]
    pub fn consensus_config(&self) -> ConsensusConfig {
        ConsensusConfig::new(
            self.security_param,
            self.slot_activation_coeff,
            self.learning_rate,
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochConfig {
    // The stake distribution is always taken at the beginning of the previous epoch.
    // This parameters controls how many slots to wait for it to be stabilized
    // The value is computed as epoch_stake_distribution_stabilization * int(floor(k / f))
    pub epoch_stake_distribution_stabilization: NonZero<u8>,
    // This parameter controls how many slots we wait after the stake distribution
    // snapshot has stabilized to take the nonce snapshot.
    pub epoch_period_nonce_buffer: NonZero<u8>,
    // This parameter controls how many slots we wait for the nonce snapshot to be considered
    // stabilized
    pub epoch_period_nonce_stabilization: NonZero<u8>,
}

// The same as `lb_ledger::mantle::sdp::Config`, minus the
// `service_rewards_params` values, which are taken from the Blend deployment
// config instead.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SdpConfig {
    pub service_params: HashMap<ServiceType, ServiceParameters>,
    pub min_stake: MinStake,
}

// The same as `lb_core::sdp::ServiceParameters`, minus the
// `session_duration` values which are calculated from the other values
// provided.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServiceParameters {
    pub lock_period: u64,
    pub inactivity_period: u64,
    pub retention_period: u64,
    pub timestamp: BlockNumber,
}

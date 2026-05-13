use lb_core::{
    crypto::ZkHash,
    sdp::{Declaration, DeclarationId, ProviderId, ServiceParameters, ServiceType},
};
use lb_cryptarchia_engine::Epoch;
use lb_groth16::{Field as _, Fr};
use lb_key_management_system_keys::keys::{Ed25519Key, ZkPublicKey};
use num_bigint::BigUint;

use crate::{EpochState, UtxoTree, mantle::sdp::Snapshot};

pub fn create_test_snapshot(
    provider_ids: &[ProviderId],
    service_type: ServiceType,
    epoch: Epoch,
) -> Snapshot {
    let mut declarations = rpds::RedBlackTreeMapSync::new_sync();
    for (i, provider_id) in provider_ids.iter().enumerate() {
        let declaration = Declaration {
            service_type,
            provider_id: *provider_id,
            locked_note_id: Fr::from(i as u64).into(),
            locators: vec![],
            zk_id: ZkPublicKey::new(BigUint::from(i as u64).into()),
            created: 0.into(),
            active: 0.into(),
            withdrawn: None,
            nonce: 0,
        };
        declarations = declarations.insert(DeclarationId([i as u8; 32]), declaration);
    }
    Snapshot {
        declarations,
        epoch,
    }
}

pub fn create_provider_id(byte: u8) -> ProviderId {
    let key_bytes = [byte; 32];
    // Ensure the key is valid by using SigningKey
    let signing_key = Ed25519Key::from_bytes(&key_bytes);
    ProviderId(signing_key.public_key())
}

pub fn create_service_parameters() -> ServiceParameters {
    ServiceParameters {
        lock_period: 10.into(),
        inactivity_period: 1.into(),
        retention_period: 1.into(),
        epoch: 0.into(),
    }
}

pub fn dummy_epoch_state(epoch: Epoch) -> EpochState {
    dummy_epoch_state_with(epoch, 0)
}

pub fn dummy_epoch_state_with(epoch: Epoch, nonce: u64) -> EpochState {
    EpochState {
        epoch,
        nonce: ZkHash::from(BigUint::from(nonce)),
        utxos: UtxoTree::default(),
        total_stake: 0,
        lottery_0: Fr::ZERO,
        lottery_1: Fr::ZERO,
    }
}

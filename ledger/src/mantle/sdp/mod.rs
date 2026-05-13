pub mod rewards;

use std::collections::HashMap;

use lb_blend_message::crypto::proofs::RealProofsVerifier;
use lb_core::{
    block::BlockNumber,
    mantle::{
        NoteId, OpProof, TxHash, Utxo, Value,
        ledger::Operation as _,
        ops::sdp::{
            SDPActiveExecutionContext, SDPActiveOp, SDPActiveValidationContext,
            SDPDeclareExecutionContext, SDPDeclareOp, SDPDeclareValidationContext,
            SDPWithdrawExecutionContext, SDPWithdrawOp, SDPWithdrawValidationContext,
        },
    },
    sdp::{
        ActivityMetadata, Declaration, DeclarationId, MinStake, Nonce, ProviderId, ProviderInfo,
        ServiceParameters, ServiceType, locked_notes, locked_notes::LockedNotes,
    },
};
use lb_cryptarchia_engine::Epoch;
use lb_key_management_system_keys::keys::{Ed25519Signature, ZkSignature};
use rewards::{Error as RewardsError, Rewards};
use tracing::warn;

use crate::{EpochState, UtxoTree, mantle::sdp::rewards::blend};

type Declarations = rpds::RedBlackTreeMapSync<DeclarationId, Declaration>;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Service {
    BlendNetwork(ServiceState<blend::Rewards<RealProofsVerifier>>),
}

impl Service {
    fn try_apply_header(
        self,
        epoch_state: &EpochState,
        config: &ServiceParameters,
        rewards_params: &ServiceRewardsParameters,
    ) -> (Self, Vec<Utxo>) {
        match self {
            Self::BlendNetwork(state) => {
                let (new_state, utxos) =
                    state.try_apply_header(epoch_state, config, &rewards_params.blend);
                (Self::BlendNetwork(new_state), utxos)
            }
        }
    }

    fn contains(&self, declaration_id: &DeclarationId) -> bool {
        match self {
            Self::BlendNetwork(state) => state.contains(declaration_id),
        }
    }

    /// The snapshot of declarations that are active in the current epoch
    const fn active_snapshot(&self) -> &Snapshot {
        match self {
            Self::BlendNetwork(state) => &state.active,
        }
    }

    /// The snapshot of declarations that will become active in the next epoch
    #[cfg(test)]
    const fn next_snapshot(&self) -> &Snapshot {
        match self {
            Self::BlendNetwork(state) => &state.next,
        }
    }

    const fn declarations(&self) -> &Declarations {
        match self {
            Self::BlendNetwork(state) => &state.declarations,
        }
    }

    pub fn declarations_clone(&self) -> Declarations {
        match self {
            Self::BlendNetwork(state) => state.declarations.clone(),
        }
    }

    pub fn update_declarations(&mut self, declarations: Declarations) {
        match self {
            Self::BlendNetwork(state) => state.declarations = declarations,
        }
    }

    pub fn update_rewards(
        &mut self,
        provider_id: ProviderId,
        metadata: &ActivityMetadata,
        rewards_params: &ServiceRewardsParameters,
    ) -> Result<(), Error> {
        match self {
            Self::BlendNetwork(state) => {
                state.rewards =
                    state
                        .rewards
                        .update_active(provider_id, metadata, &rewards_params.blend)?;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub service_params: std::sync::Arc<HashMap<ServiceType, ServiceParameters>>,
    pub service_rewards_params: ServiceRewardsParameters,
    pub min_stake: MinStake,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceRewardsParameters {
    pub blend: blend::RewardsParameters,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    // #[error("Invalid Sdp state transition: {0:?}")]
    // SdpStateError(#[from] DeclarationStateError),
    #[error("Sdp declaration id not found: {0:?}")]
    DeclarationNotFound(DeclarationId),
    #[error("Locked period did not pass yet")]
    WithdrawalWhileLocked,
    #[error(
        "Invalid sdp message nonce: message_nonce={message_nonce:?}, declaration_nonce={declaration_nonce:?}"
    )]
    InvalidNonce {
        message_nonce: Nonce,
        declaration_nonce: Nonce,
    },
    #[error("Service not found: {0:?}")]
    ServiceNotFound(ServiceType),
    #[error("Duplicate sdp declaration id: {0:?}")]
    DuplicateDeclaration(DeclarationId),
    #[error("Active session for service {0:?} not found")]
    ActiveSessionNotFound(ServiceType),
    #[error("Next session for service {0:?} not found")]
    NextSessionNotFound(ServiceType),
    #[error("Session parameters for {0:?} not found")]
    SessionParamsNotFound(ServiceType),
    #[error("Service parameters are missing for {0:?}")]
    ServiceParamsNotFound(ServiceType),
    #[error("Can't update genesis state during different block number")]
    NotGenesisBlock,
    #[error("Time travel detected, current: {current:?}, incoming: {incoming:?}")]
    TimeTravel {
        current: BlockNumber,
        incoming: BlockNumber,
    },
    #[error("Something went wrong while locking/unlocking a note: {0:?}")]
    LockingError(#[from] locked_notes::Error),
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Note not found: {0:?}")]
    NoteNotFound(NoteId),
    #[error("Invalid proof")]
    InvalidProof,
    #[error("Error while computing rewards: {0:?}")]
    RewardsError(#[from] RewardsError),
    #[error(transparent)]
    SdpOp(#[from] lb_core::mantle::ops::sdp::SdpError),
}

/// Snapshot that becomes active at the `epoch`
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub declarations: Declarations,
    pub epoch: Epoch,
}

const SNAPSHOT_FINALIZATION_DELAY: Epoch = Epoch::new(2);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ServiceState<R: Rewards> {
    /// Declarations accumulated until the current block.
    declarations: Declarations,
    /// Active declarations in the current epoch.
    /// This was snapshotted at last block from the epoch `current_epoch-2`.
    active: Snapshot,
    /// Declarations that will become active in the next epoch.
    /// This was snapshotted at last block from the epoch `current_epoch-1`.
    next: Snapshot,
    /// Rewards calculation and tracking for this service
    pub rewards: R,
}

fn is_active(declaration: &Declaration, current_epoch: Epoch, config: &ServiceParameters) -> bool {
    declaration.active
        + config.inactivity_period
        + config.retention_period
        + SNAPSHOT_FINALIZATION_DELAY
        >= current_epoch
}

impl<R: Rewards> ServiceState<R> {
    fn try_apply_header(
        mut self,
        epoch_state: &EpochState,
        service_params: &ServiceParameters,
        rewards_params: &R::Params,
    ) -> (Self, Vec<Utxo>) {
        let reward_utxos;

        // shift epoch
        if epoch_state.epoch() == self.active.epoch + 1 {
            // Remove expired declarations based on retention_period
            // This essentially duplicates the declaration set so it's only triggered at
            // epoch boundaries
            self.declarations = self
                .declarations
                .iter()
                .filter(|(_id, declaration)| {
                    let active = is_active(declaration, epoch_state.epoch(), service_params);
                    if !active {
                        warn!(
                            provider_id = ?declaration.provider_id,
                            latest_active_epoch = ?declaration.active,
                            current_epoch = ?epoch_state.epoch(),
                            service_params = ?service_params,
                            "removing declaration due to inactivity + retention + finalization_delay"
                        );
                    }
                    active
                })
                .map(|(id, declaration)| (*id, declaration.clone()))
                .collect();

            // Update rewards with current session state and distribute rewards
            (self.rewards, reward_utxos) = self.rewards.update_epoch(
                &self.active,
                epoch_state,
                service_params,
                rewards_params,
            );
            self.active = self.next.clone();
            self.next = Snapshot {
                declarations: self.declarations.clone(),
                epoch: self.next.epoch + 1,
            };
        } else {
            assert!(
                epoch_state.epoch() == self.active.epoch,
                "Logos blockchain isn't ready for time travel yet: current_epoch={:?}, active_snapshot_epoch={:?}",
                epoch_state.epoch(),
                self.active.epoch,
            );
            reward_utxos = Vec::new();
        }

        (self, reward_utxos)
    }

    fn add_income(&mut self, income: Value) {
        self.rewards = self.rewards.add_income(income);
    }

    fn contains(&self, declaration_id: &DeclarationId) -> bool {
        self.declarations.contains_key(declaration_id)
    }
}

/// A SDP state of the mantle ledger
///
/// NOTE: Most collection fields in this struct should use `rpds`
/// since we keep a copy of this state for each block.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SdpLedger {
    services: rpds::HashTrieMapSync<ServiceType, Service>,
    locked_notes: LockedNotes,
    // The epoch when this ledger was created
    epoch: Epoch,
}

impl SdpLedger {
    #[must_use]
    pub fn new(epoch: Epoch) -> Self {
        Self {
            services: rpds::HashTrieMapSync::new_sync(),
            locked_notes: LockedNotes::new(),
            epoch,
        }
    }

    pub fn from_genesis<'a>(
        config: &Config,
        utxo_tree: &UtxoTree,
        epoch_state: &EpochState,
        tx_hash: TxHash,
        ops: impl Iterator<Item = (&'a SDPDeclareOp, &'a OpProof)> + 'a,
    ) -> Result<Self, Error> {
        let mut sdp = Self::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, epoch_state);

        for (op, proof) in ops {
            let OpProof::ZkAndEd25519Sigs {
                zk_sig,
                ed25519_sig,
            } = proof
            else {
                return Err(Error::InvalidProof);
            };
            sdp =
                sdp.try_apply_sdp_declaration(utxo_tree, op, zk_sig, ed25519_sig, tx_hash, config)?;
        }

        let blend = sdp
            .services
            .get_mut(&ServiceType::BlendNetwork)
            .expect("SDP initialized with Blend in this method");

        let Service::BlendNetwork(state) = blend;
        state.active.declarations = state.declarations.clone();
        state.next.declarations = state.declarations.clone();

        Ok(sdp)
    }

    #[must_use]
    pub fn with_blend_service(
        mut self,
        rewards_settings: &blend::RewardsParameters,
        epoch_state: &EpochState,
    ) -> Self {
        assert_eq!(
            epoch_state.epoch, self.epoch,
            "TODO: refactor to remove this assertion"
        );
        let service = Service::BlendNetwork(Self::new_service_state(blend::Rewards::new(
            rewards_settings,
            epoch_state,
        )));
        self.services = self.services.insert(ServiceType::BlendNetwork, service);
        self
    }

    #[must_use]
    fn new_service_state<R: Rewards>(rewards: R) -> ServiceState<R> {
        ServiceState {
            declarations: rpds::RedBlackTreeMapSync::new_sync(),
            active: Snapshot {
                declarations: rpds::RedBlackTreeMapSync::new_sync(),
                epoch: 0.into(),
            },
            next: Snapshot {
                declarations: rpds::RedBlackTreeMapSync::new_sync(),
                epoch: 1.into(),
            },
            rewards,
        }
    }

    pub fn try_apply_header(
        &self,
        config: &Config,
        epoch_state: &EpochState,
    ) -> Result<(Self, Vec<Utxo>), Error> {
        let mut all_reward_utxos = Vec::new();

        let services = self
            .services
            .iter()
            .map(|(service, service_state)| {
                let service_params = config
                    .service_params
                    .get(service)
                    .ok_or(Error::SessionParamsNotFound(*service))?;
                let (new_state, reward_utxos) = service_state.clone().try_apply_header(
                    epoch_state,
                    service_params,
                    &config.service_rewards_params,
                );
                all_reward_utxos.extend(reward_utxos);
                Ok::<_, Error>((*service, new_state))
            })
            .collect::<Result<_, _>>()?;

        Ok((
            Self {
                epoch: epoch_state.epoch(),
                services,
                locked_notes: self.locked_notes.clone(),
            },
            all_reward_utxos,
        ))
    }

    pub fn try_apply_sdp_declaration(
        mut self,
        utxo_tree: &UtxoTree,
        op: &SDPDeclareOp,
        zk_sig: &ZkSignature,
        ed25519_sig: &Ed25519Signature,
        tx_hash: TxHash,
        config: &Config,
    ) -> Result<Self, Error> {
        let Some(service_state) = self.services.get_mut(&op.service_type) else {
            return Err(Error::ServiceNotFound(op.service_type));
        };

        // Validate SDP Declare
        op.validate(&SDPDeclareValidationContext {
            utxo_tree,
            locked_notes: &self.locked_notes,
            tx_hash: &tx_hash,
            declare_zk_sig: zk_sig,
            declare_eddsa_sig: ed25519_sig,
            declarations: service_state.declarations(),
            min_stake: &config.min_stake,
        })?;

        // Execute SDP Declare
        let result = op.execute(SDPDeclareExecutionContext {
            utxo_tree: utxo_tree.clone(),
            epoch: self.epoch,
            declarations: service_state.declarations_clone(),
            locked_notes: self.locked_notes.clone(),
            min_stake: config.min_stake,
        })?;

        self.locked_notes = result.locked_notes;
        service_state.update_declarations(result.declarations);
        Ok(self)
    }

    pub fn apply_active_msg(
        mut self,
        op: &SDPActiveOp,
        zksig: &ZkSignature,
        tx_hash: TxHash,
        config: &Config,
    ) -> Result<Self, Error> {
        let (service, _) = self.get_service(&op.declaration_id, config)?;
        let Some(service_state) = self.services.get_mut(&service) else {
            return Err(Error::ServiceNotFound(service));
        };

        //Validate SDP Active
        op.validate(&SDPActiveValidationContext {
            declarations: service_state.declarations(),
            tx_hash: &tx_hash,
            active_sig: zksig,
        })?;

        // Execute SDP Active
        let result = op.execute(SDPActiveExecutionContext {
            epoch: self.epoch,
            declarations: service_state.declarations_clone(),
        })?;

        let provider_id = result
            .declarations
            .get(&op.declaration_id)
            .expect("the declaration should be in the list after execution")
            .provider_id;

        service_state.update_declarations(result.declarations);
        service_state.update_rewards(provider_id, &op.metadata, &config.service_rewards_params)?;

        Ok(self)
    }

    pub fn apply_withdrawn_msg(
        mut self,
        op: &SDPWithdrawOp,
        zksig: &ZkSignature,
        tx_hash: TxHash,
        config: &Config,
    ) -> Result<Self, Error> {
        let (service, config) = self.get_service(&op.declaration_id, config)?;
        let Some(service_state) = self.services.get_mut(&service) else {
            return Err(Error::ServiceNotFound(service));
        };

        // Validate SDP Withdraw
        op.validate(&SDPWithdrawValidationContext {
            lock_period: config.lock_period,
            declarations: service_state.declarations(),
            epoch: self.epoch,
            locked_notes: &self.locked_notes,
            tx_hash: &tx_hash,
            sdp_withdraw_sig: zksig,
        })?;

        // Execute SDP Withdraw
        let result = op.execute(SDPWithdrawExecutionContext {
            declarations: service_state.declarations_clone(),
            locked_notes: self.locked_notes.clone(),
        })?;

        self.locked_notes = result.locked_notes;
        service_state.update_declarations(result.declarations);

        Ok(self)
    }

    pub fn add_blend_income(&mut self, income: Value) {
        if let Some(Service::BlendNetwork(state)) =
            self.services.get_mut(&ServiceType::BlendNetwork)
        {
            state.add_income(income);
        }
    }

    #[must_use]
    pub const fn locked_notes(&self) -> &LockedNotes {
        &self.locked_notes
    }

    /// Providers in the active snapshot of the current epoch
    #[must_use]
    pub fn active_providers(
        &self,
        service_type: ServiceType,
    ) -> Option<HashMap<ProviderId, ProviderInfo>> {
        let service = self.services.get(&service_type)?;

        let providers = service
            .active_snapshot()
            .declarations
            .iter()
            .map(|(_, declaration)| {
                (
                    declaration.provider_id,
                    ProviderInfo {
                        locators: declaration.locators.clone(),
                        zk_id: declaration.zk_id,
                    },
                )
            })
            .collect();

        Some(providers)
    }

    /// The epoch of the current active snapshot for each service
    #[must_use]
    pub fn active_snapshot_epochs(&self) -> HashMap<ServiceType, Epoch> {
        self.services
            .iter()
            .map(|(service_type, service)| (*service_type, service.active_snapshot().epoch))
            .collect()
    }

    /// Declarations of all services, which have been accumulated until the
    /// current block.
    ///
    /// This may be different from declarations in the active/next snapshots.
    #[must_use]
    pub fn declarations(&self) -> Vec<(DeclarationId, Declaration)> {
        self.services
            .iter()
            .flat_map(|(_, service_state)| {
                service_state
                    .declarations()
                    .iter()
                    .map(|(declaration_id, declaration)| (*declaration_id, declaration.clone()))
            })
            .collect()
    }

    /// Get a declaration by ID from the set of declarations accumulated until
    /// the current block (not from snapshots).
    #[must_use]
    pub fn get_declaration(&self, declaration_id: &DeclarationId) -> Option<&Declaration> {
        self.services.iter().find_map(|(_, service)| {
            let declarations = match service {
                Service::BlendNetwork(state) => &state.declarations,
            };
            declarations.get(declaration_id)
        })
    }

    fn get_service<'a>(
        &self,
        declaration_id: &DeclarationId,
        config: &'a Config,
    ) -> Result<(ServiceType, &'a ServiceParameters), Error> {
        let service = self
            .services
            .iter()
            .find(|(_, state)| state.contains(declaration_id))
            .map(|(service, _)| *service)
            .ok_or(Error::DeclarationNotFound(*declaration_id))?;

        let params = config
            .service_params
            .get(&service)
            .ok_or(Error::ServiceParamsNotFound(service))?;
        Ok((service, params))
    }

    #[cfg(test)]
    fn get_next_snapshot(&self, service_type: ServiceType) -> Option<&Snapshot> {
        self.services.get(&service_type).map(Service::next_snapshot)
    }

    #[cfg(test)]
    fn get_active_snapshot(&self, service_type: ServiceType) -> Option<&Snapshot> {
        self.services
            .get(&service_type)
            .map(Service::active_snapshot)
    }

    #[cfg(test)]
    fn get_declarations(&self, service_type: ServiceType) -> Option<&Declarations> {
        self.services.get(&service_type).map(Service::declarations)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use lb_core::{crypto::ZkHash, mantle::ledger::Utxos};
    use lb_groth16::{Field as _, Fr};
    use lb_key_management_system_keys::keys::{Ed25519Key, ZkKey};
    use lb_utils::math::NonNegativeF64;
    use num_bigint::BigUint;

    use super::*;
    use crate::cryptarchia::tests::{utxo, utxo_with_sk};

    fn setup() -> Config {
        setup_with(ServiceParameters {
            inactivity_period: 1.into(),
            lock_period: 10.into(),
            retention_period: 1.into(),
            epoch: 0.into(),
        })
    }

    fn setup_with(service_params: ServiceParameters) -> Config {
        let mut params = HashMap::new();
        params.insert(ServiceType::BlendNetwork, service_params);
        Config {
            service_params: Arc::new(params),
            service_rewards_params: ServiceRewardsParameters {
                blend: blend::RewardsParameters {
                    epoch_length: 10.into(),
                    message_frequency_per_slot: NonNegativeF64::try_from(1.0).unwrap(),
                    num_blend_layers: NonZeroU64::new(3).unwrap(),
                    minimum_network_size: NonZeroU64::new(1).unwrap(),
                    data_replication_factor: 0,
                    activity_threshold_sensitivity: 1,
                },
            },
            min_stake: MinStake {
                threshold: 1,
                timestamp: 0,
            },
        }
    }

    fn create_zk_key(sk: u64) -> ZkKey {
        ZkKey::from(BigUint::from(sk))
    }

    fn create_signing_key() -> Ed25519Key {
        Ed25519Key::from_bytes(&[0; 32])
    }

    fn utxo_tree(utxos: Vec<Utxo>) -> Utxos {
        let mut utxo_tree = Utxos::new();
        for utxo in utxos {
            (utxo_tree, _) = utxo_tree.insert(utxo.id(), utxo);
        }
        utxo_tree
    }

    fn apply_declare_with_dummies(
        utxos: &Utxos,
        sdp_ledger: SdpLedger,
        op: &SDPDeclareOp,
        zk_sk: &ZkKey,
        config: &Config,
    ) -> Result<SdpLedger, Error> {
        let (note_sk, _) = utxo_with_sk();
        let tx_hash = TxHash([0u8; 32]);
        let zk_sig = ZkKey::multi_sign(&[note_sk, zk_sk.clone()], &tx_hash.to_fr()).unwrap();

        let signing_key = create_signing_key();
        let ed25519_sig = signing_key.sign_payload(tx_hash.as_signing_bytes().as_ref());

        sdp_ledger.try_apply_sdp_declaration(utxos, op, &zk_sig, &ed25519_sig, tx_hash, config)
    }

    fn apply_withdraw_with_dummies(
        sdp_ledger: SdpLedger,
        op: &SDPWithdrawOp,
        note_sk: ZkKey,
        zk_key: ZkKey,
        config: &Config,
    ) -> Result<SdpLedger, Error> {
        let tx_hash = TxHash([1u8; 32]);
        let zk_sig = ZkKey::multi_sign(&[note_sk, zk_key], &tx_hash.to_fr()).unwrap();

        sdp_ledger.apply_withdrawn_msg(op, &zk_sig, tx_hash, config)
    }

    fn dummy_epoch_state(epoch: Epoch) -> EpochState {
        EpochState {
            epoch,
            nonce: ZkHash::ZERO,
            utxos: UtxoTree::default(),
            total_stake: 100,
            lottery_0: Fr::ZERO,
            lottery_1: Fr::ZERO,
        }
    }

    #[test]
    fn test_update_active_provider() {
        let config = setup();
        let service_a = ServiceType::BlendNetwork;
        let utxo = utxo();
        let note_id = utxo.id();
        let signing_key = create_signing_key();
        let zk_key = create_zk_key(0);

        let op = &SDPDeclareOp {
            service_type: service_a,
            locked_note_id: note_id,
            zk_id: zk_key.to_public_key(),
            provider_id: ProviderId(signing_key.public_key()),
            locators: Vec::new(),
        };
        let declaration_id = op.id();

        // Initialize ledger with service config
        let epoch_state = dummy_epoch_state(0.into());
        let sdp_ledger = SdpLedger::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, &epoch_state);

        // Apply a declaration at epoch 0
        let utxo_tree = utxo_tree(vec![utxo]);
        let mut sdp_ledger =
            apply_declare_with_dummies(&utxo_tree, sdp_ledger, op, &zk_key, &config).unwrap();

        // Declaration is in service_state.declarations but not in the active snapshot
        // yet
        let declarations = sdp_ledger.get_declarations(service_a).unwrap();
        assert!(declarations.contains_key(&declaration_id));
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert_eq!(active_snapshot.epoch, 0.into());
        assert!(!active_snapshot.declarations.contains_key(&declaration_id));

        // Apply a header from epoch 1 to trigger epoch transition: 0->1
        (sdp_ledger, _) = sdp_ledger
            .try_apply_header(&config, &dummy_epoch_state(1.into()))
            .unwrap();

        // At epoch 1, the declaration is still not in the active snapshot yet,
        // but it should be in the snapshot for the next epoch.
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert_eq!(active_snapshot.epoch, 1.into());
        assert!(!active_snapshot.declarations.contains_key(&declaration_id));
        let next_snapshot = sdp_ledger.get_next_snapshot(service_a).unwrap();
        assert_eq!(next_snapshot.epoch, 2.into());
        assert!(next_snapshot.declarations.contains_key(&declaration_id));
        assert_eq!(next_snapshot.declarations.size(), 1);

        // Apply a header from epoch 2 to trigger epoch transition: 1->2
        (sdp_ledger, _) = sdp_ledger
            .try_apply_header(&config, &dummy_epoch_state(2.into()))
            .unwrap();

        // At epoch 1, the declaration must be in the active/next snapshots.
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert_eq!(active_snapshot.epoch, 2.into());
        assert!(active_snapshot.declarations.contains_key(&declaration_id));
        assert_eq!(active_snapshot.declarations.size(), 1);
        let next_snapshot = sdp_ledger.get_next_snapshot(service_a).unwrap();
        assert_eq!(next_snapshot.epoch, 3.into());
        assert!(next_snapshot.declarations.contains_key(&declaration_id));
        assert_eq!(next_snapshot.declarations.size(), 1);
    }

    #[test]
    fn test_withdraw_provider() {
        let config = setup_with(ServiceParameters {
            lock_period: 10.into(),
            // inactivity/retention periods should be longer than lock period
            // for this test to avoid the declaration being removed due to
            // inacitivity before we can test the withdraw logic.
            inactivity_period: 20.into(),
            retention_period: 20.into(),
            epoch: 0.into(),
        });

        let service_a = ServiceType::BlendNetwork;
        let (utxo_sk, utxo) = utxo_with_sk();
        let note_id = utxo.id();
        let signing_key = create_signing_key();
        let zk_key = create_zk_key(1);

        let declare_op = &SDPDeclareOp {
            service_type: service_a,
            locked_note_id: note_id,
            zk_id: zk_key.to_public_key(),
            provider_id: ProviderId(signing_key.public_key()),
            locators: Vec::new(),
        };
        let declaration_id = declare_op.id();

        // Initialize ledger with service config and declare
        let epoch_state = dummy_epoch_state(0.into());
        let sdp_ledger = SdpLedger::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, &epoch_state);

        let utxo_tree = utxo_tree(vec![utxo]);
        let sdp_ledger =
            apply_declare_with_dummies(&utxo_tree, sdp_ledger, declare_op, &zk_key, &config)
                .unwrap();

        // Verify declaration is present
        let declarations = sdp_ledger.get_declarations(service_a).unwrap();
        assert!(declarations.contains_key(&declaration_id));

        // Move forward enough epochs to satisfy lock_period
        let mut sdp_ledger = sdp_ledger;
        for epoch in 1..=11 {
            (sdp_ledger, _) = sdp_ledger
                .try_apply_header(&config, &dummy_epoch_state(epoch.into()))
                .unwrap();
        }

        // Withdraw the declaration
        let withdraw_op = &SDPWithdrawOp {
            declaration_id,
            nonce: 1,
            locked_note_id: note_id,
        };
        let sdp_ledger =
            apply_withdraw_with_dummies(sdp_ledger, withdraw_op, utxo_sk, zk_key, &config).unwrap();

        // Verify declaration is removed
        let declarations = sdp_ledger.get_declarations(service_a).unwrap();
        assert!(!declarations.contains_key(&declaration_id));
        assert!(declarations.is_empty());
    }

    #[test]
    fn test_no_promotion() {
        let config = setup();
        let service_a = ServiceType::BlendNetwork;

        // Initialize ledger with service config
        let epoch_state = dummy_epoch_state(0.into());
        let sdp_ledger = SdpLedger::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, &epoch_state);

        // Check active snapshot is still epoch 0 with no declarations
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert_eq!(active_snapshot.epoch, 0.into());
        assert!(active_snapshot.declarations.is_empty());

        // Check next snapshot is still session 1
        let next_snapshot = sdp_ledger.get_next_snapshot(service_a).unwrap();
        assert_eq!(next_snapshot.epoch, 1.into());
        assert!(next_snapshot.declarations.is_empty());
    }

    #[test]
    fn test_declaration_snapshot_timing() {
        let config = setup();
        let service_a = ServiceType::BlendNetwork;
        let signing_key = create_signing_key();
        let zk_key_1 = create_zk_key(1);

        let epoch_state = dummy_epoch_state(0.into());
        let mut sdp_ledger = SdpLedger::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, &epoch_state);

        // Add a declaration at epoch 0
        let utxo_1 = utxo();
        let declare_op_1 = &SDPDeclareOp {
            service_type: service_a,
            locked_note_id: utxo_1.id(),
            zk_id: zk_key_1.to_public_key(),
            provider_id: ProviderId(signing_key.public_key()),
            locators: Vec::new(),
        };
        let declaration_id_1 = declare_op_1.id();

        let utxo_tree_1 = utxo_tree(vec![utxo_1]);
        sdp_ledger =
            apply_declare_with_dummies(&utxo_tree_1, sdp_ledger, declare_op_1, &zk_key_1, &config)
                .unwrap();

        // Save state at epoch 0
        let sdp_ledger_epoch_0 = sdp_ledger.clone();

        // Add another declaration at epoch 1
        (sdp_ledger, _) = sdp_ledger
            .try_apply_header(&config, &dummy_epoch_state(1.into()))
            .unwrap();
        assert_eq!(sdp_ledger.epoch, 1.into());

        let zk_key_2 = create_zk_key(2);
        let utxo_2 = utxo();
        let declare_op_2 = &SDPDeclareOp {
            service_type: service_a,
            locked_note_id: utxo_2.id(),
            zk_id: zk_key_2.to_public_key(),
            provider_id: ProviderId(signing_key.public_key()),
            locators: Vec::new(),
        };
        let declaration_id_2 = declare_op_2.id();

        let utxo_tree_2 = utxo_tree(vec![utxo_1, utxo_2]);
        sdp_ledger =
            apply_declare_with_dummies(&utxo_tree_2, sdp_ledger, declare_op_2, &zk_key_2, &config)
                .unwrap();

        // Move forward: epoch 1->2
        (sdp_ledger, _) = sdp_ledger
            .try_apply_header(&config, &dummy_epoch_state(2.into()))
            .unwrap();

        // Active snapshot (epoch 2) should contain the declaration_1
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert!(active_snapshot.declarations.contains_key(&declaration_id_1));
        assert!(!active_snapshot.declarations.contains_key(&declaration_id_2));
        // Next snapshot (epoch 3) should contain both declarations
        let next_snapshot = sdp_ledger.get_next_snapshot(service_a).unwrap();
        assert!(next_snapshot.declarations.contains_key(&declaration_id_1));
        assert!(next_snapshot.declarations.contains_key(&declaration_id_2));

        // Now test from the epoch 1 state - move forward to epoch 2 without new
        // declaration
        let mut sdp_ledger_from_epoch_0 = sdp_ledger_epoch_0;
        for epoch in 1..=2 {
            (sdp_ledger_from_epoch_0, _) = sdp_ledger_from_epoch_0
                .try_apply_header(&config, &dummy_epoch_state(epoch.into()))
                .unwrap();
        }

        // Active snapshot (epoch 2) should contain only the declaration_1
        let active_snapshot = sdp_ledger_from_epoch_0
            .get_active_snapshot(service_a)
            .unwrap();
        assert!(active_snapshot.declarations.contains_key(&declaration_id_1));
        assert!(!active_snapshot.declarations.contains_key(&declaration_id_2));
        // Next snapshot (epoch 3) should contain only the declaration_1
        let next_snapshot = sdp_ledger_from_epoch_0
            .get_next_snapshot(service_a)
            .unwrap();
        assert!(next_snapshot.declarations.contains_key(&declaration_id_1));
        assert!(!next_snapshot.declarations.contains_key(&declaration_id_2));
    }

    #[test]
    #[ignore = "must be enabled after defining how to handle epoch jumps"]
    fn test_epoch_jump() {
        let config = setup();
        let service_a = ServiceType::BlendNetwork;
        let signing_key = create_signing_key();
        let zk_key = create_zk_key(0);

        let epoch_state = dummy_epoch_state(0.into());
        let mut sdp_ledger = SdpLedger::new(epoch_state.epoch())
            .with_blend_service(&config.service_rewards_params.blend, &epoch_state);

        // Add declaration at epoch 0
        let utxo = utxo();
        let declare_op = &SDPDeclareOp {
            service_type: service_a,
            locked_note_id: utxo.id(),
            zk_id: zk_key.to_public_key(),
            provider_id: ProviderId(signing_key.public_key()),
            locators: Vec::new(),
        };
        let declaration_id = declare_op.id();

        let utxo_tree = utxo_tree(vec![utxo]);
        sdp_ledger =
            apply_declare_with_dummies(&utxo_tree, sdp_ledger, declare_op, &zk_key, &config)
                .unwrap();

        // Jump directly from epoch 0 to 3 (skipping epoch 1 and 2)
        (sdp_ledger, _) = sdp_ledger
            .try_apply_header(&config, &dummy_epoch_state(3.into()))
            .unwrap();
        assert_eq!(sdp_ledger.epoch, 3.into());

        // Declaration snapshots should be taken from the last known state.
        // Active snapshot (epoch 3) should contain the declaration.
        let active_snapshot = sdp_ledger.get_active_snapshot(service_a).unwrap();
        assert_eq!(active_snapshot.epoch, 3.into());
        assert!(active_snapshot.declarations.contains_key(&declaration_id));
        // Next session (epoch 4) should also contain the declaration
        let next_snapshot = sdp_ledger.get_next_snapshot(service_a).unwrap();
        assert_eq!(next_snapshot.epoch, 4.into());
        assert!(next_snapshot.declarations.contains_key(&declaration_id));
    }
}

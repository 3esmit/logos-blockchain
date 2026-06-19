use std::sync::Arc;

use ark_ff::PrimeField as _;
use lb_cryptarchia_engine::{Epoch, Slot};
use lb_groth16::{Fr, fr_from_bytes, fr_to_bytes};
use lb_pol::{LotteryConstants, P};
use lb_poseidon2::{Digest as _, Poseidon2Bn254Hasher};
use lb_utils::math::NonNegativeRatio;
use nom::{IResult, Parser as _, combinator::map};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::{
    events::Events,
    mantle::{
        NoteId, Value,
        ledger::{self, Operation as _},
        nom::{NomDecode, NomEncode},
        ops::channel::{
            ChannelId, ChannelKeyIndex, Ed25519PublicKey, MsgId,
            config::Keys,
            inscribe::{InscriptionExecutionContext, InscriptionOp},
        },
    },
};

/// Domain separator for the channel inscription lottery ticket.
///
/// Sibling of `LEAD_V1` ([`crate::proofs::leader_proof`]); the channel id is
/// hashed into the ticket so a consensus-lottery (`PoL`) win can never be
/// replayed as a channel-lottery win, and vice-versa.
static CHAN_LEAD_V1: LazyLock<Fr> = LazyLock::new(|| {
    fr_from_bytes(b"CHAN_LEAD_V1").expect("BigUint should load from constant string")
});

/// Number of epochs a stake must "age" before it becomes eligible to win.
///
/// Mirrors Proof-of-Leadership's aged-tree lag: a freshly created `note_id`
/// hashes the contents of the tx that minted it, so an attacker who already
/// knew the epoch nonce could mint candidate notes and stake only the winners.
/// Requiring staking to complete `STAKE_MATURITY` epochs before eligibility
/// means note ids are committed before the seed they would be ground against
/// exists. See design doc §3.4.
pub const STAKE_MATURITY: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct SlotTimeframe(u32);

impl From<u32> for SlotTimeframe {
    fn from(slot: u32) -> Self {
        Self(slot)
    }
}

impl From<SlotTimeframe> for u32 {
    fn from(slot: SlotTimeframe) -> Self {
        slot.0
    }
}

impl NomEncode for SlotTimeframe {
    fn encode(&self) -> Vec<u8> {
        self.0.encode()
    }
}

impl NomDecode for SlotTimeframe {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        map(u32::decode, Self).parse(bytes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct SlotTimeout(u32);

impl From<u32> for SlotTimeout {
    fn from(slot: u32) -> Self {
        Self(slot)
    }
}

impl From<SlotTimeout> for u32 {
    fn from(slot: SlotTimeout) -> Self {
        slot.0
    }
}

impl NomEncode for SlotTimeout {
    fn encode(&self) -> Vec<u8> {
        self.0.encode()
    }
}

impl NomDecode for SlotTimeout {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        map(u32::decode, Self).parse(bytes)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("Invalid parent {parent:?} for channel {channel_id:?}, expected {actual:?}")]
    InvalidParent {
        channel_id: ChannelId,
        parent: [u8; 32],
        actual: [u8; 32],
    },
    #[error("Unauthorized signer {signer:?} for channel {channel_id:?}")]
    UnauthorizedSigner {
        channel_id: ChannelId,
        signer: String,
    },
    #[error("Invalid signature")]
    InvalidSignature,
    #[error(
        "Invalid signature index {index:?} for channel {channel_id:?} which has {sequencers:?} sequencers"
    )]
    InvalidSignatureIndex {
        channel_id: ChannelId,
        sequencers: usize,
        index: ChannelKeyIndex,
    },
    #[error("Channel {channel_id:?} not found")]
    ChannelNotFound { channel_id: ChannelId },
    #[error("Insufficient funds")]
    InsufficientFunds,
    #[error("Balance overflow")]
    BalanceOverflow,
    #[error("The withdraw nonce doesn't correspond to the channel state")]
    InvalidWithdrawNonce,
    #[error("The Channel Config isn't well formed")]
    InvalidChannelConfig,
    #[error("Withdraw Nonce overflow")]
    WithdrawNonceOverflow,
    #[error("Inputs error: {0}")]
    Inputs(#[from] ledger::InputsError),
    #[error("Outputs error: {0}")]
    Outputs(#[from] ledger::OutputsError),
    #[error(
        "Invalid number of signatures (treshold:?) for channel {channel_id:?}, expected {actual:?}"
    )]
    ThresholdUnmet {
        channel_id: ChannelId,
        threshold: u16,
        actual: usize,
    },
    // --- Permissionless (stake-lottery) sequencing ---
    #[error("Channel {channel_id:?} is not a lottery channel")]
    NotALotteryChannel { channel_id: ChannelId },
    #[error("Inscription on lottery channel {channel_id:?} is missing its lottery-win proof")]
    MissingLotteryProof { channel_id: ChannelId },
    #[error("Note {note_id:?} is not staked in channel {channel_id:?}")]
    StakeNotFound {
        channel_id: ChannelId,
        note_id: NoteId,
    },
    #[error("Note {note_id:?} is already staked in channel {channel_id:?}")]
    DuplicateStake {
        channel_id: ChannelId,
        note_id: NoteId,
    },
    #[error("Note {note_id:?} stake is not yet effective in channel {channel_id:?}")]
    StakeNotYetEffective {
        channel_id: ChannelId,
        note_id: NoteId,
    },
    #[error("Note {note_id:?} stake is being withdrawn from channel {channel_id:?}")]
    StakeWithdrawn {
        channel_id: ChannelId,
        note_id: NoteId,
    },
    #[error("Note {note_id:?} stake is already unstaking from channel {channel_id:?}")]
    AlreadyUnstaking {
        channel_id: ChannelId,
        note_id: NoteId,
    },
    #[error("Signer {signer:?} is not the posting key bound to note {note_id:?}")]
    WrongPostingKey { signer: String, note_id: NoteId },
    #[error("Note {note_id:?} value {value} is below the channel's minimum stake {min_stake}")]
    StakeBelowMinimum {
        note_id: NoteId,
        value: Value,
        min_stake: Value,
    },
    #[error("Note {note_id:?} does not exist or is already spent")]
    InexistingNote { note_id: NoteId },
    #[error("Note {note_id:?} is already locked")]
    NoteAlreadyLocked { note_id: NoteId },
    #[error("Note {note_id:?} did not win the lottery for channel {channel_id:?} at slot {slot:?}")]
    NotAWinner {
        channel_id: ChannelId,
        note_id: NoteId,
        slot: Slot,
    },
    #[error("Locking error: {0}")]
    Locking(#[from] crate::sdp::locked_notes::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    pub channels: rpds::HashTrieMapSync<ChannelId, ChannelState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelState {
    // Channel Configuration
    pub accredited_keys: Arc<Keys>, // keys.len() <= ChannelKeyIndex::MAX
    pub configuration_threshold: u16, /* indicating how many keys are required to update
                                     * the
                                     * configuration */

    // Message Ordering
    pub tip_message: MsgId,

    // Decentralized Sequencing
    pub tip_slot: Slot,
    pub tip_sequencer: u16, /* indicating the actual sequencer position in the list of
                             * accredited keys */
    pub tip_sequencer_starting_slot: Slot,
    pub posting_timeframe: SlotTimeframe, // number of slots (0 = infinity)
    pub posting_timeout: SlotTimeout,     // number of slots (0 = no timeout)

    // Bridging
    pub balance: Value,
    pub withdrawal_nonce: u32,
    pub withdraw_threshold: ChannelKeyIndex, /* indicating how many keys are required to
                                              * withdraw
                                              * funds from the channel */

    // Permissionless sequencing.
    //
    // `None` => the channel is sequenced by `round_robin` over `accredited_keys`
    // (the default, permissioned mode). `Some(..)` => the channel is sequenced by
    // a stake lottery (see [`ChannelState::lottery_wins`]); the round-robin cursor
    // fields above are then unused, but `accredited_keys`/`configuration_threshold`
    // remain live for config and bridging (design doc §2.3).
    //
    // NOTE: the production design (§3.1) replaces the four round-robin cursor
    // fields with a `Sequencing` enum. This first pass keeps the fields and
    // adds the lottery state additively to avoid rippling an enum refactor
    // through the zone-sdk, tui, and every test; the on-chain mechanism is the
    // same either way.
    #[serde(default)]
    pub lottery: Option<LotteryState>,
}

/// Per-channel state for the permissionless stake lottery (design doc §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LotteryState {
    /// `note_id -> stake entry`. rpds map: cheap per-fork clone, like
    /// `Arc<Keys>`.
    pub stakes: rpds::HashTrieMapSync<NoteId, StakeEntry>,
    /// Channel-local active-slot coefficient: the target posting rate per
    /// staked unit (e.g. `1/2` => one expected post per 2 slots for the whole
    /// channel at full participation). Set via `CHANNEL_CONFIG`.
    pub f_c: NonNegativeRatio,
    /// Liveness: the win threshold doubles every `posting_timeout` slots without
    /// a post (`0` = no widening). The round-robin timeout analogue.
    pub posting_timeout: SlotTimeout,
    /// Anti-bloat floor for a single staked note.
    pub min_stake: Value,
    /// Unbonding delay in epochs before an unstaked note becomes spendable
    /// again.
    pub unbonding_epochs: u32,
}

/// A single staked note bound to an Ed25519 posting key (design doc §3.1).
///
/// Entries are immutable once written; removals are flagged (`unstaked_at`)
/// rather than deleted, so "the registry as of epoch e" is reconstructible by
/// filtering — no snapshot duplication. This mirrors Proof-of-Leadership's dual check:
/// `effective_from <= e` plays aged-membership and `unstaked_at.is_none()`
/// plays still-unspent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakeEntry {
    /// Note value, frozen at stake time (notes are immutable).
    pub value: Value,
    /// The Ed25519 key that signs inscriptions for this note.
    pub posting_key: Ed25519PublicKey,
    /// First epoch in which this stake may win (`stake_epoch + STAKE_MATURITY`).
    pub effective_from: Epoch,
    /// Set by `CHANNEL_UNSTAKE`; ineligible immediately, unlocked after
    /// `unbonding_epochs`.
    pub unstaked_at: Option<Epoch>,
}

impl StakeEntry {
    /// Whether this entry is eligible to win in `epoch`: it has matured and is
    /// not exiting. Used both for the per-note membership check and for summing
    /// the channel's eligible staked total.
    #[must_use]
    pub fn is_eligible(&self, epoch: Epoch) -> bool {
        self.effective_from <= epoch && self.unstaked_at.is_none()
    }
}

pub(crate) const DEFAULT_WITHDRAW_THRESHOLD: ChannelKeyIndex = 1;

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub fn from_genesis(op: &InscriptionOp) -> Result<(Self, Events), Error> {
        let (ctx, events) = op.execute(InscriptionExecutionContext {
            channels: Self::default(),
            block_slot: Slot::default(),
        })?;
        Ok((ctx.channels, events))
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: rpds::HashTrieMapSync::new_sync(),
        }
    }

    #[must_use]
    pub fn channel_state(&self, channel_id: &ChannelId) -> Option<&ChannelState> {
        self.channels.get(channel_id)
    }
}

impl ChannelState {
    // Returns the new sequencer index and its starting slot
    #[must_use]
    pub fn round_robin(&self, block_slot: Slot) -> (u16, Slot) {
        let elapsed_slot_since_last_tip = (block_slot.saturating_sub(self.tip_slot)).into_inner();
        let tip_sequencer_duration =
            (block_slot.saturating_sub(self.tip_sequencer_starting_slot)).into_inner();
        let posting_timeframe = u64::from(self.posting_timeframe.0);
        let posting_timeout = u64::from(self.posting_timeout.0);
        let num_sequencers = self.accredited_keys.len() as u64; // bounded by ChannelKeyIndex::MAX
        let tip_sequencer = u64::from(self.tip_sequencer);
        let is_timed_out = elapsed_slot_since_last_tip >= posting_timeout && posting_timeout != 0;
        let sequencers_timed_out = elapsed_slot_since_last_tip.checked_div(posting_timeout); // None if posting_timeout == 0
        let timeframe_elapsed = tip_sequencer_duration.checked_div(posting_timeframe); // None if timeframe == 0

        // Timeout-based rotation takes priority when timed out.
        // Falls back to timeframe-based rotation, then to the current sequencer.
        let index = sequencers_timed_out
            .filter(|_| is_timed_out)
            .or(timeframe_elapsed)
            .map_or(self.tip_sequencer, |slot| {
                ((tip_sequencer + slot) % num_sequencers) as u16
            });

        // Starting slot mirrors the same priority.
        let starting_slot = sequencers_timed_out
            .filter(|_| is_timed_out)
            .map(|sequencers_timed_out| {
                self.tip_slot
                    .strict_add((sequencers_timed_out * posting_timeout).into())
            })
            .or_else(|| {
                timeframe_elapsed.map(|timeframe_elapsed| {
                    self.tip_sequencer_starting_slot
                        .strict_add((timeframe_elapsed * posting_timeframe).into())
                })
            })
            .unwrap_or(self.tip_sequencer_starting_slot);
        (index, starting_slot)
    }

    /// Does `note_id`, staked in this channel and bound to `signer`, win the
    /// right to extend the tip at `block_slot`?
    ///
    /// This is the permissionless-sequencing sibling of [`Self::round_robin`]:
    /// a pure function of `(channel state, chain-derived inputs)` that every
    /// observer (validators and the off-chain SDK) evaluates identically.
    ///
    /// `beacon = (epoch, epoch_nonce)` is the frozen Cryptarchia epoch
    /// randomness, read from the including block's epoch state. `channel_id` is
    /// passed in because it is the map key, not a field of [`ChannelState`].
    ///
    /// See design doc §3.3. The ticket is public (no secret input), so anyone
    /// can precompute every winning slot for the epoch the moment the nonce
    /// freezes — the transparency/predictability trade of Variant A (§3.7).
    pub fn lottery_wins(
        &self,
        channel_id: &ChannelId,
        note_id: &NoteId,
        signer: &Ed25519PublicKey,
        block_slot: Slot,
        beacon: &(Epoch, Fr),
    ) -> Result<(), Error> {
        let Some(lottery) = &self.lottery else {
            return Err(Error::NotALotteryChannel {
                channel_id: *channel_id,
            });
        };
        let (epoch, epoch_nonce) = beacon;

        // 1. Membership: staked early enough (aged-tree analogue) and not
        //    exiting (latest-tree analogue).
        let entry = lottery
            .stakes
            .get(note_id)
            .ok_or(Error::StakeNotFound {
                channel_id: *channel_id,
                note_id: *note_id,
            })?;
        if entry.effective_from > *epoch {
            return Err(Error::StakeNotYetEffective {
                channel_id: *channel_id,
                note_id: *note_id,
            });
        }
        if entry.unstaked_at.is_some() {
            return Err(Error::StakeWithdrawn {
                channel_id: *channel_id,
                note_id: *note_id,
            });
        }

        // 2. Identity: the posting key bound at stake time signs the
        //    inscription.
        if entry.posting_key != *signer {
            return Err(Error::WrongPostingKey {
                signer: format!("{signer:?}"),
                note_id: *note_id,
            });
        }

        // 3. The ticket — public, same shape as PoL's
        //    `Poseidon2(LEAD_V1, nonce, slot, note_id, sk)` minus the secret
        //    `sk` (a transparent variant cannot include a secret the validator
        //    can't see; see §3.7).
        let ticket = Poseidon2Bn254Hasher::digest(&[
            *CHAN_LEAD_V1,
            *epoch_nonce,
            Fr::from(block_slot.into_inner()),
            Self::channel_id_fr(channel_id),
            note_id.0,
        ]);

        // 4. The threshold — `phi_approx` over the channel's eligible staked
        //    total for this epoch (same math as PoL), widened by elapsed silence
        //    since the last post (the `posting_timeout` analogue).
        let total_staked = lottery.eligible_total(*epoch);
        // `entry` passed the eligibility checks above, so it is counted in
        // `total_staked`. `min_stake >= 1` is enforced at config time, so an
        // eligible note has value >= 1 and the total is normally non-zero — but
        // guard explicitly anyway, because `compute_lottery_values` divides by
        // the total and a zero would panic.
        if total_staked == 0 {
            return Err(Error::NotAWinner {
                channel_id: *channel_id,
                note_id: *note_id,
                slot: block_slot,
            });
        }
        let (t0, t1) = LotteryConstants::new(lottery.f_c).compute_lottery_values(total_staked);
        let base = Self::phi_approx(entry.value, &(t0, t1));

        let elapsed = block_slot.saturating_sub(self.tip_slot).into_inner();
        let doublings = match u64::from(lottery.posting_timeout.0) {
            0 => 0,
            timeout => elapsed / timeout,
        };
        let threshold = Self::widen(base, doublings);

        if Self::fr_to_biguint(ticket) < threshold {
            Ok(())
        } else {
            Err(Error::NotAWinner {
                channel_id: *channel_id,
                note_id: *note_id,
                slot: block_slot,
            })
        }
    }

    /// `phi_approx` — the 2-term Taylor expansion of `1 - (1-f)^(v/S)` used by
    /// Proof-of-Leadership: `value * (t0 + t1 * value)` over the BN254 field.
    /// Sibling of `LeaderPublic::phi_approx`
    /// ([`crate::proofs::leader_proof`]).
    fn phi_approx(value: Value, approx: &(Fr, Fr)) -> Fr {
        let v = Fr::from(value);
        v * (approx.0 + (approx.1 * v))
    }

    /// Threshold widening: `min(base * 2^doublings, p - 1)`, computed as an
    /// integer (NOT in the field — field multiplication wraps mod p, which
    /// would break the monotone "grow towards everyone-wins" guarantee).
    ///
    /// Once `doublings` is large enough that `base << doublings >= p`, the
    /// threshold saturates at `p - 1` ("every staked note wins every slot"), so
    /// liveness needs only one online staker and no takeover message — exactly
    /// like round-robin's timeout. See design doc §3.3.
    fn widen(base: Fr, doublings: u64) -> BigUint {
        let max = &*P - BigUint::from(1u32);
        let base = Self::fr_to_biguint(base);
        if doublings == 0 {
            return base.min(max);
        }
        // p ≈ 2^254, so any shift past ~256 bits already saturates; clamp first
        // to avoid materialising an enormous BigUint.
        let shift = usize::try_from(doublings).unwrap_or(usize::MAX).min(256);
        (base << shift).min(max)
    }

    fn fr_to_biguint(fr: Fr) -> BigUint {
        BigUint::from_bytes_le(&fr_to_bytes(&fr))
    }

    /// Convert a [`ChannelId`] (32 bytes) to a field element for hashing into
    /// the ticket. Reduced mod p (the bytes may exceed the field order), which
    /// is fine: it only needs to be a deterministic per-channel domain value.
    fn channel_id_fr(channel_id: &ChannelId) -> Fr {
        Fr::from_le_bytes_mod_order(channel_id.as_ref())
    }
}

impl LotteryState {
    /// Sum of the value of every entry eligible to win in `epoch`
    /// (`effective_from <= epoch` and not exiting). Saturates on overflow.
    #[must_use]
    pub fn eligible_total(&self, epoch: Epoch) -> Value {
        self.stakes
            .values()
            .filter(|entry| entry.is_eligible(epoch))
            .fold(0, |acc, entry| acc.saturating_add(entry.value))
    }
}

#[cfg(test)]
mod tests {
    use ark_ff::AdditiveGroup as _;
    use lb_key_management_system_keys::keys::{Ed25519Key, ZkKey, ZkPublicKey};
    use lb_utils::blake_rng::RngCore as _;
    use rand::thread_rng;

    use super::*;
    use crate::{
        events::{Event, EventPayload},
        mantle::{
            Note, Utxo,
            ledger::{Outputs, Utxos},
            ops::{
                OpId as _,
                channel::{
                    Ed25519PublicKey as PublicKey,
                    deposit::{DepositExecutionContext, DepositOp, Metadata},
                    withdraw::{ChannelWithdrawOp, WithdrawExecutionContext},
                },
            },
            tx::{GasPrices, MantleTxGasContext},
        },
        sdp::locked_notes::LockedNotes,
    };

    fn test_public_key(seed: u8) -> PublicKey {
        Ed25519Key::from_bytes(&[seed; 32]).public_key()
    }

    fn make_channel(
        tip_slot: u64,
        tip_sequencer: u16,
        tip_sequencer_starting_slot: u64,
        posting_timeframe: u32,
        posting_timeout: u32,
        num_keys: u8,
    ) -> ChannelState {
        ChannelState {
            tip_slot: Slot::new(tip_slot),
            tip_sequencer,
            tip_sequencer_starting_slot: Slot::new(tip_sequencer_starting_slot),
            posting_timeframe: SlotTimeframe(posting_timeframe),
            posting_timeout: SlotTimeout(posting_timeout),
            balance: 0,
            withdrawal_nonce: 0,
            accredited_keys: Keys::try_from((0..num_keys).map(test_public_key).collect::<Vec<_>>())
                .unwrap()
                .into(),
            configuration_threshold: 0,
            tip_message: MsgId::root(),
            withdraw_threshold: 0,
            lottery: None,
        }
    }

    fn utxo(value: Value) -> (ZkKey, Utxo) {
        let mut op_id = [0u8; 32];
        thread_rng().fill_bytes(&mut op_id);
        let zk_sk = ZkKey::from(Fr::ZERO);
        let utxo = Utxo {
            op_id,
            output_index: 0,
            note: Note::new(value, zk_sk.to_public_key()),
        };
        (zk_sk, utxo)
    }

    fn utxo_tree(utxos: Vec<Utxo>) -> Utxos {
        let mut utxo_tree = Utxos::new();
        for utxo in utxos {
            (utxo_tree, _) = utxo_tree.insert(utxo.id(), utxo);
        }
        utxo_tree
    }

    impl Channels {
        #[must_use]
        pub fn with_balance(channel_id: ChannelId, balance: Value) -> Self {
            Self {
                channels: rpds::HashTrieMapSync::new_sync().insert(
                    channel_id,
                    ChannelState {
                        accredited_keys: Keys::from(test_public_key(7)).into(),
                        configuration_threshold: 1,
                        tip_message: MsgId::root(),
                        tip_slot: Slot::default(),
                        tip_sequencer: 0,
                        tip_sequencer_starting_slot: Slot::default(),
                        posting_timeframe: 0u32.into(),
                        balance,
                        withdraw_threshold: 1,
                        withdrawal_nonce: 0,
                        posting_timeout: 0u32.into(),
                        lottery: None,
                    },
                ),
            }
        }
    }

    #[test]
    fn channels_to_gas_context_tracks_withdraw_thresholds() {
        let first_id = ChannelId::from([1u8; 32]);
        let second_id = ChannelId::from([2u8; 32]);
        let missing_id = ChannelId::from([0u8; 32]);

        let channels = Channels {
            channels: rpds::HashTrieMapSync::new_sync()
                .insert(
                    first_id,
                    ChannelState {
                        accredited_keys: Keys::from(test_public_key(11)).into(),
                        configuration_threshold: 1,
                        tip_message: MsgId::root(),
                        tip_slot: Slot::default(),
                        tip_sequencer: 0,
                        tip_sequencer_starting_slot: Slot::default(),
                        posting_timeframe: 0u32.into(),
                        balance: 5,
                        withdraw_threshold: 1,
                        withdrawal_nonce: 0,
                        posting_timeout: 0u32.into(),
                        lottery: None,
                    },
                )
                .insert(
                    second_id,
                    ChannelState {
                        accredited_keys: Keys::from([test_public_key(22), test_public_key(23)])
                            .into(),
                        configuration_threshold: 1,
                        tip_message: MsgId::root(),
                        tip_slot: Slot::default(),
                        tip_sequencer: 0,
                        tip_sequencer_starting_slot: Slot::default(),
                        posting_timeframe: 0.into(),
                        balance: 9,
                        withdraw_threshold: 2,
                        withdrawal_nonce: 0,
                        posting_timeout: 0.into(),
                        lottery: None,
                    },
                ),
        };

        let gas_context = MantleTxGasContext::from_channels(&channels, GasPrices::new(0, 0));

        assert_eq!(gas_context.withdraw_threshold(&first_id), Some(1));
        assert_eq!(gas_context.withdraw_threshold(&second_id), Some(2));
        assert_eq!(gas_context.withdraw_threshold(&missing_id), None);
    }

    #[test]
    fn deposit_increases_channel_balance() {
        let channel_id = ChannelId::from([0u8; 32]);
        let channels = Channels::with_balance(channel_id, 10);

        let (_, utxo) = utxo(6u64);

        let deposit_op = DepositOp {
            channel_id,
            inputs: [utxo.id()].into(),
            metadata: Metadata::empty(),
        };

        let utxo_tree = utxo_tree(vec![utxo]);

        let (updated, events) = deposit_op
            .execute(DepositExecutionContext {
                channels,
                locked_notes: LockedNotes::new(),
                utxos: utxo_tree,
                tx_hash: [0; 32].into(),
            })
            .expect("execution should succeed");

        assert_eq!(
            updated.channels.channel_state(&channel_id).unwrap().balance,
            16
        );

        assert_eq!(events.len(), 1);
        let Event::Tx {
            tx_hash,
            op_id,
            payload,
        } = events.iter().next().cloned().unwrap()
        else {
            panic!("expected Tx event")
        };
        assert_eq!(tx_hash, [0; 32].into());
        assert_eq!(op_id, deposit_op.op_id());
        let EventPayload::Deposit {
            channel_id,
            amount,
            metadata,
        } = payload;
        assert_eq!(channel_id, deposit_op.channel_id);
        assert_eq!(amount, utxo.note.value);
        assert_eq!(metadata, deposit_op.metadata);
    }

    #[test]
    fn withdraw_decreases_channel_balance() {
        let channel_id = ChannelId::from([0u8; 32]);
        let channels = Channels::with_balance(channel_id, 10);

        let (_, utxo) = utxo(6u64);

        let withdraw_op = ChannelWithdrawOp {
            channel_id,
            outputs: Outputs::new([Note {
                value: 6,
                pk: ZkPublicKey::zero(),
            }]),
            withdraw_nonce: 0,
        };

        let utxo_tree = utxo_tree(vec![utxo]);

        let (updated, events) = withdraw_op
            .execute(WithdrawExecutionContext {
                channels,
                utxos: utxo_tree,
            })
            .expect("execution should succeed");

        assert_eq!(
            updated.channels.channel_state(&channel_id).unwrap().balance,
            4
        );
        assert!(events.is_empty());
    }

    #[test]
    fn withdraw_fails_with_insufficient_funds() {
        let channel_id = ChannelId::from([0u8; 32]);
        let channels = Channels::with_balance(channel_id, 3);

        let (_, utxo) = utxo(6u64);

        let withdraw_op = ChannelWithdrawOp {
            channel_id,
            outputs: Outputs::new([Note {
                value: 6,
                pk: ZkPublicKey::zero(),
            }]),
            withdraw_nonce: 0,
        };

        let utxo_tree = utxo_tree(vec![utxo]);

        let result = withdraw_op.execute(WithdrawExecutionContext {
            channels,
            utxos: utxo_tree,
        });

        assert!(matches!(result, Err(Error::InsufficientFunds)));
    }

    #[test]
    fn withdraw_fails_for_missing_channel() {
        let channel_id = ChannelId::from([0u8; 32]);
        let channels = Channels::new();
        let (_, utxo) = utxo(6u64);

        let withdraw_op = ChannelWithdrawOp {
            channel_id,
            outputs: Outputs::new([Note {
                value: 6,
                pk: ZkPublicKey::zero(),
            }]),
            withdraw_nonce: 0,
        };

        let utxo_tree = utxo_tree(vec![utxo]);

        let result = withdraw_op.execute(WithdrawExecutionContext {
            channels,
            utxos: utxo_tree,
        });

        assert!(matches!(result, Err(Error::ChannelNotFound { .. })));
    }

    // 1. Infinite timeframe (timeframe=0): sequencer holds indefinitely unless
    //    timed out
    #[test]
    fn infinite_timeframe_no_timeout_stays_forever() {
        let channel = make_channel(100, 2, 80, 0, 0, 5);
        assert_eq!(channel.round_robin(100.into()), (2, 80.into()));
        assert_eq!(channel.round_robin(999_999.into()), (2, 80.into()));
    }

    #[test]
    fn infinite_timeframe_not_yet_timed_out() {
        let channel = make_channel(100, 1, 90, 0, 50, 4);
        assert_eq!(channel.round_robin(130.into()), (1, 90.into()));
    }

    #[test]
    fn infinite_timeframe_timed_out() {
        let channel = make_channel(100, 1, 90, 0, 50, 4);
        assert_eq!(channel.round_robin(150.into()), (2, 150.into()));
    }

    #[test]
    fn infinite_timeframe_multiple_timeouts() {
        let channel = make_channel(100, 1, 90, 0, 50, 4);
        assert_eq!(channel.round_robin(220.into()), (3, 200.into()));
    }

    // 2. Normal timeframe rotation (no timeout triggered)
    #[test]
    fn timeframe_rotation_same_slot_no_advance() {
        let channel = make_channel(100, 0, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(100.into()), (0, 100.into()));
    }

    #[test]
    fn timeframe_rotation_within_first_frame() {
        let channel = make_channel(100, 0, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(105.into()), (0, 100.into()));
    }

    #[test]
    fn timeframe_rotation_exact_boundary() {
        let channel = make_channel(100, 0, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(110.into()), (1, 110.into()));
    }

    #[test]
    fn timeframe_rotation_multiple_frames() {
        let channel = make_channel(100, 0, 100, 10, 0, 4);
        assert_eq!(channel.round_robin(125.into()), (2, 120.into()));
    }

    #[test]
    fn timeframe_rotation_wraps_around() {
        let channel = make_channel(100, 2, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(110.into()), (0, 110.into()));
    }

    #[test]
    fn timeframe_rotation_full_cycle() {
        // 3 keys, 3 rotations => back to the same sequencer
        let channel = make_channel(100, 1, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(130.into()), (1, 130.into()));
    }

    #[test]
    fn timeframe_rotation_starting_slot_offset() {
        let channel = make_channel(100, 0, 95, 10, 0, 3);
        assert_eq!(channel.round_robin(105.into()), (1, 105.into()));
    }

    // 3. Timed out sequencers
    #[test]
    fn timeout_exact_boundary() {
        let channel = make_channel(100, 0, 100, 10, 20, 4);
        assert_eq!(channel.round_robin(120.into()), (1, 120.into()));
    }

    #[test]
    fn timeout_skips_multiple_unresponsive_sequencers() {
        let channel = make_channel(100, 0, 100, 5, 10, 4);
        assert_eq!(channel.round_robin(135.into()), (3, 130.into()));
    }

    #[test]
    fn timeout_wraps_past_end_of_key_list() {
        let channel = make_channel(100, 2, 100, 5, 10, 3);
        assert_eq!(channel.round_robin(120.into()), (1, 120.into()));
    }

    #[test]
    fn timeout_wraps_full_cycle() {
        let channel = make_channel(100, 0, 100, 5, 10, 3);
        assert_eq!(channel.round_robin(130.into()), (0, 130.into()));
    }

    // 4. No timeout (timeout=0)
    #[test]
    fn no_timeout_rotates_by_timeframe_even_after_long_absence() {
        let channel = make_channel(100, 0, 100, 10, 0, 3);
        assert_eq!(channel.round_robin(1100.into()), (1, 1100.into()));
    }

    // 5. Just below the timeout threshold
    #[test]
    fn just_below_timeout_uses_timeframe_branch() {
        let channel = make_channel(100, 0, 100, 10, 20, 4);
        assert_eq!(channel.round_robin(119.into()), (1, 110.into()));
    }

    // 6. Single sequencer
    #[test]
    fn single_key_always_index_zero() {
        let channel = make_channel(100, 0, 100, 10, 20, 1);
        assert_eq!(channel.round_robin(100.into()).0, 0);
        assert_eq!(channel.round_robin(115.into()).0, 0);
        assert_eq!(channel.round_robin(130.into()).0, 0);
    }

    // 7. Two sequencers
    #[test]
    fn two_sequencers_alternate() {
        let channel = make_channel(100, 0, 100, 5, 0, 2);
        assert_eq!(channel.round_robin(100.into()).0, 0);
        assert_eq!(channel.round_robin(104.into()).0, 0);
        assert_eq!(channel.round_robin(105.into()).0, 1);
        assert_eq!(channel.round_robin(109.into()).0, 1);
        assert_eq!(channel.round_robin(110.into()).0, 0);
    }

    // 8. 50 sequencers
    #[test]
    fn fifty_sequencers_rotate_and_wrap() {
        let channel = make_channel(0, 0, 0, 5, 0, 50);

        // After 5 slots => sequencer 1
        assert_eq!(channel.round_robin(5.into()).0, 1);
        // After 5*49 = 245 slots => sequencer 49 (last)
        assert_eq!(channel.round_robin(245.into()).0, 49);
        // After 5*50 = 250 slots => wrap back to 0
        assert_eq!(channel.round_robin(250.into()).0, 0);
        // After 5*73 = 365 slots => (0+73)%50 = 23
        assert_eq!(channel.round_robin(365.into()).0, 23);
    }

    #[test]
    fn fifty_sequencers_cascading_timeouts() {
        let channel = make_channel(1000, 10, 1000, 5, 3, 50);
        assert_eq!(channel.round_robin(1090.into()), (40, 1090.into()));
    }

    // 9. State transition: after timeout, new sequencer gets a fresh baseline
    #[test]
    fn after_timeout_new_sequencer_gets_fresh_starting_slot() {
        let channel = make_channel(110, 1, 110, 15, 10, 3);
        assert_eq!(channel.round_robin(125.into()), (2, 120.into()));
        assert_eq!(channel.round_robin(135.into()), (0, 130.into()));
    }

    // 10. Zero elapsed (block_slot == tip_slot)
    #[test]
    fn zero_elapsed_no_change() {
        let channel = make_channel(100, 3, 95, 10, 20, 5);
        assert_eq!(channel.round_robin(100.into()), (3, 95.into()));
    }

    // ---- Permissionless (stake-lottery) sequencing ----

    fn default_f_c() -> NonNegativeRatio {
        NonNegativeRatio::new(1, 20.try_into().unwrap())
    }

    fn note_id(seed: u64) -> NoteId {
        NoteId::from(Fr::from(seed))
    }

    fn entry(value: Value, key_seed: u8, effective_from: u32, unstaked_at: Option<u32>) -> StakeEntry {
        StakeEntry {
            value,
            posting_key: test_public_key(key_seed),
            effective_from: Epoch::new(effective_from),
            unstaked_at: unstaked_at.map(Epoch::new),
        }
    }

    fn lottery_channel(
        stakes: Vec<(NoteId, StakeEntry)>,
        f_c: NonNegativeRatio,
        posting_timeout: u32,
        tip_slot: u64,
    ) -> ChannelState {
        let mut map = rpds::HashTrieMapSync::new_sync();
        for (id, e) in stakes {
            map = map.insert(id, e);
        }
        ChannelState {
            accredited_keys: Keys::from(test_public_key(0)).into(),
            configuration_threshold: 1,
            tip_message: MsgId::root(),
            tip_slot: Slot::new(tip_slot),
            tip_sequencer: 0,
            tip_sequencer_starting_slot: Slot::default(),
            posting_timeframe: 0u32.into(),
            posting_timeout: 0u32.into(),
            balance: 0,
            withdrawal_nonce: 0,
            withdraw_threshold: 1,
            lottery: Some(LotteryState {
                stakes: map,
                f_c,
                posting_timeout: posting_timeout.into(),
                min_stake: 1,
                unbonding_epochs: 2,
            }),
        }
    }

    fn cid() -> ChannelId {
        ChannelId::from([9u8; 32])
    }

    // Threshold widening saturates to "everyone wins": with a tiny base rate but
    // a long silence (elapsed >> posting_timeout), the threshold doubles up to
    // p-1 and any ticket wins. This is the passive liveness guarantee.
    #[test]
    fn lottery_widening_eventually_guarantees_a_win() {
        let nid = note_id(7);
        let pk = test_public_key(3);
        let channel = lottery_channel(
            vec![(nid, entry(100, 3, 0, None))],
            NonNegativeRatio::new(1, 1_000_000.try_into().unwrap()),
            1,
            0,
        );
        // tip_slot = 0, posting_timeout = 1 → at slot 10_000 the threshold has
        // widened far past p, so it saturates and the ticket wins.
        assert!(
            channel
                .lottery_wins(&cid(), &nid, &pk, Slot::new(10_000), &(Epoch::new(0), Fr::from(1u64)))
                .is_ok()
        );
    }

    #[test]
    fn lottery_rejects_unknown_stake() {
        let channel = lottery_channel(vec![], default_f_c(), 0, 0);
        let err = channel.lottery_wins(
            &cid(),
            &note_id(7),
            &test_public_key(3),
            Slot::new(1),
            &(Epoch::new(5), Fr::from(1u64)),
        );
        assert!(matches!(err, Err(Error::StakeNotFound { .. })));
    }

    #[test]
    fn lottery_rejects_immature_stake() {
        let nid = note_id(7);
        let channel = lottery_channel(vec![(nid, entry(100, 3, 10, None))], default_f_c(), 0, 0);
        let err = channel.lottery_wins(
            &cid(),
            &nid,
            &test_public_key(3),
            Slot::new(1),
            &(Epoch::new(5), Fr::from(1u64)),
        );
        assert!(matches!(err, Err(Error::StakeNotYetEffective { .. })));
    }

    #[test]
    fn lottery_rejects_withdrawn_stake() {
        let nid = note_id(7);
        let channel =
            lottery_channel(vec![(nid, entry(100, 3, 0, Some(4)))], default_f_c(), 0, 0);
        let err = channel.lottery_wins(
            &cid(),
            &nid,
            &test_public_key(3),
            Slot::new(1),
            &(Epoch::new(5), Fr::from(1u64)),
        );
        assert!(matches!(err, Err(Error::StakeWithdrawn { .. })));
    }

    #[test]
    fn lottery_rejects_wrong_posting_key() {
        let nid = note_id(7);
        let channel = lottery_channel(vec![(nid, entry(100, 3, 0, None))], default_f_c(), 0, 0);
        let err = channel.lottery_wins(
            &cid(),
            &nid,
            &test_public_key(99),
            Slot::new(1),
            &(Epoch::new(5), Fr::from(1u64)),
        );
        assert!(matches!(err, Err(Error::WrongPostingKey { .. })));
    }

    #[test]
    fn lottery_wins_on_round_robin_channel_errors() {
        let channel = make_channel(0, 0, 0, 0, 0, 1);
        let err = channel.lottery_wins(
            &cid(),
            &note_id(1),
            &test_public_key(0),
            Slot::new(1),
            &(Epoch::new(0), Fr::from(0u64)),
        );
        assert!(matches!(err, Err(Error::NotALotteryChannel { .. })));
    }

    #[test]
    fn eligible_total_filters_immature_and_withdrawn() {
        let channel = lottery_channel(
            vec![
                (note_id(1), entry(100, 1, 0, None)),    // eligible at e>=0
                (note_id(2), entry(50, 2, 10, None)),    // matures at e=10
                (note_id(3), entry(25, 3, 0, Some(3))),  // withdrawn
            ],
            default_f_c(),
            0,
            0,
        );
        let lottery = channel.lottery.as_ref().unwrap();
        assert_eq!(lottery.eligible_total(Epoch::new(5)), 100);
        assert_eq!(lottery.eligible_total(Epoch::new(10)), 150);
    }

    // The ticket is channel-scoped: the same note staked under two channel ids
    // gets different tickets, so a consensus/other-channel win never transfers.
    #[test]
    fn lottery_ticket_is_channel_scoped() {
        let a = ChannelState::channel_id_fr(&ChannelId::from([1u8; 32]));
        let b = ChannelState::channel_id_fr(&ChannelId::from([2u8; 32]));
        assert_ne!(a, b);
    }
}

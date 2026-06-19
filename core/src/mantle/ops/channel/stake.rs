use lb_cryptarchia_engine::Epoch;
use lb_key_management_system_keys::keys::{Ed25519Signature, ZkPublicKey, ZkSignature};
use nom::IResult;
use serde::{Deserialize, Serialize};

use super::{ChannelId, Ed25519PublicKey};
use crate::{
    events::Events,
    mantle::{
        NoteId, TxHash,
        channel::{Channels, Error, LotteryState, STAKE_MATURITY, StakeEntry},
        ledger::{Operation, Utxos},
        nom::{NomDecode, NomEncode},
    },
    sdp::locked_notes::LockedNotes,
};

/// `CHANNEL_STAKE` (opcode `0x14`).
///
/// Binds a note to an Ed25519 posting key in a lottery channel's per-channel
/// stake registry. SDP_DECLARE-shaped: the note stays in the UTXO tree but is
/// blocked from spending via [`LockedNotes`], and a registry entry records the
/// operational key. See design doc §3.2.
///
/// The note becomes eligible to win [`STAKE_MATURITY`] epochs later
/// (`effective_from = current_epoch + STAKE_MATURITY`); this is the
/// grinding-resistance lag, not just snapshot hygiene (design doc §3.4).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ChannelStakeOp {
    pub channel_id: ChannelId,
    /// The note being staked (stays in the UTXO tree).
    pub note_id: NoteId,
    /// The posting identity bound to this stake; it signs inscriptions.
    pub posting_key: Ed25519PublicKey,
}

// ChannelStake = ChannelId NoteId PostingKey
impl NomEncode for ChannelStakeOp {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.channel_id.encode();
        bytes.extend(self.note_id.encode());
        bytes.extend(self.posting_key.encode());
        bytes
    }
}

impl NomDecode for ChannelStakeOp {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        let (input, channel_id) = ChannelId::decode(bytes)?;
        let (input, note_id) = NoteId::decode(input)?;
        let (input, posting_key) = Ed25519PublicKey::decode(input)?;
        Ok((
            input,
            Self {
                channel_id,
                note_id,
                posting_key,
            },
        ))
    }
}

pub struct ChannelStakeValidationContext<'a> {
    pub channels: &'a Channels,
    pub utxos: &'a Utxos,
    pub locked_notes: &'a LockedNotes,
    pub tx_hash: &'a TxHash,
    /// Proves ownership of the staked note (signs `tx_hash` with the note key).
    pub stake_zk_sig: &'a ZkSignature,
    /// Proves control of `posting_key` (signs `tx_hash` with the posting key).
    pub stake_ed_sig: &'a Ed25519Signature,
}

pub struct ChannelStakeExecutionContext {
    pub channels: Channels,
    pub utxos: Utxos,
    pub locked_notes: LockedNotes,
    pub epoch: Epoch,
}

impl ChannelStakeOp {
    /// The lottery state of the target channel, or an error if the channel does
    /// not exist or is not in lottery mode.
    fn lottery<'a>(&self, channels: &'a Channels) -> Result<&'a LotteryState, Error> {
        let channel = channels
            .channels
            .get(&self.channel_id)
            .ok_or(Error::ChannelNotFound {
                channel_id: self.channel_id,
            })?;
        channel.lottery.as_ref().ok_or(Error::NotALotteryChannel {
            channel_id: self.channel_id,
        })
    }
}

impl Operation<ChannelStakeValidationContext<'_>> for ChannelStakeOp {
    type ExecutionContext<'a>
        = ChannelStakeExecutionContext
    where
        Self: 'a;
    type Error = Error;

    fn validate(&self, ctx: &ChannelStakeValidationContext<'_>) -> Result<(), Self::Error> {
        let lottery = self.lottery(ctx.channels)?;

        // The note must exist and be unspent.
        let utxo = ctx.utxos.get(&self.note_id).ok_or(Error::InexistingNote {
            note_id: self.note_id,
        })?;
        let note = utxo.note;

        // Value must clear the channel's anti-bloat floor.
        if note.value < lottery.min_stake {
            return Err(Error::StakeBelowMinimum {
                note_id: self.note_id,
                value: note.value,
                min_stake: lottery.min_stake,
            });
        }

        // The note must not be locked elsewhere (another channel or SDP
        // service), and must not already be staked here.
        if ctx.locked_notes.contains(&self.note_id) {
            return Err(Error::NoteAlreadyLocked {
                note_id: self.note_id,
            });
        }
        if lottery.stakes.contains_key(&self.note_id) {
            return Err(Error::DuplicateStake {
                channel_id: self.channel_id,
                note_id: self.note_id,
            });
        }

        // Ownership of the note (zk) and control of the posting key (Ed25519).
        if !ZkPublicKey::verify_multi(&[note.pk], &ctx.tx_hash.to_fr(), ctx.stake_zk_sig) {
            return Err(Error::InvalidSignature);
        }
        self.posting_key
            .verify(ctx.tx_hash.as_signing_bytes().as_ref(), ctx.stake_ed_sig)
            .map_err(|_| Error::InvalidSignature)?;

        Ok(())
    }

    fn execute(
        &self,
        mut ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
        let utxo = ctx.utxos.get(&self.note_id).ok_or(Error::InexistingNote {
            note_id: self.note_id,
        })?;
        let note = utxo.note;

        // Insert the registry entry. NOTE: does NOT touch `tip_message` —
        // staking is not a channel message and must not splice the hash chain.
        let channel = ctx
            .channels
            .channels
            .get_mut(&self.channel_id)
            .ok_or(Error::ChannelNotFound {
                channel_id: self.channel_id,
            })?;
        let lottery = channel.lottery.as_mut().ok_or(Error::NotALotteryChannel {
            channel_id: self.channel_id,
        })?;
        lottery.stakes = lottery.stakes.insert(
            self.note_id,
            StakeEntry {
                value: note.value,
                posting_key: self.posting_key,
                effective_from: ctx.epoch.strict_add(Epoch::new(STAKE_MATURITY)),
                unstaked_at: None,
            },
        );

        // Block the note from being spent while it is staked.
        ctx.locked_notes = ctx
            .locked_notes
            .lock_for_channel(self.channel_id, note, &self.note_id)?;

        Ok((ctx, Events::new()))
    }
}

#[cfg(test)]
mod tests {
    use lb_groth16::Fr;
    use lb_key_management_system_keys::keys::Ed25519Key;

    use super::*;
    use crate::mantle::ops::Op;

    fn sample() -> ChannelStakeOp {
        ChannelStakeOp {
            channel_id: ChannelId::from([3u8; 32]),
            note_id: NoteId::from(Fr::from(42u64)),
            posting_key: Ed25519Key::from_bytes(&[5u8; 32]).public_key(),
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let op = sample();
        let encoded = op.encode();
        let (rest, decoded) = ChannelStakeOp::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }

    #[test]
    fn op_dispatch_round_trip() {
        let op = Op::ChannelStake(sample());
        let encoded = op.encode();
        let (rest, decoded) = Op::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }
}

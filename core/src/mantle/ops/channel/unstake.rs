use lb_cryptarchia_engine::Epoch;
use lb_key_management_system_keys::keys::{ZkPublicKey, ZkSignature};
use nom::IResult;
use serde::{Deserialize, Serialize};

use super::ChannelId;
use crate::{
    events::Events,
    mantle::{
        NoteId, TxHash,
        channel::{Channels, Error, LotteryState},
        ledger::Operation,
        nom::{NomDecode, NomEncode},
    },
    sdp::locked_notes::LockedNotes,
};

/// `CHANNEL_UNSTAKE` (opcode `0x15`).
///
/// Flags a staked note as exiting: the entry becomes ineligible to win
/// immediately, so stake that has left can no longer keep winning.
///
/// The note stays locked through the unbonding delay. Once
/// `current_epoch >= unstaked_at + unbonding_epochs`, the epoch-boundary
/// unbonding sweep prunes the entry and unlocks the note, making it spendable
/// again.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ChannelUnstakeOp {
    pub channel_id: ChannelId,
    pub note_id: NoteId,
}

// ChannelUnstake = ChannelId NoteId
impl NomEncode for ChannelUnstakeOp {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.channel_id.encode();
        bytes.extend(self.note_id.encode());
        bytes
    }
}

impl NomDecode for ChannelUnstakeOp {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        let (input, channel_id) = ChannelId::decode(bytes)?;
        let (input, note_id) = NoteId::decode(input)?;
        Ok((
            input,
            Self {
                channel_id,
                note_id,
            },
        ))
    }
}

pub struct ChannelUnstakeValidationContext<'a> {
    pub channels: &'a Channels,
    pub locked_notes: &'a LockedNotes,
    pub tx_hash: &'a TxHash,
    /// Proves ownership of the staked note.
    pub unstake_zk_sig: &'a ZkSignature,
}

pub struct ChannelUnstakeExecutionContext {
    pub channels: Channels,
    pub epoch: Epoch,
}

impl ChannelUnstakeOp {
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

impl Operation<ChannelUnstakeValidationContext<'_>> for ChannelUnstakeOp {
    type ExecutionContext<'a>
        = ChannelUnstakeExecutionContext
    where
        Self: 'a;
    type Error = Error;

    fn validate(&self, ctx: &ChannelUnstakeValidationContext<'_>) -> Result<(), Self::Error> {
        let lottery = self.lottery(ctx.channels)?;

        let entry = lottery.stakes.get(&self.note_id).ok_or(Error::StakeNotFound {
            channel_id: self.channel_id,
            note_id: self.note_id,
        })?;
        if entry.unstaked_at.is_some() {
            return Err(Error::AlreadyUnstaking {
                channel_id: self.channel_id,
                note_id: self.note_id,
            });
        }

        // Ownership: the staked note is locked, so its key is in `locked_notes`.
        let note = ctx
            .locked_notes
            .get(&self.note_id)
            .ok_or(Error::InexistingNote {
                note_id: self.note_id,
            })?;
        if !ZkPublicKey::verify_multi(&[note.pk], &ctx.tx_hash.to_fr(), ctx.unstake_zk_sig) {
            return Err(Error::InvalidSignature);
        }

        Ok(())
    }

    fn execute(
        &self,
        mut ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
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
        let mut entry = *lottery.stakes.get(&self.note_id).ok_or(Error::StakeNotFound {
            channel_id: self.channel_id,
            note_id: self.note_id,
        })?;
        // Mark the entry as exiting: ineligible to win from now on. The note
        // stays locked until the epoch-boundary unbonding sweep releases it.
        entry.unstaked_at = Some(ctx.epoch);
        lottery.stakes = lottery.stakes.insert(self.note_id, entry);

        Ok((ctx, Events::new()))
    }
}

#[cfg(test)]
mod tests {
    use lb_groth16::Fr;

    use super::*;
    use crate::mantle::ops::Op;

    fn sample() -> ChannelUnstakeOp {
        ChannelUnstakeOp {
            channel_id: ChannelId::from([4u8; 32]),
            note_id: NoteId::from(Fr::from(7u64)),
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let op = sample();
        let encoded = op.encode();
        let (rest, decoded) = ChannelUnstakeOp::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }

    #[test]
    fn op_dispatch_round_trip() {
        let op = Op::ChannelUnstake(sample());
        let encoded = op.encode();
        let (rest, decoded) = Op::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }
}

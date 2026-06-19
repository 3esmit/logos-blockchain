use lb_utils::math::NonNegativeRatio;
use nom::IResult;
use serde::{Deserialize, Serialize};

use super::{ChannelId, MsgId};
use crate::{
    crypto::{Digest as _, Hasher},
    events::Events,
    mantle::{
        TxHash, Value,
        channel::{Channels, Error, LotteryState, SlotTimeout},
        ledger::Operation,
        nom::{NomDecode, NomEncode},
    },
    proofs::channel_multi_sig_proof::ChannelMultiSigProof,
};

/// `CHANNEL_LOTTERY_CONFIG` (opcode `0x16`).
///
/// Flips a channel into permissionless (stake-lottery) sequencing mode, or
/// reconfigures its lottery parameters. Authorized by the same incumbent
/// `configuration_threshold` multisig as [`super::config::ChannelConfigOp`], so
/// the governance story is unchanged (design doc §2.2): a channel is born
/// permissioned and its incumbents opt it into the lottery.
///
/// This first pass only *enables/reconfigures* lottery mode; an existing stake
/// registry is preserved across reconfiguration. Flipping a channel back to
/// round-robin (which would need to unlock staked notes) is out of scope here.
///
/// Unlike `CHANNEL_CONFIG`, this op does NOT splice `tip_message` — it is a
/// sequencing-mode change, not a channel message, so it must not break the
/// hash chain or every indexer cursor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelLotteryConfigOp {
    pub channel: ChannelId,
    /// Channel-local active-slot coefficient (target posting rate).
    pub f_c: NonNegativeRatio,
    /// Liveness widening period (slots; `0` = no widening).
    pub posting_timeout: SlotTimeout,
    /// Anti-bloat floor for a single staked note.
    pub min_stake: Value,
    /// Unbonding delay (epochs) before an unstaked note is spendable again.
    pub unbonding_epochs: u32,
}

impl ChannelLotteryConfigOp {
    #[must_use]
    pub fn id(&self) -> MsgId {
        let mut hasher = Hasher::new();
        hasher.update(self.encode());
        MsgId(hasher.finalize().into())
    }
}

// ChannelLotteryConfig = ChannelId FcNum FcDen PostingTimeout MinStake
// UnbondingEpochs
impl NomEncode for ChannelLotteryConfigOp {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.channel.encode();
        bytes.extend(self.f_c.numerator.encode());
        bytes.extend(self.f_c.denominator.get().encode());
        bytes.extend(self.posting_timeout.encode());
        bytes.extend(crate::mantle::encoding::encode_uint64(self.min_stake));
        bytes.extend(self.unbonding_epochs.encode());
        bytes
    }
}

impl NomDecode for ChannelLotteryConfigOp {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        let (bytes, channel) = ChannelId::decode(bytes)?;
        let (bytes, f_num) = u32::decode(bytes)?;
        let (bytes, f_den) = u32::decode(bytes)?;
        let (bytes, posting_timeout) = SlotTimeout::decode(bytes)?;
        let (bytes, min_stake) = crate::mantle::encoding::decode_uint64(bytes)?;
        let (bytes, unbonding_epochs) = u32::decode(bytes)?;
        let denominator = f_den
            .try_into()
            .map_err(|_| nom::Err::Error(nom::error::Error::new(bytes, nom::error::ErrorKind::Fail)))?;
        Ok((
            bytes,
            Self {
                channel,
                f_c: NonNegativeRatio::new(f_num, denominator),
                posting_timeout,
                min_stake,
                unbonding_epochs,
            },
        ))
    }
}

pub struct ChannelLotteryConfigValidationContext<'a> {
    pub channels: &'a Channels,
    pub tx_hash: &'a TxHash,
    pub config_sigs: &'a ChannelMultiSigProof,
}

pub struct ChannelLotteryConfigExecutionContext {
    pub channels: Channels,
}

impl Operation<ChannelLotteryConfigValidationContext<'_>> for ChannelLotteryConfigOp {
    type ExecutionContext<'a>
        = ChannelLotteryConfigExecutionContext
    where
        Self: 'a;
    type Error = Error;

    fn validate(
        &self,
        ctx: &ChannelLotteryConfigValidationContext<'_>,
    ) -> Result<(), Self::Error> {
        // `f_c` must be a proper fraction in (0, 1): the lottery constants are
        // derived from `ln(1 - f_c)`, so `f_c >= 1` yields `ln(<= 0)` (=-inf /
        // NaN) and panics `LotteryConstants::new` during block validation. A
        // zero `min_stake` would permit value-0 stakes and a permanently
        // unwinnable channel.
        if self.f_c.numerator == 0
            || self.f_c.numerator >= self.f_c.denominator.get()
            || self.min_stake == 0
        {
            return Err(Error::InvalidChannelConfig);
        }

        // Authorized by the channel's configuration multisig — identical to
        // ChannelConfigOp. The channel must already exist (it was created
        // permissioned).
        let channel = ctx
            .channels
            .channels
            .get(&self.channel)
            .ok_or(Error::ChannelNotFound {
                channel_id: self.channel,
            })?;

        let signatures = ctx.config_sigs.signatures();
        if signatures.len() != channel.configuration_threshold as usize {
            return Err(Error::ThresholdUnmet {
                channel_id: self.channel,
                threshold: channel.configuration_threshold,
                actual: signatures.len(),
            });
        }
        for sig in signatures {
            if channel
                .accredited_keys
                .get(sig.channel_key_index as usize)
                .ok_or_else(|| Error::InvalidSignatureIndex {
                    channel_id: self.channel,
                    sequencers: channel.accredited_keys.len(),
                    index: sig.channel_key_index,
                })?
                .verify(ctx.tx_hash.as_signing_bytes().as_ref(), &sig.signature)
                .is_err()
            {
                return Err(Error::InvalidSignature);
            }
        }

        Ok(())
    }

    fn execute(
        &self,
        mut ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
        let channel =
            ctx.channels
                .channels
                .get_mut(&self.channel)
                .ok_or(Error::ChannelNotFound {
                    channel_id: self.channel,
                })?;

        // Preserve an existing registry across reconfiguration.
        let stakes = channel
            .lottery
            .as_ref()
            .map_or_else(rpds::HashTrieMapSync::new_sync, |l| l.stakes.clone());

        channel.lottery = Some(LotteryState {
            stakes,
            f_c: self.f_c,
            posting_timeout: self.posting_timeout.clone(),
            min_stake: self.min_stake,
            unbonding_epochs: self.unbonding_epochs,
        });

        Ok((ctx, Events::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mantle::ops::Op;

    fn sample() -> ChannelLotteryConfigOp {
        ChannelLotteryConfigOp {
            channel: ChannelId::from([6u8; 32]),
            f_c: NonNegativeRatio::new(3, 20.try_into().unwrap()),
            posting_timeout: 50u32.into(),
            min_stake: 1_000,
            unbonding_epochs: 2,
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let op = sample();
        let encoded = op.encode();
        let (rest, decoded) = ChannelLotteryConfigOp::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }

    #[test]
    fn op_dispatch_round_trip() {
        let op = Op::ChannelLotteryConfig(sample());
        let encoded = op.encode();
        let (rest, decoded) = Op::decode(&encoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(op, decoded);
    }
}

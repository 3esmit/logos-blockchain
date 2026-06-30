use lb_key_management_system_keys::keys::{ZkPublicKey, ZkSignature};
use nom::IResult;
use serde::{Deserialize, Serialize};

use crate::{
    events::Events,
    mantle::{
        TxHash,
        channel::{Channels, Error},
        encoding::{NomInputs, NomOutputs},
        ledger::{Inputs, Operation, Outputs, Utxos},
        nom::{NomDecode, NomEncode},
        ops::{OpId, channel::ChannelId},
    },
    sdp::locked_notes::LockedNotes,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelStakeTransferOp {
    pub channel_id: ChannelId,
    pub inputs: Inputs,
    pub outputs: Outputs,
}

impl OpId for ChannelStakeTransferOp {
    fn op_bytes(&self) -> Vec<u8> {
        self.encode()
    }
}

impl NomEncode for ChannelStakeTransferOp {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(self.channel_id.encode());
        bytes.extend(NomInputs::from(self.inputs.as_ref()).encode());
        bytes.extend(NomOutputs::from(self.outputs.as_ref()).encode());
        bytes
    }
}

impl NomDecode for ChannelStakeTransferOp {
    type Output = Self;

    fn decode(bytes: &[u8]) -> IResult<&[u8], Self::Output> {
        let (bytes, channel_id) = ChannelId::decode(bytes)?;
        let (bytes, inputs) = NomInputs::decode(bytes)?;
        let (bytes, outputs) = NomOutputs::decode(bytes)?;
        Ok((
            bytes,
            Self {
                channel_id,
                inputs: Inputs::new(inputs),
                outputs: Outputs::new(outputs),
            },
        ))
    }
}

pub struct StakeTransferValidationContext<'a> {
    pub channels: &'a Channels,
    pub locked_notes: &'a LockedNotes,
    pub utxos: &'a Utxos,
    pub tx_hash: &'a TxHash,
    pub stake_transfer_sig: &'a ZkSignature,
}

pub struct StakeTransferExecutionContext {
    pub channels: Channels,
    pub utxos: Utxos,
}

impl Operation<StakeTransferValidationContext<'_>> for ChannelStakeTransferOp {
    type ExecutionContext<'a>
        = StakeTransferExecutionContext
    where
        Self: 'a;
    type Error = Error;

    fn validate(&self, ctx: &StakeTransferValidationContext<'_>) -> Result<(), Self::Error> {
        // Check that the inputs exist and belong to the channel
        // It indirectly validates that the channel exist.
        self.inputs.validate(
            ctx.locked_notes,
            ctx.utxos,
            vec![Some(self.channel_id); self.inputs.len()],
        )?;

        // Check the Signature
        let pks = self.inputs.get_pk(ctx.utxos)?;
        if !ZkPublicKey::verify_multi(&pks, &ctx.tx_hash.to_fr(), ctx.stake_transfer_sig) {
            return Err(Error::InvalidSignature);
        }

        // Check that the outputs are valid
        self.outputs.validate()?;

        // Check the operation is balanced
        let input_amount = self.inputs.amount(ctx.utxos)?;
        let output_amount = self.outputs.amount()?;
        if input_amount != output_amount {
            return Err(Error::UnbalancedOperation);
        }

        Ok(())
    }

    fn execute(
        &self,
        mut ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
        // Remove inputs from the ledger
        ctx.utxos = self.inputs.execute(ctx.utxos)?;

        // Add the ouputs to the ledger
        ctx.utxos = self.outputs.execute(
            ctx.utxos,
            self,
            vec![Some(self.channel_id); self.outputs.len()],
        );

        Ok((ctx, Events::new()))
    }
}

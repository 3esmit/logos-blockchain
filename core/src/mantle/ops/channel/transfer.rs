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
    proofs::channel_multi_sig_proof::ChannelMultiSigProof,
    sdp::service_notes::ServiceNotes,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelTransferOp {
    pub channel_id: ChannelId,
    pub inputs: Inputs,
    pub outputs: Outputs,
}

impl OpId for ChannelTransferOp {
    fn op_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    fn outputs_channel_id(&self) -> Option<ChannelId> {
        Some(self.channel_id)
    }
}

impl NomEncode for ChannelTransferOp {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(self.channel_id.encode());
        bytes.extend(NomInputs::from(self.inputs.as_ref()).encode());
        bytes.extend(NomOutputs::from(self.outputs.as_ref()).encode());
        bytes
    }
}

impl NomDecode for ChannelTransferOp {
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

pub struct ChannelTransferValidationContext<'a> {
    pub channels: &'a Channels,
    pub service_notes: &'a ServiceNotes,
    pub utxos: &'a Utxos,
    pub tx_hash: &'a TxHash,
    pub transfer_sigs: &'a ChannelMultiSigProof,
}

pub struct ChannelTransferExecutionContext {
    pub channels: Channels,
    pub utxos: Utxos,
}

impl Operation<ChannelTransferValidationContext<'_>> for ChannelTransferOp {
    type ExecutionContext<'a>
        = ChannelTransferExecutionContext
    where
        Self: 'a;
    type Error = Error;

    fn validate(&self, ctx: &ChannelTransferValidationContext<'_>) -> Result<(), Self::Error> {
        // Check that the outputs are valid
        self.outputs.validate()?;

        // Check that the channel exist
        if !ctx.channels.channels.contains_key(&self.channel_id) {
            return Err(Error::ChannelNotFound {
                channel_id: self.channel_id,
            });
        }

        // Get the Channel
        let channel = ctx
            .channels
            .channels
            .get(&self.channel_id)
            .cloned()
            .expect("we checked that the channel exist above");

        // Check that the inputs exist and belong to the channel
        self.inputs
            .validate(ctx.service_notes, ctx.utxos, Some(self.channel_id))?;

        // Check the operation is balanced
        let input_amount = self.inputs.amount(ctx.utxos)?;
        let output_amount = self.outputs.amount()?;
        if input_amount != output_amount {
            return Err(Error::UnbalancedOperation);
        }

        // Check that the indexes are unique and there is the same number of proof and
        // index. This is enforced by the proof structure that enforces it.

        // Check there is enough signatures
        let signatures = ctx.transfer_sigs.signatures();
        if signatures.len() != channel.transfer_threshold as usize {
            return Err(Error::ThresholdUnmet {
                channel_id: self.channel_id,
                threshold: channel.transfer_threshold,
                actual: ctx.transfer_sigs.signatures().len(),
            });
        }

        // Check the signatures
        for sig in signatures {
            if channel.accredited_keys[sig.channel_key_index as usize]
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
        // Remove inputs from the ledger
        ctx.utxos = self.inputs.execute(ctx.utxos)?;

        // Add the ouputs to the ledger
        ctx.utxos = self.outputs.execute(ctx.utxos, self);

        Ok((ctx, Events::new()))
    }
}

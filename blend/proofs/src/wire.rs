use lb_wire::{DecodeError, WireDecode, WireEncode, wire_fixtures};

use crate::{
    quota::{PROOF_OF_QUOTA_SIZE, ProofOfQuota, VerifiedProofOfQuota},
    selection::{PROOF_OF_SELECTION_SIZE, ProofOfSelection, VerifiedProofOfSelection},
};

impl WireEncode for ProofOfQuota {
    fn encoded_length(&self) -> usize {
        PROOF_OF_QUOTA_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&<[u8; _]>::from(self));
    }
}

impl WireDecode for ProofOfQuota {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, value) = <[u8; _]>::decode(input, &())?;
        let proof = Self::try_from(value)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a valid proof of quota"))?;
        Ok((rest, proof))
    }
}

wire_fixtures!(
    ProofOfQuota,
    VerifiedProofOfQuota::from_bytes_unchecked([1u8; _]).into() => "01010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101"
);

impl WireEncode for ProofOfSelection {
    fn encoded_length(&self) -> usize {
        PROOF_OF_SELECTION_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&<[u8; _]>::from(self));
    }
}

impl WireDecode for ProofOfSelection {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, value) = <[u8; _]>::decode(input, &())?;
        let proof = Self::try_from(value)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a valid proof of selection"))?;
        Ok((rest, proof))
    }
}

wire_fixtures!(
    ProofOfSelection,
    VerifiedProofOfSelection::from_bytes_unchecked([1u8; _]).into() => "0101010101010101010101010101010101010101010101010101010101010101"
);

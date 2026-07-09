//! `lb-wire` codecs for the proof types this crate owns.
//!
//! These live here (not in a consumer crate) because the unified `WireEncode`/
//! `WireDecode` traits are foreign to every consumer: the orphan rule requires
//! the impl to sit where the *type* is local. Wire format is unchanged from the
//! former Mantle (`WireCodec`) and Blend codecs — the golden fixtures below pin
//! it (they double as the shared fixture for what were two separate impls).

use lb_wire::{DecodeError, WireDecode, WireEncode, wire_fixtures};

use crate::{
    quota::{PROOF_OF_QUOTA_SIZE, ProofOfQuota},
    selection::{PROOF_OF_SELECTION_SIZE, ProofOfSelection},
};

// Proof of quota: a fixed-size byte array, no length prefix.
impl WireEncode for ProofOfQuota {
    fn encoded_length(&self) -> usize {
        PROOF_OF_QUOTA_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&<[u8; PROOF_OF_QUOTA_SIZE]>::from(self));
    }
}

impl WireDecode for ProofOfQuota {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (rest, value) = <[u8; PROOF_OF_QUOTA_SIZE]>::decode(input, ())?;
        let proof = Self::try_from(value)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a valid proof of quota"))?;
        Ok((rest, proof))
    }
}

wire_fixtures!(
    ProofOfQuota,
    crate::quota::VerifiedProofOfQuota::from_bytes_unchecked([1u8; _]).into() => "01010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101"
);

// Proof of selection: a fixed-size byte array, no length prefix.
impl WireEncode for ProofOfSelection {
    fn encoded_length(&self) -> usize {
        PROOF_OF_SELECTION_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&<[u8; PROOF_OF_SELECTION_SIZE]>::from(self));
    }
}

impl WireDecode for ProofOfSelection {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (rest, value) = <[u8; PROOF_OF_SELECTION_SIZE]>::decode(input, ())?;
        let proof = Self::try_from(value)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a valid proof of selection"))?;
        Ok((rest, proof))
    }
}

wire_fixtures!(
    ProofOfSelection,
    crate::selection::VerifiedProofOfSelection::from_bytes_unchecked([1u8; _]).into() => "0101010101010101010101010101010101010101010101010101010101010101"
);

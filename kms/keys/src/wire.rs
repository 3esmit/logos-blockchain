use lb_groth16::{COMPRESSED_PROOF_SIZE, Fr};
use lb_wire::{DecodeError, WireDecode, WireEncode};
use lb_zksign::ZkSignProof;

use crate::keys::{
    ED25519_PUBLIC_KEY_SIZE, ED25519_SIGNATURE_SIZE, Ed25519PublicKey, Ed25519Signature,
    ZkPublicKey, ZkSignature,
};

impl WireEncode for Ed25519PublicKey {
    fn encoded_length(&self) -> usize {
        ED25519_PUBLIC_KEY_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_bytes());
    }
}

impl WireDecode for Ed25519PublicKey {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, inner) = <[u8; ED25519_PUBLIC_KEY_SIZE]>::decode(input, &())?;
        let key = Self::from_bytes(&inner)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a valid Ed25519 public key"))?;
        Ok((rest, key))
    }
}

impl WireEncode for Ed25519Signature {
    fn encoded_length(&self) -> usize {
        ED25519_SIGNATURE_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_bytes());
    }
}

impl WireDecode for Ed25519Signature {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, inner) = <[u8; ED25519_SIGNATURE_SIZE]>::decode(input, &())?;
        Ok((rest, Self::from_bytes(&inner)))
    }
}

impl WireEncode for ZkPublicKey {
    fn encoded_length(&self) -> usize {
        self.as_fr().encoded_length()
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        self.as_fr().encode_into(out);
    }
}

impl WireDecode for ZkPublicKey {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, inner) = Fr::decode(input, &())?;
        Ok((rest, Self::new(inner)))
    }
}

impl WireEncode for ZkSignature {
    fn encoded_length(&self) -> usize {
        // The compressed Groth16 proof is a fixed-size blob; return the constant
        // rather than serializing the proof just to measure it (the trait
        // contract requires `encoded_length` to neither allocate nor encode).
        COMPRESSED_PROOF_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.as_proof().to_bytes());
    }
}

impl WireDecode for ZkSignature {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, inner) = <[u8; _]>::decode(input, &())?;
        Ok((rest, Self::new(ZkSignProof::from_bytes(&inner))))
    }
}

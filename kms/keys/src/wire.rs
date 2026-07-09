use lb_groth16::Fr;
use lb_wire::{DecodeError, WireDecode, WireEncode, wire_fixtures};
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

wire_fixtures!(
    Ed25519PublicKey,
    Self::from_bytes(&[1u8; _]).unwrap() => "0101010101010101010101010101010101010101010101010101010101010101"
);

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

wire_fixtures!(Ed25519Signature, Self::from_bytes(&[1u8; _]) => "01010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101");

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

wire_fixtures!(
    ZkPublicKey,
    Fr::from(1u64).into() => "0100000000000000000000000000000000000000000000000000000000000000"
);

impl WireEncode for ZkSignature {
    fn encoded_length(&self) -> usize {
        self.as_proof().to_bytes().len()
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

wire_fixtures!(
    ZkSignature,
    Self::new(ZkSignProof::from_bytes(&[1u8; _])) => "0101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101"
);

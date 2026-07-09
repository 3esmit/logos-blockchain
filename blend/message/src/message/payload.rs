use lb_wire::{DecodeError, WireDecode, WireEncode, take, wire_fixtures};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::Error;

pub const MAX_PAYLOAD_BODY_SIZE: usize = 34 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadType {
    Cover = 0x00,
    Data = 0x01,
}

impl TryFrom<u8> for PayloadType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Cover),
            0x01 => Ok(Self::Data),
            _ => Err(()),
        }
    }
}

impl WireEncode for PayloadType {
    fn encoded_length(&self) -> usize {
        size_of::<u8>()
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        (*self as u8).encode_into(out);
    }
}

impl WireDecode for PayloadType {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (remaining, discriminant) = u8::decode(input, &())?;
        let payload_type = Self::try_from(discriminant)
            .map_err(|()| DecodeError::unknown_discriminant::<Self>(u64::from(discriminant)))?;
        Ok((remaining, payload_type))
    }
}

wire_fixtures!(PayloadType, PayloadType::Cover => "00", PayloadType::Data => "01");

/// The decapsulated payload body, padded to a fixed size.
///
/// `actual_len` is the length of the real (unpadded) content and is the single
/// source of truth for it — the payload no longer stores it a second time.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaddedPayloadBody {
    /// The real content length; `padded[..actual_len]` is the body.
    actual_len: u16,
    /// A body padded to [`MAX_PAYLOAD_BODY_SIZE`]. `Box` avoids a large stack
    /// allocation.
    #[serde_as(as = "serde_with::Bytes")]
    padded: Box<[u8; MAX_PAYLOAD_BODY_SIZE]>,
}

impl TryFrom<Vec<u8>> for PaddedPayloadBody {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(value.as_slice())
    }
}

impl TryFrom<&[u8]> for PaddedPayloadBody {
    type Error = Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() > MAX_PAYLOAD_BODY_SIZE {
            return Err(Error::PayloadTooLarge);
        }

        let actual_len: u16 = value
            .len()
            .try_into()
            .map_err(|_| Error::InvalidPayloadLength)?;

        let mut padded: Box<[u8; MAX_PAYLOAD_BODY_SIZE]> = vec![0; MAX_PAYLOAD_BODY_SIZE]
            .into_boxed_slice()
            .try_into()
            .expect("body must be created with the correct size");
        let (to_be_set, to_be_random) = padded.split_at_mut(value.len());
        to_be_set.copy_from_slice(value);
        rand::thread_rng().fill(to_be_random);

        Ok(Self { actual_len, padded })
    }
}

impl WireEncode for PaddedPayloadBody {
    fn encoded_length(&self) -> usize {
        self.actual_len
            .encoded_length()
            .checked_add(MAX_PAYLOAD_BODY_SIZE)
            .unwrap()
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        self.actual_len.encode_into(out);
        out.extend_from_slice(&self.padded[..]);
    }
}

impl WireDecode for PaddedPayloadBody {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (input, actual_len) = u16::decode(input, &())?;
        let (body_bytes, remaining) = take::<Self>(input, MAX_PAYLOAD_BODY_SIZE)?;
        let padded: Box<[u8; MAX_PAYLOAD_BODY_SIZE]> = body_bytes
            .to_vec()
            .into_boxed_slice()
            .try_into()
            .expect("Take guarantees the length");
        Ok((remaining, Self { actual_len, padded }))
    }
}

// Well-known bytes: a `u16` length of 3, the body `[1, 2, 3]`, then zero
// padding to `MAX_PAYLOAD_BODY_SIZE`. Externalised as hex because it is ~34
// KiB.
wire_fixtures!(
    PaddedPayloadBody,
    PaddedPayloadBody::try_from([1u8, 2, 3].as_slice()).unwrap()
        => include_str!("../fixtures/padded_payload_body.hex")
);

/// The exact number of bytes a [`Payload`] encodes to: a fixed enum
/// discriminant, the `u16` body length, and the body padded to
/// [`MAX_PAYLOAD_BODY_SIZE`]. Compile-time constant, so the encapsulated
/// (ciphered) form can be stored as a `Box<[u8; PAYLOAD_ENCODED_SIZE]>`.
pub const PAYLOAD_ENCODED_SIZE: usize =
    size_of::<PayloadType>() + size_of::<u16>() + MAX_PAYLOAD_BODY_SIZE;

/// A payload that is fully decapsulated.
/// This must be encapsulated when being sent to the blend network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    payload_type: PayloadType,
    body: PaddedPayloadBody,
}

impl Payload {
    pub const fn new(payload_type: PayloadType, body: PaddedPayloadBody) -> Self {
        Self { payload_type, body }
    }

    pub const fn payload_type(&self) -> PayloadType {
        self.payload_type
    }

    /// Returns the payload body unpadded.
    /// Returns an error if the recorded length exceeds the padded buffer.
    pub fn body(&self) -> Result<&[u8], Error> {
        let len = self.body.actual_len as usize;
        if self.body.padded.len() < len {
            return Err(Error::InvalidPayloadLength);
        }
        Ok(&self.body.padded[..len])
    }

    pub fn try_into_components(self) -> Result<(PayloadType, Vec<u8>), Error> {
        Ok((self.payload_type(), self.body()?.to_vec()))
    }
}

impl WireEncode for Payload {
    fn encoded_length(&self) -> usize {
        PAYLOAD_ENCODED_SIZE
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        self.payload_type.encode_into(out);
        self.body.encode_into(out);
    }
}

impl WireDecode for Payload {
    type Context = ();

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (input, payload_type) = PayloadType::decode(input, &())?;
        let (input, body) = PaddedPayloadBody::decode(input, &())?;
        Ok((input, Self { payload_type, body }))
    }
}

// Well-known bytes: the `Data` discriminant (`0x01`), a `u16` length of 3, the
// body `[4, 5, 6]`, then zero padding. Externalised as hex because it is ~34
// KiB.
wire_fixtures!(
    Payload,
    Payload::new(
        PayloadType::Data,
        PaddedPayloadBody::try_from([4u8, 5, 6].as_slice()).unwrap(),
    ) => include_str!("../fixtures/payload.hex")
);

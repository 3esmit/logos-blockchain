use lb_groth16::{Fr, fr_from_bytes, fr_to_bytes};

use crate::{DecodeError, WireDecode, WireEncode, wire_fixtures};

/// Split `n` bytes off the front of `input`, or fail with an
/// [`UnexpectedEnd`](DecodeError::UnexpectedEnd) that names `T`.
///
/// Returns `(head, rest)`. This is the single length check for every fixed-size
/// primitive decode — replacing the panicking `split_at`/index the two legacy
/// codecs used.
pub(crate) fn split_prefix<T: ?Sized>(
    input: &[u8],
    n: usize,
) -> Result<(&[u8], &[u8]), DecodeError> {
    input
        .split_at_checked(n)
        .ok_or_else(|| DecodeError::end_of_input::<T>(n.saturating_sub(input.len())))
}

/// `WireEncode`/`WireDecode` for a little-endian fixed-width integer.
macro_rules! impl_le_integer {
    ($ty:ty) => {
        impl WireEncode for $ty {
            fn encoded_length(&self) -> usize {
                ::core::mem::size_of::<$ty>()
            }

            fn encode_into(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }

        impl WireDecode for $ty {
            type Context = ();

            fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
                let (head, rest) = split_prefix::<Self>(input, ::core::mem::size_of::<$ty>())?;
                let value = <$ty>::from_le_bytes(head.try_into().expect("split_prefix took the right length"));
                Ok((rest, value))
            }
        }
    };
}

impl_le_integer!(u8);
impl_le_integer!(u16);
impl_le_integer!(u32);
impl_le_integer!(u64);

wire_fixtures!(u8, 0x07u8 => "07", 0u8 => "00");
wire_fixtures!(u16, 1u16 => "0100", 0x0201u16 => "0102");
wire_fixtures!(u32, 1u32 => "01000000", 0x0403_0201u32 => "01020304");
wire_fixtures!(u64, 1u64 => "0100000000000000", 0x0807_0605_0403_0201u64 => "0102030405060708");

// A BLS scalar, encoded as its 32-byte little-endian representation.
impl WireEncode for Fr {
    fn encoded_length(&self) -> usize {
        32
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&fr_to_bytes(self));
    }
}

impl WireDecode for Fr {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (head, rest) = split_prefix::<Self>(input, 32)?;
        let bytes: [u8; 32] = head.try_into().expect("split_prefix took the right length");
        let value = fr_from_bytes(&bytes)
            .map_err(|_| DecodeError::invalid_value::<Self>("not a canonical field element"))?;
        Ok((rest, value))
    }
}

wire_fixtures!(
    Fr,
    Self::from(1u64) => "0100000000000000000000000000000000000000000000000000000000000000"
);

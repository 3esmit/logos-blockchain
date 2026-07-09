//! `lb-wire` codec for [`Epoch`], which this crate owns.
//!
//! Lives here (not in a consumer crate) because the unified `WireEncode`/
//! `WireDecode` traits are foreign to every consumer: the orphan rule requires
//! the impl to sit where the *type* is local. Wire format is unchanged from the
//! former Mantle (`WireCodec`) codec — the golden fixture below pins it.

use lb_wire::{DecodeError, WireDecode, WireEncode, wire_fixtures};

use crate::Epoch;

// Epoch is a `u32` newtype; encoded as its little-endian `u32`.
impl WireEncode for Epoch {
    fn encoded_length(&self) -> usize {
        self.as_ref().encoded_length()
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        self.as_ref().encode_into(out);
    }
}

impl WireDecode for Epoch {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (rest, inner) = u32::decode(input, ())?;
        Ok((rest, Self::new(inner)))
    }
}

wire_fixtures!(Epoch, Self::new(1) => "01000000");

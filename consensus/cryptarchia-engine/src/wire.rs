use lb_wire::{DecodeError, WireDecode, WireEncode};

use crate::Epoch;

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

    fn decode<'input>(
        input: &'input [u8],
        (): &Self::Context,
    ) -> Result<(&'input [u8], Self), DecodeError> {
        let (rest, inner) = u32::decode(input, &())?;
        Ok((rest, Self::new(inner)))
    }
}

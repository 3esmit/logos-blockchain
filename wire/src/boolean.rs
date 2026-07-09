use crate::{DecodeError, WireDecode, WireEncode, wire_fixtures};

// A single byte: `0` for `false`, `1` for `true`; any other value is rejected.
impl WireEncode for bool {
    fn encoded_length(&self) -> usize {
        1
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl WireDecode for bool {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (rest, byte) = u8::decode(input, ())?;
        match byte {
            0 => Ok((rest, false)),
            1 => Ok((rest, true)),
            _ => Err(DecodeError::invalid_value::<Self>("a bool byte must be 0 or 1")),
        }
    }
}

wire_fixtures!(bool, false => "00", true => "01");

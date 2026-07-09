//! The unified wire codec for Logos blockchain components.
//!
//! One encode trait ([`WireEncode`]) and one decode trait ([`WireDecode`]) that
//! every on-the-wire type implements, replacing the previously separate
//! `NomEncode`/`NomDecode` (Mantle) and `WireEncode`/`WireDecode` (Blend)
//! families. Primitives, fixed-size arrays and `BoundedVec` get default impls
//! here; domain types implement the traits in their own crate (orphan rule) via
//! `#[derive(WireCodec)]` for the trivial field-order case, or by hand.
//!
//! Every codec must also ship at least one **well-known fixture** (a value and
//! its exact wire bytes). This is enforced at compile time: both codec traits
//! require [`WireExamples`], whose only sanctioned implementation path is
//! [`wire_fixtures!`] / `#[derive(WireCodec)]`, so a codec without a fixture is a
//! `cargo build` error.
//!
//! `encode`/`decode` never allocate beyond the caller's buffer, use little-endian
//! integers, and decode returns the value plus the unconsumed remainder
//! (`(rest, value)`, as in `nom`).

// The derive and `wire_fixtures!` expansions refer to this crate as `::lb_wire`,
// so the crate must be able to name itself that way when it uses them for its own
// primitives.
extern crate self as lb_wire;

mod array;
mod boolean;
mod bounded_vec;
#[cfg(test)]
mod derive_smoke;
mod error;
mod fixtures;
mod numbers;

pub use error::DecodeError;
pub use fixtures::{
    WireExamples, WireFixture, WireFixtures, assert_wire_fixtures, assert_wire_fixtures_with,
};
pub use lb_wire_macros::{WireCodec, wire_fixtures};

/// Sealed marker that gates [`WireExamples`] to the blessed macro path.
///
/// `#[doc(hidden)] pub` (rather than `pub(crate)`) so the `wire_fixtures!` /
/// `#[derive(WireCodec)]` expansions can implement it from any downstream crate;
/// undocumented, so the macros remain the only sanctioned way to satisfy it.
#[doc(hidden)]
pub mod sealed {
    pub trait Sealed {}
}

/// Append a value's wire bytes to a caller-owned buffer.
///
/// Requires [`WireExamples`]: a type cannot be a wire codec without also pinning
/// a well-known fixture.
pub trait WireEncode: WireExamples {
    /// The exact number of bytes [`encode_into`](Self::encode_into) will append,
    /// computed without encoding or allocating.
    fn encoded_length(&self) -> usize;

    /// Append this value's wire bytes to `out`. The single required
    /// serialization primitive; composites chain their children's `encode_into`.
    fn encode_into(&self, out: &mut Vec<u8>);

    /// Encode into a freshly allocated, exactly-sized boxed slice.
    fn encode(&self) -> Box<[u8]> {
        let mut out = Vec::with_capacity(self.encoded_length());
        self.encode_into(&mut out);
        out.into_boxed_slice()
    }

    /// Encode into a freshly allocated `Vec<u8>` — the ergonomic bridge for the
    /// many call sites that feed encoded bytes into a `Vec<u8>` sink.
    fn encode_to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_length());
        self.encode_into(&mut out);
        out
    }
}

/// Decode a value from the front of `input`, returning it and the unconsumed
/// remainder (`(rest, value)`, as in `nom`).
///
/// `Context` carries anything the decoder needs that is not on the wire (e.g. a
/// layer count); it is `()` for self-describing components. Requires
/// [`WireExamples`] for the same reason as [`WireEncode`].
pub trait WireDecode: WireExamples + Sized {
    type Context;

    fn decode(input: &[u8], context: Self::Context) -> Result<(&[u8], Self), DecodeError>;
}

/// Ergonomic decode for the common `Context = ()` case: `T::decode_default(bytes)`
/// instead of `T::decode(bytes, ())`.
pub trait WireDecodeExt: WireDecode<Context = ()> {
    fn decode_default(input: &[u8]) -> Result<(&[u8], Self), DecodeError> {
        Self::decode(input, ())
    }
}

impl<T: WireDecode<Context = ()>> WireDecodeExt for T {}

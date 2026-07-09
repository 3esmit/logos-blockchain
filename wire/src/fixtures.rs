use std::borrow::Cow;

use lb_utils::bounded::LowerBoundedVec;

use crate::{WireDecode, WireEncode};

/// Carries the mandatory [`WireFixtures`] for a codec. The non-empty return type
/// means a codec cannot exist without at least one fixture.
///
/// Sealed via [`crate::sealed::Sealed`], so the only ways to satisfy it are
/// `#[derive(WireCodec)]` and `wire_fixtures!`, both of which demand a fixture.
/// It is a supertrait of both codec traits, so `impl WireEncode for Foo` without
/// a fixture is a `cargo build` error (E0277).
pub trait WireExamples: crate::sealed::Sealed + Sized {
    #[must_use]
    fn fixtures() -> WireFixtures<Self>;
}

/// A single golden vector: a value and its exact wire bytes.
///
/// `bytes` is a [`Cow`] so leaf fixtures can borrow a `&'static` slice (emitted
/// by the macros) while generic blanket impls build theirs from the element's
/// fixtures ([`Cow::Owned`]).
pub struct WireFixture<T> {
    pub value: T,
    pub bytes: Cow<'static, [u8]>,
}

/// A codec's well-known fixtures: at least one `(value, bytes)` pair, up to as
/// many as needed. The `1`-lower-bounded type is what makes "a codec cannot exist
/// without a fixture" part of the contract.
pub type WireFixtures<T> = LowerBoundedVec<WireFixture<T>, 1>;

/// Drives every fixture of a `Context = ()` codec through the wire-format
/// invariants. Called by the round-trip test the macros generate.
///
/// `#[doc(hidden)] pub` (not `#[cfg(test)]`) because the generated test lives in
/// *downstream* crates and calls this against `lb-wire`'s non-test build.
#[doc(hidden)]
pub fn assert_wire_fixtures<T>()
where
    T: WireEncode + WireDecode<Context = ()> + PartialEq + core::fmt::Debug,
{
    assert_wire_fixtures_with::<T>(|| ());
}

/// Like [`assert_wire_fixtures`], but for codecs whose `Context` is not `()`:
/// `make_context` produces a fresh context per decode.
#[doc(hidden)]
pub fn assert_wire_fixtures_with<T>(make_context: impl Fn() -> T::Context)
where
    T: WireEncode + WireDecode + PartialEq + core::fmt::Debug,
{
    let type_name = core::any::type_name::<T>();

    for fixture in T::fixtures() {
        let expected = fixture.bytes.as_ref();

        // Golden encode: the value serializes to exactly the pinned bytes. On a
        // mismatch we print both sides hex-encoded — the `assert_eq!` default
        // would dump raw `[u8]` arrays in decimal.
        let encoded = fixture.value.encode();
        assert!(
            &*encoded == expected,
            "{type_name}: encode(value) drifted from the well-known bytes\n  value: {:?}\n  actual   (hex): {actual}\n  expected (hex): {expected_hex}",
            fixture.value,
            actual = hex::encode(&*encoded),
            expected_hex = hex::encode(expected),
        );

        // `encoded_length` must agree with the real byte count, or release
        // builds (where downstream `debug_assert`s are gone) silently mis-size.
        assert_eq!(
            fixture.value.encoded_length(),
            encoded.len(),
            "{type_name}: encoded_length() disagrees with encode().len()",
        );

        // Golden decode: the pinned bytes decode back to the value, leaving
        // nothing behind.
        let (rest, decoded) = T::decode(expected, make_context()).unwrap_or_else(|err| {
            panic!(
                "{type_name}: well-known bytes failed to decode: {err:?}\n  bytes (hex): {}",
                hex::encode(expected),
            )
        });
        assert!(
            rest.is_empty(),
            "{type_name}: well-known bytes left trailing data (hex): {}",
            hex::encode(rest),
        );
        assert!(
            decoded == fixture.value,
            "{type_name}: decode(bytes) != value\n  bytes (hex): {bytes}\n  decoded:  {decoded:?}\n  expected: {expected_value:?}",
            bytes = hex::encode(expected),
            expected_value = &fixture.value,
        );

        // Round-trip: encode then decode is the identity (independent of the
        // pinned bytes, so it catches encode/decode asymmetry directly).
        let (rest, round_tripped) = T::decode(&encoded, make_context())
            .unwrap_or_else(|err| panic!("{type_name}: round-trip decode failed: {err:?}"));
        assert!(
            rest.is_empty(),
            "{type_name}: round-trip left trailing data (hex): {}",
            hex::encode(rest),
        );
        assert!(
            round_tripped == fixture.value,
            "{type_name}: round-trip changed the value\n  before: {before:?}\n  after:  {round_tripped:?}",
            before = &fixture.value,
        );
    }
}

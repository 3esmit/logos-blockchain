//! Transitional facade for the Mantle wire codec, which now lives in the shared
//! [`lb_wire`] crate.
//!
//! The traits, `DecodeError`, the primitive/container impls, and the fixture
//! machinery all moved to `lb-wire`; the former foreign-type impls
//! (`Ed25519*`/`Zk*`, `ProofOf*`, `Fr`, `Epoch`) moved to their owning crates.
//! This module re-exports the new items under their former Mantle names so
//! existing call sites keep compiling, and hosts the well-known fixtures for
//! the Mantle-local codecs. It will be removed once call sites move to
//! `lb_wire` directly.

pub use lb_wire::{
    DecodeError, WireCodec as NomCodec, WireDecode as NomDecode, WireEncode as NomEncode,
    WireExamples, WireFixture, WireFixtures, wire_fixtures as nom_wire_fixtures,
};

mod fixtures;

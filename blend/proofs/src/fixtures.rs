//! Well-known wire fixtures for this crate's codecs.
//!
//! Each [`wire_fixtures!`](lb_wire::wire_fixtures) emits the type's
//! `WireExamples` impl (the fixture the codec traits require) plus a
//! `#[cfg(test)]` round-trip test. Gathering them here keeps the codec impls in
//! [`crate::wire`] free of fixture noise and gives a single auditable list of
//! every golden vector.

use lb_wire::wire_fixtures;

use crate::{
    quota::{ProofOfQuota, VerifiedProofOfQuota},
    selection::{ProofOfSelection, VerifiedProofOfSelection},
};

wire_fixtures!(
    ProofOfQuota,
    VerifiedProofOfQuota::from_bytes_unchecked([1u8; _]).into() => "01010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101"
);

wire_fixtures!(
    ProofOfSelection,
    VerifiedProofOfSelection::from_bytes_unchecked([1u8; _]).into() => "0101010101010101010101010101010101010101010101010101010101010101"
);

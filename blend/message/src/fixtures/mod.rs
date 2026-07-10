//! Centralized well-known wire fixtures for this crate's codecs.
//!
//! Each [`wire_fixtures!`](lb_wire::wire_fixtures) emits the type's
//! `WireExamples` impl (the fixture the codec traits require) plus a
//! `#[cfg(test)]` round-trip test. Gathering them here keeps the type
//! definitions free of fixture noise and gives a single auditable list of every
//! golden vector.
//!
//! The three encapsulated-part leaves (`EncapsulatedBlendingHeader`,
//! `EncapsulatedPrivateHeader`, `EncapsulatedPayload`) are private byte
//! containers, so their fixtures stay next to the types in
//! [`crate::encap::encapsulated`] rather than being exposed here.

use lb_blend_proofs::{
    quota::{PROOF_OF_QUOTA_SIZE, VerifiedProofOfQuota},
    selection::{PROOF_OF_SELECTION_SIZE, VerifiedProofOfSelection},
};
use lb_key_management_system_keys::keys::{
    ED25519_PUBLIC_KEY_SIZE, ED25519_SIGNATURE_SIZE, Ed25519PublicKey, Ed25519Signature,
};
use lb_wire::wire_fixtures;

use crate::{
    PaddedPayloadBody, PayloadType,
    encap::{
        encapsulated::{EncapsulatedMessage, EncapsulatedPart},
        validated::{
            EncapsulatedMessageWithVerifiedPublicHeader, EncapsulatedMessageWithVerifiedSignature,
            wire_fixture_message,
        },
    },
    message::{
        blending_header::BlendingHeader,
        payload::Payload,
        public_header::{PublicHeader, PublicHeaderWithVerifiedSignature, VerifiedPublicHeader},
    },
};

// -- Payload ---------------------------------------------------------------

wire_fixtures!(PayloadType, Self::Cover => "00", Self::Data => "01");

// Well-known bytes: a `u16` length of 3, the body `[1, 2, 3]`, then zero
// padding to `MAX_PAYLOAD_BODY_SIZE`. Externalised as hex because it is ~34
// KiB.
wire_fixtures!(
    PaddedPayloadBody,
    Self::zero_padded(&[1u8, 2, 3]).unwrap()
        => include_str!("padded_payload_body.hex")
);

// Well-known bytes: the `Data` discriminant (`0x01`), a `u16` length of 3, the
// body `[4, 5, 6]`, then zero padding. Externalised as hex because it is ~34
// KiB.
wire_fixtures!(
    Payload,
    Self::new(
        PayloadType::Data,
        PaddedPayloadBody::zero_padded(&[4u8, 5, 6]).unwrap(),
    ) => include_str!("payload.hex")
);

// -- Headers ---------------------------------------------------------------

wire_fixtures!(
    BlendingHeader,
    Self {
        signing_pubkey: Ed25519PublicKey::from_bytes(&[0; ED25519_PUBLIC_KEY_SIZE]).unwrap(),
        proof_of_quota: VerifiedProofOfQuota::from_bytes_unchecked([1; PROOF_OF_QUOTA_SIZE])
            .into_inner(),
        signature: Ed25519Signature::from_bytes(&[2; ED25519_SIGNATURE_SIZE]),
        proof_of_selection: VerifiedProofOfSelection::from_bytes_unchecked(
            [3; PROOF_OF_SELECTION_SIZE],
        )
        .into_inner(),
        is_last: false,
    } => "00000000000000000000000000000000000000000000000000000000000000000101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010102020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202030303030303030303030303030303030303030303030303030303030303030300"
);

/// The well-known bytes of a `PublicHeader` (version `0x01`, the reconstructed
/// signing key of all `0x00`, a proof of quota of all `0x01`, and a signature
/// of all `0x02`). Shared by the `PublicHeader` fixture and the two verified
/// wrappers, which encode to the same bytes.
const PUBLIC_HEADER_HEX: &str = "0100000000000000000000000000000000000000000000000000000000000000000101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010102020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202";

wire_fixtures!(
    PublicHeader,
    Self::new(
        Ed25519PublicKey::from_bytes(&[0; ED25519_PUBLIC_KEY_SIZE]).unwrap(),
        &VerifiedProofOfQuota::from_bytes_unchecked([1; PROOF_OF_QUOTA_SIZE]).into_inner(),
        Ed25519Signature::from_bytes(&[2; ED25519_SIGNATURE_SIZE]),
    ) => PUBLIC_HEADER_HEX
);

wire_fixtures!(
    PublicHeaderWithVerifiedSignature,
    encode_only,
    Self::new(
        VerifiedProofOfQuota::from_bytes_unchecked([1; PROOF_OF_QUOTA_SIZE]).into_inner(),
        Ed25519PublicKey::from_bytes(&[0; ED25519_PUBLIC_KEY_SIZE]).unwrap(),
        Ed25519Signature::from_bytes(&[2; ED25519_SIGNATURE_SIZE]),
    ) => PUBLIC_HEADER_HEX
);

wire_fixtures!(
    VerifiedPublicHeader,
    encode_only,
    Self::new(
        VerifiedProofOfQuota::from_bytes_unchecked([1; PROOF_OF_QUOTA_SIZE]),
        Ed25519PublicKey::from_bytes(&[0; ED25519_PUBLIC_KEY_SIZE]).unwrap(),
        Ed25519Signature::from_bytes(&[2; ED25519_SIGNATURE_SIZE]),
    ) => PUBLIC_HEADER_HEX
);

// -- Encapsulated message --------------------------------------------------
//
// All three message types encode to the same bytes: a genuine, deterministic
// single-layer encapsulation built by [`wire_fixture_message`].

wire_fixtures!(
    EncapsulatedMessage,
    decode_only,
    context = core::num::NonZeroU64::new(1).unwrap(),
    EncapsulatedMessage::from(wire_fixture_message())
        => include_str!("encapsulated_message.hex")
);

wire_fixtures!(
    EncapsulatedPart,
    context = core::num::NonZeroU64::new(1).unwrap(),
    EncapsulatedMessage::from(wire_fixture_message())
        .into_components()
        .1 => include_str!("encapsulated_part.hex")
);

wire_fixtures!(
    EncapsulatedMessageWithVerifiedSignature,
    encode_only,
    EncapsulatedMessageWithVerifiedSignature::from(wire_fixture_message())
        => include_str!("encapsulated_message.hex")
);

wire_fixtures!(
    EncapsulatedMessageWithVerifiedPublicHeader,
    encode_only,
    wire_fixture_message() => include_str!("encapsulated_message.hex")
);

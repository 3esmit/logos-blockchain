use lb_blend_proofs::{
    quota::{PROOF_OF_QUOTA_SIZE, VerifiedProofOfQuota},
    selection::{PROOF_OF_SELECTION_SIZE, VerifiedProofOfSelection},
};
use lb_key_management_system_keys::keys::{
    ED25519_PUBLIC_KEY_SIZE, ED25519_SIGNATURE_SIZE, Ed25519PublicKey, Ed25519Signature,
    UnsecuredEd25519Key,
};
use lb_wire::wire_fixtures;

use crate::{
    PaddedPayloadBody, PayloadType,
    encap::{
        encapsulated::{
            EncapsulatedBlendingHeader, EncapsulatedMessage, EncapsulatedPart, EncapsulatedPayload,
            EncapsulatedPrivateHeader,
        },
        validated::{
            EncapsulatedMessageWithVerifiedPublicHeader, EncapsulatedMessageWithVerifiedSignature,
        },
    },
    input::EncapsulationInput,
    message::{
        blending_header::BlendingHeader,
        payload::Payload,
        public_header::{PublicHeader, PublicHeaderWithVerifiedSignature, VerifiedPublicHeader},
    },
};

// -- Payload ---------------------------------------------------------------

wire_fixtures!(PayloadType, Self::Cover => "00", Self::Data => "01");

wire_fixtures!(
    PaddedPayloadBody,
    Self::try_from(&[1u8, 2, 3][..]).unwrap()
        => include_str!("padded_payload_body.hex")
);

wire_fixtures!(
    Payload,
    Self::new(
        PayloadType::Data,
        PaddedPayloadBody::try_from(&[4u8, 5, 6][..]).unwrap(),
    ) => include_str!("payload.hex")
);

wire_fixtures!(
    EncapsulatedPayload,
    Self::initialize(&Payload::new(
        PayloadType::Data,
        PaddedPayloadBody::try_from(&[7u8, 8, 9][..]).unwrap(),
    )) => include_str!("encapsulated_payload.hex")
);

// -- Headers ---------------------------------------------------------------

wire_fixtures!(
    EncapsulatedPrivateHeader,
    context = core::num::NonZeroU64::new(1).unwrap(),
    Self::try_initialize(
        &[EncapsulationInput::try_new(
            UnsecuredEd25519Key::from_bytes(&[1u8; 32]),
            &UnsecuredEd25519Key::from_bytes(&[2u8; 32]).public_key(),
            VerifiedProofOfQuota::from_bytes_unchecked([0u8; PROOF_OF_QUOTA_SIZE]),
            VerifiedProofOfSelection::from_bytes_unchecked([0u8; PROOF_OF_SELECTION_SIZE]),
        ).unwrap()]
    ).unwrap() => "07070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707070707"
);

wire_fixtures!(
    EncapsulatedBlendingHeader,
    Self::initialize(&BlendingHeader::pseudo_random(&[1u8; 32])) => "00"
);

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

fn wire_fixture_message() -> EncapsulatedMessageWithVerifiedPublicHeader {
    let recipient_signing_key = UnsecuredEd25519Key::from_bytes(&[1u8; 32]);
    let inputs = [EncapsulationInput::try_new(
        UnsecuredEd25519Key::from_bytes(&[2u8; 32]),
        &recipient_signing_key.public_key(),
        VerifiedProofOfQuota::from_bytes_unchecked([0u8; PROOF_OF_QUOTA_SIZE]),
        VerifiedProofOfSelection::from_bytes_unchecked([0u8; PROOF_OF_SELECTION_SIZE]),
    )
    .expect("well-known encapsulation input is valid")];

    let payload_body = PaddedPayloadBody::try_from(b"well-known blend message payload".as_ref())
        .expect("payload body fits");

    let (part, signing_key, proof_of_quota) = inputs.iter().enumerate().fold(
        (
            EncapsulatedPart::try_initialize(&inputs, PayloadType::Data, payload_body)
                .expect("inputs are non-empty"),
            // Fixed stand-ins for `try_new`'s randomly-sampled outer-sender identity.
            UnsecuredEd25519Key::from_bytes(&[3u8; 32]),
            VerifiedProofOfQuota::from_bytes_unchecked([0u8; PROOF_OF_QUOTA_SIZE]),
        ),
        |(part, signing_key, proof_of_quota), (i, input)| {
            (
                part.encapsulate(
                    input.ephemeral_encryption_key(),
                    &signing_key,
                    &proof_of_quota,
                    *input.proof_of_selection(),
                    i == 0,
                ),
                input.ephemeral_signing_key().clone(),
                *input.proof_of_quota(),
            )
        },
    );

    EncapsulatedMessageWithVerifiedPublicHeader::from_components(
        VerifiedPublicHeader::new(
            proof_of_quota,
            signing_key.public_key(),
            part.sign(&signing_key),
        ),
        part,
    )
}

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

#[cfg(test)]
#[test]
fn __generate_leaf_fixtures() {
    use core::fmt::Write as _;

    use lb_wire::WireEncode as _;

    fn to_hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
                let _ = write!(&mut hex, "{byte:02x}");
                hex
            })
    }

    let blending_header =
        EncapsulatedBlendingHeader::initialize(&BlendingHeader::pseudo_random(&[1u8; 32]));
    std::fs::write(
        "src/fixtures/__blending_header.hex",
        to_hex(&blending_header.encode()),
    )
    .unwrap();

    let payload = EncapsulatedPayload::initialize(&Payload::new(
        PayloadType::Data,
        PaddedPayloadBody::try_from(vec![7u8, 8, 9]).unwrap(),
    ));
    std::fs::write(
        "src/fixtures/encapsulated_payload.hex",
        to_hex(&payload.encode()),
    )
    .unwrap();

    let private_header = EncapsulatedPrivateHeader::try_initialize(&[EncapsulationInput::try_new(
        UnsecuredEd25519Key::from_bytes(&[1u8; 32]),
        &UnsecuredEd25519Key::from_bytes(&[2u8; 32]).public_key(),
        VerifiedProofOfQuota::from_bytes_unchecked([0u8; PROOF_OF_QUOTA_SIZE]),
        VerifiedProofOfSelection::from_bytes_unchecked([0u8; PROOF_OF_SELECTION_SIZE]),
    )
    .unwrap()])
    .unwrap();
    std::fs::write(
        "src/fixtures/__private_header.hex",
        to_hex(&private_header.encode()),
    )
    .unwrap();
}

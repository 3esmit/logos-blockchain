pub mod channel;
pub mod leader_claim;
pub mod sdp;
pub mod transfer;

pub(crate) mod internal;

pub(crate) mod codec;
mod serde_;

use std::sync::LazyLock;

use channel::{
    config::ChannelConfigOp, deposit::DepositOp, inscribe::InscriptionOp,
    withdraw::ChannelWithdrawOp,
};
use lb_key_management_system_keys::keys::{Ed25519Signature, ZkSignature};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    gas::{Gas, GasConstants},
    ops::{
        leader_claim::LeaderClaimOp,
        sdp::{SDPActiveOp, SDPDeclareOp, SDPWithdrawOp},
    },
};
use crate::{
    crypto::{Digest as _, Hash, Hasher},
    mantle::{
        nom::{DecodeError, NomDecode, NomEncode},
        ops::{
            internal::{OpDe, OpSer},
            transfer::TransferOp,
        },
    },
    proofs::{
        channel_multi_sig_proof::ChannelMultiSigProof, leader_claim_proof::Groth16LeaderClaimProof,
    },
};

static OPERATION_ID_V1: LazyLock<Vec<u8>> = LazyLock::new(|| b"OPERATION_ID_V1".to_vec());

pub trait OpId {
    fn op_id(&self) -> Hash {
        let mut encoded_bytes = OPERATION_ID_V1.clone();
        encoded_bytes.extend(self.op_bytes());
        Hasher::digest(&encoded_bytes).into()
    }

    fn op_bytes(&self) -> Vec<u8>;
}

const TRANSFER: u8 = 0x00;
const CHANNEL_CONFIG: u8 = 0x10;
const INSCRIBE: u8 = 0x11;
const CHANNEL_DEPOSIT: u8 = 0x12;
const CHANNEL_WITHDRAW: u8 = 0x13;
const SDP_DECLARE: u8 = 0x20;
const SDP_WITHDRAW: u8 = 0x21;
const SDP_ACTIVE: u8 = 0x22;
const LEADER_CLAIM: u8 = 0x30;

/// Core set of supported Mantle operations.
///
/// This type serves as the public-facing representation of [`OpSer`] and
/// [`OpDe`], delegating default serialization and deserialization to them.
///
/// Serialization and deserialization share a single [`serde_::OpWire`] wire
/// shape, which carries an `opcode` tag used to identify the correct variant.
/// Due to limitations in [`bincode`] and [`serde`]'s `#[serde(untagged)]`
/// enums, binary deserialization is routed through [`decode_op`] instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Op {
    ChannelInscribe(InscriptionOp),
    ChannelConfig(ChannelConfigOp),
    ChannelDeposit(DepositOp),
    ChannelWithdraw(ChannelWithdrawOp),
    SDPDeclare(SDPDeclareOp),
    SDPWithdraw(SDPWithdrawOp),
    SDPActive(SDPActiveOp),
    LeaderClaim(LeaderClaimOp),
    Transfer(TransferOp),
}

/// Delegates serialization through the [`OpInternal`] representation.
impl Serialize for Op {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            let op_ser = OpSer::from(self);
            op_ser.serialize(serializer)
        } else {
            let bytes = self.encode();
            serializer.serialize_bytes(&bytes)
        }
    }
}

/// Delegates deserialization through the [`OpDe`] representation.
///
/// If the deserializer is non-human-readable it falls back into custom
/// decoding via [`decode_op`]. Otherwise, it deserializes via [`OpDe`]'s
/// default behaviour.
impl<'de> Deserialize<'de> for Op {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            OpDe::deserialize(deserializer).map(Self::from)
        } else {
            let bytes = <Vec<u8>>::deserialize(deserializer)?;
            Self::decode(&bytes, ())
                .map(|(_, op)| op)
                .map_err(serde::de::Error::custom)
        }
    }
}

// Op = Opcode OpPayload
impl NomEncode for Op {
    fn encoded_length(&self) -> usize {
        let payload = match self {
            Self::ChannelInscribe(op) => op.encoded_length(),
            Self::ChannelConfig(op) => op.encoded_length(),
            Self::ChannelDeposit(op) => op.encoded_length(),
            Self::ChannelWithdraw(op) => op.encoded_length(),
            Self::SDPDeclare(op) => op.encoded_length(),
            Self::SDPWithdraw(op) => op.encoded_length(),
            Self::SDPActive(op) => op.encoded_length(),
            Self::LeaderClaim(op) => op.encoded_length(),
            Self::Transfer(op) => op.encoded_length(),
        };
        self.code().encoded_length() + payload
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        self.code().encode_into(out);
        match self {
            Self::ChannelInscribe(op) => op.encode_into(out),
            Self::ChannelConfig(op) => op.encode_into(out),
            Self::ChannelDeposit(op) => op.encode_into(out),
            Self::ChannelWithdraw(op) => op.encode_into(out),
            Self::SDPDeclare(op) => op.encode_into(out),
            Self::SDPWithdraw(op) => op.encode_into(out),
            Self::SDPActive(op) => op.encode_into(out),
            Self::LeaderClaim(op) => op.encode_into(out),
            Self::Transfer(op) => op.encode_into(out),
        }
    }
}

impl NomDecode for Op {
    type Context = ();

    fn decode(input: &[u8], (): Self::Context) -> Result<(&[u8], Self), DecodeError> {
        let (input, opcode) = u8::decode(input, ())?;

        match opcode {
            INSCRIBE => {
                InscriptionOp::decode(input, ()).map(|(rest, op)| (rest, Self::ChannelInscribe(op)))
            }
            CHANNEL_CONFIG => {
                ChannelConfigOp::decode(input, ()).map(|(rest, op)| (rest, Self::ChannelConfig(op)))
            }
            CHANNEL_DEPOSIT => {
                DepositOp::decode(input, ()).map(|(rest, op)| (rest, Self::ChannelDeposit(op)))
            }
            CHANNEL_WITHDRAW => ChannelWithdrawOp::decode(input, ())
                .map(|(rest, op)| (rest, Self::ChannelWithdraw(op))),
            SDP_DECLARE => {
                SDPDeclareOp::decode(input, ()).map(|(rest, op)| (rest, Self::SDPDeclare(op)))
            }
            SDP_WITHDRAW => {
                SDPWithdrawOp::decode(input, ()).map(|(rest, op)| (rest, Self::SDPWithdraw(op)))
            }
            SDP_ACTIVE => {
                SDPActiveOp::decode(input, ()).map(|(rest, op)| (rest, Self::SDPActive(op)))
            }
            LEADER_CLAIM => {
                LeaderClaimOp::decode(input, ()).map(|(rest, op)| (rest, Self::LeaderClaim(op)))
            }
            TRANSFER => TransferOp::decode(input, ()).map(|(rest, op)| (rest, Self::Transfer(op))),
            other => Err(DecodeError::unknown_discriminant::<Self>(u64::from(other))),
        }
    }
}

// We just check that the enum discriminant tag is encoded correctly, so a
// single fixture is fine here.
// TODO: Remove once the `NomCodec` macro supports enums.

impl Op {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ChannelInscribe(_) => "ChannelInscribe",
            Self::ChannelConfig(_) => "ChannelConfig",
            Self::ChannelDeposit(_) => "ChannelDeposit",
            Self::ChannelWithdraw(_) => "ChannelWithdraw",
            Self::SDPDeclare(_) => "SDPDeclare",
            Self::SDPWithdraw(_) => "SDPWithdraw",
            Self::SDPActive(_) => "SDPActive",
            Self::LeaderClaim(_) => "LeaderClaim",
            Self::Transfer(_) => "Transfer",
        }
    }

    #[must_use]
    pub const fn execution_gas<Constants: GasConstants>(&self) -> Gas {
        match self {
            Self::ChannelInscribe(_) => Constants::CHANNEL_INSCRIBE,
            Self::ChannelConfig(_) => Constants::CHANNEL_CONFIG,
            Self::ChannelDeposit(_) => Constants::CHANNEL_DEPOSIT,
            Self::ChannelWithdraw(_) => Constants::CHANNEL_WITHDRAW,
            Self::SDPDeclare(_) => Constants::SDP_DECLARE,
            Self::SDPWithdraw(_) => Constants::SDP_WITHDRAW,
            Self::SDPActive(_) => Constants::SDP_ACTIVE,
            Self::LeaderClaim(_) => Constants::LEADER_CLAIM,
            Self::Transfer(_) => Constants::TRANSFER,
        }
    }

    const fn code(&self) -> u8 {
        match self {
            Self::ChannelInscribe(_) => INSCRIBE,
            Self::ChannelConfig(_) => CHANNEL_CONFIG,
            Self::ChannelDeposit(_) => CHANNEL_DEPOSIT,
            Self::ChannelWithdraw(_) => CHANNEL_WITHDRAW,
            Self::SDPDeclare(_) => SDP_DECLARE,
            Self::SDPWithdraw(_) => SDP_WITHDRAW,
            Self::SDPActive(_) => SDP_ACTIVE,
            Self::LeaderClaim(_) => LEADER_CLAIM,
            Self::Transfer(_) => TRANSFER,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpProof {
    Ed25519Sig(Ed25519Signature),
    ZkSig(ZkSignature),
    ZkAndEd25519Sigs {
        zk_sig: ZkSignature,
        ed25519_sig: Ed25519Signature,
    },
    PoC(Groth16LeaderClaimProof),
    ChannelMultiSigProof(ChannelMultiSigProof),
}

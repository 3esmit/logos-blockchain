use lb_core::{
    mantle::{
        MantleTx, NoteId, SignedMantleTx, Transaction as _,
        channel::{ChannelState, SlotTimeframe, SlotTimeout},
        encoding::Ops,
        ops::{
            Op, OpProof,
            channel::{
                ChannelId, ChannelKeyIndex, MsgId,
                config::{ChannelConfigOp, Keys},
                inscribe::{Inscription, InscriptionOp},
                stake::ChannelStakeOp,
            },
        },
        tx::TxHash,
    },
    proofs::channel_multi_sig_proof::{ChannelMultiSigProof, IndexedSignature},
};
use lb_key_management_system_service::keys::{Ed25519Key, Ed25519Signature, ZkKey};

use super::types::Error;

/// Build per-op proofs for an atomic withdraw bundle. The same single-signer
/// `ChannelMultiSigProof` is reused for every `ChannelWithdraw` op (all sign
/// the same tx hash with the same key) and the inscription op carries an
/// `Ed25519Sig` proof.
pub(super) fn build_atomic_withdraw_ops_proofs(
    tx: &MantleTx,
    own_key_index: ChannelKeyIndex,
    own_sig: Ed25519Signature,
) -> Result<Vec<OpProof>, Error> {
    let withdraw_proof =
        ChannelMultiSigProof::new(vec![IndexedSignature::new(own_key_index, own_sig)])
            .map_err(|e| Error::Network(format!("multi-sig proof assembly failed: {e:?}")))?;
    let mut ops_proofs = Vec::with_capacity(tx.ops().len());
    for op in tx.ops() {
        match op {
            Op::ChannelWithdraw(_) => {
                ops_proofs.push(OpProof::ChannelMultiSigProof(withdraw_proof.clone()));
            }
            Op::ChannelInscribe(_) => ops_proofs.push(OpProof::Ed25519Sig(own_sig)),
            _ => {
                return Err(Error::Network(format!(
                    "unexpected op in atomic withdraw bundle: {op:?}"
                )));
            }
        }
    }
    Ok(ops_proofs)
}

/// Find the position of the SDK's public key in the channel's `accredited_keys`
/// list. Returns an error if our key is not on the accredited list (we can't
/// sign for this channel).
pub(super) fn find_own_key_index(
    channel_state: &ChannelState,
    signing_key: &Ed25519Key,
) -> Result<ChannelKeyIndex, Error> {
    let own_pk = signing_key.public_key();
    channel_state
        .accredited_keys
        .iter()
        .position(|k| *k == own_pk)
        .map(|i| i as ChannelKeyIndex)
        .ok_or_else(|| Error::Network("sequencer key not in channel accredited_keys".into()))
}

pub(super) fn create_inscribe_tx(
    channel_id: ChannelId,
    signing_key: &Ed25519Key,
    inscription: Inscription,
    parent: MsgId,
    lottery_note_id: Option<NoteId>,
) -> (SignedMantleTx, MsgId) {
    let signer = signing_key.public_key();

    let inscribe_op = InscriptionOp {
        channel_id,
        inscription,
        parent,
        signer,
    };
    let msg_id = inscribe_op.id();

    // TODO: set realistic gas prices and fund tx
    let inscribe_tx = MantleTx([Op::ChannelInscribe(inscribe_op)].into());

    let tx_hash = inscribe_tx.hash();
    let proof = lottery_note_id.map_or_else(
        // Round-robin channel: the accredited key signs just `tx_hash`.
        || OpProof::Ed25519Sig(sign_tx(tx_hash, signing_key)),
        // Lottery channel: the posting key signs `tx_hash || note_id`, binding
        // the won note into the signature, and the proof names that note.
        |note_id| {
            let mut message = tx_hash.as_signing_bytes().as_ref().to_vec();
            message.extend_from_slice(&lb_groth16::fr_to_bytes(&note_id.0));
            OpProof::LotteryWin {
                note_id,
                sig: signing_key.sign_payload(&message),
            }
        },
    );

    let signed_tx = SignedMantleTx {
        ops_proofs: vec![proof],
        mantle_tx: inscribe_tx,
    };

    (signed_tx, msg_id)
}

/// Build a signed `CHANNEL_STAKE` tx that registers `note_id` under `posting_key`
/// in a lottery channel's stake registry.
///
/// The node wallet refuses to sign staking ops, so the SDK signs them: the
/// note's `staking_key` (zk) proves ownership and the `posting_key` (Ed25519)
/// proves control of the bound posting identity. The caller submits the result
/// via [`crate::adapter::Node::post_transaction`].
pub fn create_stake_tx(
    channel_id: ChannelId,
    note_id: NoteId,
    staking_key: &ZkKey,
    posting_key: &Ed25519Key,
) -> Result<SignedMantleTx, Error> {
    let stake_op = ChannelStakeOp {
        channel_id,
        note_id,
        posting_key: posting_key.public_key(),
    };

    // TODO: fund tx
    let tx = MantleTx([Op::ChannelStake(stake_op)].into());
    let tx_hash = tx.hash();

    let zk_sig = ZkKey::multi_sign(std::slice::from_ref(staking_key), &tx_hash.to_fr())
        .map_err(|e| Error::Network(format!("stake zk signature failed: {e:?}")))?;
    let ed25519_sig = posting_key.sign_payload(tx_hash.as_signing_bytes().as_ref());

    Ok(SignedMantleTx {
        ops_proofs: vec![OpProof::ZkAndEd25519Sigs {
            zk_sig,
            ed25519_sig,
        }],
        mantle_tx: tx,
    })
}

pub(super) fn create_channel_config_tx(
    channel_id: ChannelId,
    signing_keys: &[&Ed25519Key],
    keys: Keys,
    posting_timeframe: SlotTimeframe,
    posting_timeout: SlotTimeout,
    configuration_threshold: u16,
    withdraw_threshold: u16,
) -> SignedMantleTx {
    let config_op = ChannelConfigOp {
        channel: channel_id,
        keys,
        posting_timeframe,
        posting_timeout,
        configuration_threshold,
        withdraw_threshold,
    };

    // TODO: fund tx
    let config_tx = MantleTx([Op::ChannelConfig(config_op)].into());

    let tx_hash = config_tx.hash();
    let signatures = signing_keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            IndexedSignature::new(
                index as ChannelKeyIndex,
                key.sign_payload(tx_hash.as_signing_bytes().as_ref()),
            )
        })
        .collect();
    let proof = ChannelMultiSigProof::new(signatures).unwrap();

    SignedMantleTx {
        ops_proofs: vec![OpProof::ChannelMultiSigProof(proof)],
        mantle_tx: config_tx,
    }
}

pub(super) fn prepare_tx(
    mut ops: Ops,
    channel_id: ChannelId,
    signing_key: &Ed25519Key,
    inscription: Inscription,
    parent: MsgId,
) -> (MantleTx, MsgId, Ed25519Signature) {
    let inscription_op = InscriptionOp {
        channel_id,
        inscription,
        parent,
        signer: signing_key.public_key(),
    };
    let msg_id = inscription_op.id();
    // TODO: Return `Error` in case there's too many ops already.
    ops.try_push(Op::ChannelInscribe(inscription_op)).unwrap();

    // TODO: fund tx
    let tx = MantleTx(ops);

    let inscription_sig = sign_tx(tx.hash(), signing_key);

    (tx, msg_id, inscription_sig)
}

pub(super) fn sign_tx(tx_hash: TxHash, signing_key: &Ed25519Key) -> Ed25519Signature {
    signing_key.sign_payload(tx_hash.as_signing_bytes().as_ref())
}

#[cfg(test)]
mod tests {
    use lb_groth16::Fr;
    use num_bigint::BigUint;

    use super::*;

    fn posting_key() -> Ed25519Key {
        Ed25519Key::from_bytes(&[7u8; 32])
    }

    #[test]
    fn lottery_inscription_carries_a_verifying_lottery_win_proof() {
        let posting = posting_key();
        let note_id = NoteId::from(Fr::from(42u64));
        let (signed, _msg_id) = create_inscribe_tx(
            ChannelId::from([1u8; 32]),
            &posting,
            b"hello".into(),
            MsgId::root(),
            Some(note_id),
        );

        assert!(
            matches!(signed.ops_proofs.as_slice(), [OpProof::LotteryWin { note_id: n, .. }] if *n == note_id),
            "lottery inscription must carry a LotteryWin proof naming the won note"
        );
        // Re-verifying re-runs `verify_ops_proofs`, which checks the posting key
        // signed `tx_hash || note_id` — confirming our signing bytes match the
        // canonical `NoteId::encode()` the validator uses.
        SignedMantleTx::new(signed.mantle_tx, signed.ops_proofs)
            .expect("the LotteryWin proof must verify");
    }

    #[test]
    fn round_robin_inscription_carries_a_verifying_ed25519_proof() {
        let posting = posting_key();
        let (signed, _msg_id) = create_inscribe_tx(
            ChannelId::from([1u8; 32]),
            &posting,
            b"hello".into(),
            MsgId::root(),
            None,
        );

        assert!(
            matches!(signed.ops_proofs.as_slice(), [OpProof::Ed25519Sig(_)]),
            "round-robin inscription must carry an Ed25519Sig proof"
        );
        SignedMantleTx::new(signed.mantle_tx, signed.ops_proofs)
            .expect("the round-robin proof must verify");
    }

    #[test]
    fn stake_tx_is_built_with_dual_signatures() {
        let posting = posting_key();
        let staking = ZkKey::from(BigUint::from(3u64));
        let note_id = NoteId::from(Fr::from(99u64));

        let signed = create_stake_tx(ChannelId::from([2u8; 32]), note_id, &staking, &posting)
            .expect("stake tx builds");

        assert!(
            matches!(
                signed.ops_proofs.as_slice(),
                [OpProof::ZkAndEd25519Sigs { .. }]
            ),
            "a CHANNEL_STAKE carries a zk (ownership) + Ed25519 (posting key) proof"
        );
        let ops: &[Op] = signed.mantle_tx.ops().as_ref();
        assert!(matches!(
            ops,
            [Op::ChannelStake(op)] if op.note_id == note_id && op.posting_key == posting.public_key()
        ));
    }
}

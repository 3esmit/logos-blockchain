use lb_core::mantle::{Note, TxHash, Utxo};
use lb_key_management_system_service::keys::ZkPublicKey;
use lb_ledger::UtxoTree;
use lb_wallet_service::UtxoWithKeyId;
use tokio::sync::mpsc;

use crate::{CryptarchiaLeader, WinningPolInfo, leadership::PotentialWinningPoLSlotNotifier};

#[tokio::test]
async fn try_build_and_propose_block() {
    let eligible = UtxoWithKeyId {
        utxo: Utxo {
            tx_hash: TxHash::default(),
            output_index: 0,
            note: Note {
                value: 1000,
                pk: ZkPublicKey::zero(),
            },
        },
        key_id: "key-0".to_owned(),
    };

    let mut latest_tree = UtxoTree::new();
    latest_tree.insert(eligible.utxo.id(), eligible.utxo.clone());

    let res = CryptarchiaLeader::try_build_and_propose_block(
        [1u8; 32], // parent
        10.into(), // slot
        &[eligible],
        &latest_tree,
        epoch_state,
        winning_pol_slot_notifier,
        chain_network_api,
        blend_adapter,
        wallet_api,
        kms_api,
        tx_selector,
        relays,
        tip_state,
        ledger_config,
    )
    .await;
}

fn dummy_potential_winning_slot_notifier<'service>(
    ledger_config: &'service lb_ledger::Config,
) -> (
    PotentialWinningPoLSlotNotifier<'service>,
    mpsc::Sender<Option<WinningPolInfo>>,
    mpsc::Receiver<Option<WinningPolInfo>>,
) {
    let (sender, receiver) = mpsc::channel(100);
    (
        PotentialWinningPoLSlotNotifier::new(ledger_config, &sender),
        sender,
        receiver,
    )
}

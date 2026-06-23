use std::{num::NonZeroU64, path::PathBuf, time::Duration};

use lb_config::kms::key_id_for_preload_backend;
use lb_http_api_common::bodies::wallet::transfer_funds::WalletTransferFundsRequestBody;
use lb_key_management_system_service::keys::{Key, ZkKey};
use lb_node::config::RunConfig;
use lb_testing_framework::{DeploymentBuilder, TopologyConfig as TfTopologyConfig};
use lb_utils::math::NonNegativeRatio;
use logos_blockchain_tests::{
    common::manual_cluster::{ManualNodeLayout, start_local_manual_cluster_with_layout},
    cucumber::defaults::E2E_ARTIFACTS_DIR,
};
use num_bigint::BigUint;
use serial_test::serial;
use testing_framework_core::scenario::DynError;

const ITERATIONS: usize = 100;
const TRANSFER_AMOUNT: u64 = 1;

#[tokio::test]
#[serial]
async fn cluster_propagates_tx_via_tip_poll() {
    // Imitates faucet key, which triggered error in the devnet.
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(b"test");
    let shared_sk = ZkKey::from(BigUint::from_bytes_le(&seed));
    let shared_pk = shared_sk.to_public_key();
    let shared_key = Key::Zk(shared_sk);
    let shared_key_id = key_id_for_preload_backend(&shared_key);

    let test_name = "cluster_propagates_tx_via_tip_poll";
    let node_count = 2;

    let (base, nodes) = start_local_manual_cluster_with_layout(
        test_name,
        "tip-poll-tx-prop",
        DeploymentBuilder::new(
            TfTopologyConfig::with_node_numbers(node_count)
                .with_test_context(Some(test_name.to_owned())),
        ),
        node_count,
        ManualNodeLayout::SelectNodeSeed(0),
        {
            let shared_key_id = shared_key_id.clone();
            let shared_key = shared_key.clone();
            move |mut cfg| {
                cfg = config(cfg);
                cfg.user
                    .kms
                    .backend
                    .keys
                    .insert(shared_key_id.clone(), shared_key.clone());
                cfg.user
                    .wallet
                    .known_keys
                    .insert(shared_key_id.clone(), shared_pk);
                Ok::<_, DynError>(cfg)
            }
        },
        Some(PathBuf::from(E2E_ARTIFACTS_DIR)),
    )
    .await;

    let funding_pks = [
        base.deployment().plans[0]
            .general
            .consensus_config
            .funding_pk,
        base.deployment().plans[1]
            .general
            .consensus_config
            .funding_pk,
    ];

    let pre_fund_amount = (ITERATIONS as u64) * TRANSFER_AMOUNT;

    for (i, pk) in funding_pks.iter().enumerate() {
        nodes[i]
            .client
            .transfer_funds(WalletTransferFundsRequestBody {
                tip: None,
                change_public_key: *pk,
                funding_public_keys: vec![*pk],
                recipient_public_key: shared_pk,
                amount: pre_fund_amount,
            })
            .await
            .unwrap_or_else(|_| panic!("Pre-funding transfer from Node {i} should succeed"));
    }

    tokio::time::sleep(Duration::from_secs(5)).await;

    for i in 0..ITERATIONS {
        let sender_node_idx = i % 2;
        let recipient_pk = funding_pks[sender_node_idx];

        println!("Node {sender_node_idx} submitting transfer to shared key...");

        // This fails with "Input note is missing in the Ledger".
        nodes[sender_node_idx]
            .client
            .transfer_funds(WalletTransferFundsRequestBody {
                tip: None,
                change_public_key: shared_pk,
                funding_public_keys: vec![shared_pk],
                recipient_public_key: recipient_pk,
                amount: TRANSFER_AMOUNT,
            })
            .await
            .expect("transfer funds should succeed");

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn config(mut config: RunConfig) -> RunConfig {
    config.deployment.time.slot_duration = Duration::from_secs(1);
    config.deployment.cryptarchia.security_param = 1.try_into().unwrap();
    config.deployment.cryptarchia.slot_activation_coeff =
        NonNegativeRatio::new(1, 2.try_into().unwrap());

    let cryptarchia = &mut config.user.cryptarchia;
    cryptarchia.service.bootstrap.prolonged_bootstrap_period = Duration::ZERO;
    cryptarchia.network.sync.tip_poll.lag_threshold_blocks = NonZeroU64::new(10).unwrap();

    let network = &mut cryptarchia.network.network;
    network.max_connected_peers_to_try_download = 2;
    network.max_discovered_peers_to_try_download = 2;

    config
}

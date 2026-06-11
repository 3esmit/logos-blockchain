use std::{
    collections::{HashMap, HashSet},
    num::NonZero,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use lb_chain_service::Epoch;
use lb_core::{
    mantle::{
        GenesisTx as _, MantleTx, NoteId, OpProof, SignedMantleTx, Transaction as _, Utxo,
        genesis_tx::GENESIS_STORAGE_GAS_PRICE,
        ops::Op,
        tx::{GasPrices, MantleTxGasContext},
        tx_builder::MantleTxBuilder,
    },
    sdp::{Declaration, DeclarationMessage, Locator, NumberOfEpochs, ServiceType, WithdrawMessage},
};
use lb_key_management_system_service::keys::{Ed25519Key, Ed25519Signature, ZkKey};
use lb_node::config::{
    RunConfig, blend::deployment::MinimumNetworkSize, cryptarchia::deployment::EpochConfig,
};
use lb_testing_framework::{
    DeploymentBuilder, NodeHttpClient, TopologyConfig as TfTopologyConfig,
    configs::wallet::{WalletAccount, WalletConfig},
};
use lb_utils::math::NonNegativeRatio;
use logos_blockchain_tests::{
    common::{
        chain::wait_for_transactions_inclusion,
        manual_cluster::{
            LocalManualClusterHarnessBase, build_local_manual_cluster, read_manual_node_logs,
            wait_for_height as wait_for_manual_cluster_height,
        },
        wallet::{current_wallet_funding_source, fund_builder_from_wallet_source},
    },
    cucumber::defaults::E2E_ARTIFACTS_DIR,
};
use num_bigint::BigUint;
use testing_framework_core::scenario::{DynError, StartNodeOptions};
use tokio::time::{sleep, timeout};

const LOCK_PERIOD: NumberOfEpochs = NumberOfEpochs::new(Epoch::new(1));

/// High-level SDP flow covered by this E2E:
/// - submit a `Declare` transaction backed by an unused genesis note and wait
///   for inclusion;
/// - advance past the lock period, `Withdraw`, and verify the declaration
///   disappears.
///
/// Note: Activity testing requires the blend service to generate real proofs,
/// which happens automatically for nodes that are declared as blend providers.
/// This test focuses on declare/withdraw flow which doesn't require blend
/// proofs.
#[tokio::test]
#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in these integration tests; boxing would not improve readability"
)]
async fn sdp_ops_e2e() {
    let (
        _cluster,
        _node0_name,
        node0,
        genesis_utxos,
        funding_wallet,
        spare_note_secret_key,
        spare_note_id,
        lock_period,
        slots_per_epoch,
        slot_duration,
    ) = start_sdp_manual_cluster("sdp-ops").await;

    let inclusion_timeout = Duration::from_mins(1);
    let state_timeout = Duration::from_secs(45);

    let existing = wait_for_sdp_declarations(&node0, Duration::from_secs(30))
        .await
        .expect("fetching SDP declarations should succeed");
    let locked: HashSet<_> = existing.iter().map(|decl| decl.locked_note_id).collect();
    let locked_note_id = spare_note_id;
    assert!(
        !locked.contains(&locked_note_id),
        "manual-cluster wallet note must be unused before submitting declare"
    );

    let provider_signing_key = Ed25519Key::from_bytes(&[7u8; 32]);
    let provider_zk_key = ZkKey::from(BigUint::from(7u64));
    let declaration = DeclarationMessage {
        service_type: ServiceType::BlendNetwork,
        locators: "/ip4/127.0.0.1/tcp/9100"
            .parse::<Locator>()
            .expect("Valid locator multiaddr")
            .into(),
        provider_id: lb_core::sdp::ProviderId::try_from(
            provider_signing_key.public_key().to_bytes(),
        )
        .expect("provider signing key should yield a provider id"),
        zk_id: provider_zk_key.to_public_key(),
        locked_note_id,
    };
    let declaration_id = declaration.id();

    let declare_hash = submit_sdp_declare(
        &node0,
        &genesis_utxos,
        &funding_wallet,
        &provider_signing_key,
        &provider_zk_key,
        &spare_note_secret_key,
        declaration,
    )
    .await;
    assert!(
        wait_for_transactions_inclusion(&node0, &[declare_hash], inclusion_timeout).await,
        "declare transaction should be included"
    );

    let declaration_state = wait_for_declaration(&node0, state_timeout, {
        let target_locked_note = locked_note_id;
        move |decl| decl.locked_note_id == target_locked_note
    })
    .await
    .expect("declaration should appear after submission");

    // Wait until we're past the lock period
    let wait_lock_period = (Epoch::new(1) + lock_period).into_inner() // +1 buffer
        * u32::try_from(slots_per_epoch).unwrap()
        * slot_duration;
    sleep(wait_lock_period).await;

    let withdraw_message = WithdrawMessage {
        declaration_id,
        locked_note_id,
        nonce: declaration_state.nonce + 1,
    };

    let (withdraw_mantle_tx, withdraw_signing_keys) = fund_sdp_transaction(
        &node0,
        &genesis_utxos,
        &funding_wallet,
        Op::SDPWithdraw(withdraw_message),
    )
    .await;

    let withdraw_hash = withdraw_mantle_tx.hash();
    let withdraw_zk_sig = ZkKey::multi_sign(
        &[spare_note_secret_key.clone(), provider_zk_key.clone()],
        &withdraw_hash.to_fr(),
    )
    .expect("SDP withdraw zk proof should build");

    let withdraw_transfer_proof = OpProof::ZkSig(
        ZkKey::multi_sign(&withdraw_signing_keys, &withdraw_hash.to_fr())
            .expect("transfer proof should build"),
    );

    let withdraw_tx = SignedMantleTx::new(
        withdraw_mantle_tx,
        vec![OpProof::ZkSig(withdraw_zk_sig), withdraw_transfer_proof],
    )
    .expect("funded SDP withdraw transaction should be valid");

    node0
        .submit_transaction(&withdraw_tx)
        .await
        .expect("submit withdraw transaction");

    assert!(
        wait_for_transactions_inclusion(&node0, &[withdraw_hash], inclusion_timeout).await,
        "withdraw transaction should be included"
    );

    let removed = wait_for_declaration_absence(&node0, locked_note_id, state_timeout).await;
    assert!(removed, "withdraw should remove the declaration");
}

/// Test that SDP declaration is correctly restored after validator restart.
///
/// This test verifies that after restart, the validator fetches its declaration
/// from the ledger and the SDP service correctly loads declaration state.
#[tokio::test]
#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in these integration tests; boxing would not improve readability"
)]
async fn sdp_declaration_restoration_e2e() {
    let (cluster_harness, node0_name, node0, ..) =
        start_sdp_manual_cluster("sdp-declaration-restoration").await;

    let declarations = node0
        .get_sdp_declarations()
        .await
        .expect("fetching SDP declarations should succeed");
    assert!(
        !declarations.is_empty(),
        "validators should have declarations from genesis"
    );

    let initial_declaration = declarations.first().unwrap().clone();
    let target_locked_note = initial_declaration.locked_note_id;

    cluster_harness
        .cluster()
        .restart_node(&node0_name)
        .await
        .expect("manual cluster node should restart successfully");

    sleep(Duration::from_secs(5)).await;

    let post_restart_declarations = cluster_harness
        .cluster()
        .node_client(&node0_name)
        .expect("restarted node client should be available")
        .get_sdp_declarations()
        .await
        .expect("fetching post-restart SDP declarations should succeed");
    assert!(
        !post_restart_declarations.is_empty(),
        "declarations should be visible after restart"
    );

    let restored_declaration = post_restart_declarations
        .iter()
        .find(|d| d.locked_note_id == target_locked_note)
        .expect("original declaration should still exist after restart");

    assert_eq!(
        restored_declaration.service_type, initial_declaration.service_type,
        "service type should be preserved after restart"
    );
    assert_eq!(
        restored_declaration.zk_id, initial_declaration.zk_id,
        "zk_id should be preserved after restart"
    );

    let logs = read_manual_node_logs(cluster_harness.scenario_base_dir(), &node0_name);
    assert!(
        logs.contains("Loaded declaration from ledger"),
        "SDP service should log that it loaded declaration from ledger"
    );
}

async fn wait_for_declaration<F>(
    node: &NodeHttpClient,
    duration: Duration,
    predicate: F,
) -> Option<Declaration>
where
    F: Fn(&Declaration) -> bool + Send + Sync + 'static,
{
    timeout(duration, async {
        loop {
            if let Ok(declarations) = node.get_sdp_declarations().await
                && let Some(declaration) = declarations.into_iter().find(|decl| predicate(decl))
            {
                break declaration;
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .ok()
}

async fn wait_for_declaration_absence(
    node: &NodeHttpClient,
    locked_note_id: NoteId,
    duration: Duration,
) -> bool {
    timeout(duration, async {
        loop {
            let present = node
                .get_sdp_declarations()
                .await
                .map_or(true, |declarations| {
                    declarations
                        .into_iter()
                        .any(|decl| decl.locked_note_id == locked_note_id)
                });

            if !present {
                break;
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .is_ok()
}

async fn wait_for_sdp_declarations(
    node: &NodeHttpClient,
    duration: Duration,
) -> Option<Vec<Declaration>> {
    timeout(duration, async {
        loop {
            if let Ok(declarations) = node.get_sdp_declarations().await {
                break declarations;
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .ok()
}

#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in this integration-test helper; boxing would not improve readability"
)]
async fn start_sdp_manual_cluster(
    test_name: &str,
) -> (
    LocalManualClusterHarnessBase,
    String,
    NodeHttpClient,
    Vec<Utxo>,
    WalletAccount,
    ZkKey,
    NoteId,
    NumberOfEpochs,
    u64,
    Duration,
) {
    let spare_wallet =
        WalletAccount::deterministic(1, 100, false).expect("spare locked-note wallet should build");
    let (
        cluster_harness,
        node0_name,
        node0_client,
        genesis_utxos,
        funding_wallet,
        slots,
        slot_duration,
    ) = start_sdp_cluster(test_name, std::slice::from_ref(&spare_wallet)).await;
    let spare_note_id = note_id_for(&genesis_utxos, &spare_wallet);
    (
        cluster_harness,
        node0_name,
        node0_client,
        genesis_utxos,
        funding_wallet,
        spare_wallet.secret_key,
        spare_note_id,
        LOCK_PERIOD,
        slots,
        slot_duration,
    )
}

/// Find the genesis note id owned by `wallet`.
fn note_id_for(genesis_utxos: &[Utxo], wallet: &WalletAccount) -> NoteId {
    genesis_utxos
        .iter()
        .find(|utxo| utxo.note.pk == wallet.public_key())
        .expect("wallet-backed note should exist at genesis")
        .id()
}

/// Start a single-node SDP manual cluster seeded with a funding wallet plus the
/// given `spare_wallets` (each backing one lockable genesis note). Returns the
/// harness, node-0 name/client, genesis UTXOs, the funding wallet, and the
/// epoch timing (`slots_per_epoch`, `slot_duration`).
#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in these integration tests; boxing would not improve readability"
)]
async fn start_sdp_cluster(
    test_name: &str,
    spare_wallets: &[WalletAccount],
) -> (
    LocalManualClusterHarnessBase,
    String,
    NodeHttpClient,
    Vec<Utxo>,
    WalletAccount,
    u64,
    Duration,
) {
    let slots_per_epoch = Arc::new(AtomicU64::new(0));
    let slot_duration = Arc::new(Mutex::new(Duration::ZERO));
    let funding_wallet =
        WalletAccount::deterministic(0, 2_000_000, false).expect("funding wallet should build");

    let mut wallets = vec![funding_wallet.clone()];
    wallets.extend(spare_wallets.iter().cloned());

    let cluster_harness = build_local_manual_cluster(
        test_name,
        "tf-sdp",
        DeploymentBuilder::new(TfTopologyConfig::with_node_numbers(1))
            .with_wallet_config(WalletConfig::new(wallets))
            .with_test_context(test_name),
        Some(PathBuf::from(E2E_ARTIFACTS_DIR)),
    );

    let node0 = cluster_harness
        .cluster()
        .start_node_with(
            "0",
            StartNodeOptions::default()
                .with_persist_dir(cluster_harness.scenario_base_dir().join("node-0"))
                .create_patch({
                    let slots_per_epoch = Arc::clone(&slots_per_epoch);
                    let slot_duration = Arc::clone(&slot_duration);
                    move |config| {
                        let config = patch_sdp_manual_cluster_config(config);
                        slots_per_epoch.store(
                            config.deployment.cryptarchia.slots_per_epoch(),
                            Ordering::Relaxed,
                        );
                        *slot_duration.lock().unwrap() = config.deployment.time.slot_duration;
                        Ok::<_, DynError>(config)
                    }
                }),
        )
        .await
        .expect("starting node-0 should succeed");

    cluster_harness
        .cluster()
        .wait_network_ready()
        .await
        .expect("manual cluster should become ready");

    wait_for_manual_cluster_height(&node0.client, 1, Duration::from_mins(2))
        .await
        .expect("node-0 should produce the first block");

    let genesis_block = cluster_harness
        .deployment()
        .config
        .genesis_block
        .clone()
        .expect("manual-cluster deployment should include genesis tx");
    let genesis_tx = genesis_block.genesis_tx();
    let genesis_utxos: Vec<_> = genesis_tx
        .genesis_transfer()
        .outputs
        .utxos(genesis_tx.genesis_transfer())
        .collect();

    (
        cluster_harness,
        node0.name,
        node0.client,
        genesis_utxos,
        funding_wallet,
        slots_per_epoch.load(Ordering::Relaxed),
        *slot_duration.lock().unwrap(),
    )
}

fn patch_sdp_manual_cluster_config(mut config: RunConfig) -> RunConfig {
    config.deployment.time.slot_duration = Duration::from_secs(1);
    config
        .user
        .cryptarchia
        .service
        .bootstrap
        .prolonged_bootstrap_period = Duration::ZERO;
    config.deployment.cryptarchia.security_param = NonZero::new(2).unwrap();
    config.deployment.cryptarchia.slot_activation_coeff =
        NonNegativeRatio::new(1, 2.try_into().unwrap());
    config.deployment.cryptarchia.epoch_config = EpochConfig {
        epoch_stake_distribution_stabilization: 1.try_into().unwrap(),
        epoch_period_nonce_buffer: 1.try_into().unwrap(),
        epoch_period_nonce_stabilization: 1.try_into().unwrap(),
    };
    config.deployment.cryptarchia.learning_rate = 0.5.try_into().unwrap();

    let service_params = config
        .deployment
        .cryptarchia
        .sdp_config
        .service_params
        .get_mut(&ServiceType::BlendNetwork)
        .expect("blend network params should exist");
    service_params.lock_period = LOCK_PERIOD;
    service_params.inactivity_period = 10.into();
    service_params.retention_period = 10.into();

    config.deployment.blend.common.num_blend_layers = 1.try_into().unwrap();
    config.deployment.blend.common.minimum_network_size = MinimumNetworkSize::try_new(2).unwrap();
    config
        .deployment
        .blend
        .core
        .scheduler
        .delayer
        .maximum_release_delay_in_rounds = 1.try_into().unwrap();

    config
}

async fn fund_sdp_transaction(
    node: &NodeHttpClient,
    genesis_utxos: &[Utxo],
    funding_wallet: &WalletAccount,
    extra_op: Op,
) -> (MantleTx, Vec<ZkKey>) {
    let funding_source = current_wallet_funding_source(node, genesis_utxos, funding_wallet.clone())
        .await
        .expect("funding wallet source should sync from chain");

    let empty_context = MantleTxGasContext::new(
        HashMap::new(),
        HashMap::new(),
        GasPrices {
            execution_base_gas_price: 0.into(),
            storage_gas_price: GENESIS_STORAGE_GAS_PRICE,
        },
    );
    let tx_context = lb_core::mantle::tx::MantleTxContext {
        gas_context: empty_context,
        leader_reward_amount: 0,
    };
    let tx_builder = MantleTxBuilder::new(tx_context)
        .push_op(extra_op)
        .expect("mixed-op helper should fit op bounds");

    let funded_builder = fund_builder_from_wallet_source(&funding_source, &tx_builder)
        .expect("funding mixed-op transaction should succeed");

    let signing_keys = funded_builder
        .ledger_inputs()
        .iter()
        .map(|_| funding_wallet.secret_key.clone())
        .collect::<Vec<_>>();

    (
        funded_builder
            .build()
            .expect("funded mixed-op builder should build"),
        signing_keys,
    )
}

/// AUDIT Finding 1 (High) — E2E REGRESSION TEST (fails until fixed): a Blend
/// node must survive an SDP snapshot that contains two declarations sharing a
/// `zk_id`.
///
/// Submits two SDP `Declare`s that share the same `zk_id` but differ in their
/// locators and locked notes (so distinct `DeclarationId`s — both spec-valid:
/// the SDP spec only requires `declaration_id` uniqueness, and the blend spec
/// models core membership as a *set*). Today, once both sit in the SDP
/// membership snapshot, `membership_info_from_epoch_state` builds the
/// core-membership Merkle tree over the colliding `zk_ids` and `.expect()`s
/// `MerkleTree::new_from_ordered(..) == Err(DuplicateKey)`; the panic hook
/// (`log_and_exit_hook`) logs the payload and `std::process::exit(1)`s, so the
/// node stops producing blocks.
///
/// This asserts the desired post-fix behavior — the node keeps producing
/// blocks and never logs the Merkle panic — so it FAILS today and passes once
/// `membership_info_from_epoch_state` dedupes duplicate `zk_ids` before
/// building the tree.
#[tokio::test]
#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in these integration tests; boxing would not improve readability"
)]
async fn blend_survives_duplicate_zk_id_declarations_e2e() {
    let spare1 = WalletAccount::deterministic(1, 100, false)
        .expect("spare locked-note wallet 1 should build");
    let spare2 = WalletAccount::deterministic(2, 100, false)
        .expect("spare locked-note wallet 2 should build");
    let (
        cluster_harness,
        node0_name,
        node0,
        genesis_utxos,
        funding_wallet,
        slots_per_epoch,
        slot_duration,
    ) = start_sdp_cluster("dup-zkid-blend-panic", &[spare1.clone(), spare2.clone()]).await;
    let spare1_note_id = note_id_for(&genesis_utxos, &spare1);
    let spare2_note_id = note_id_for(&genesis_utxos, &spare2);

    // One operator's keys, reused by BOTH declarations.
    let provider_signing_key = Ed25519Key::from_bytes(&[7u8; 32]);
    let provider_zk_key = ZkKey::from(BigUint::from(7u64));
    let zk_id = provider_zk_key.to_public_key();
    let provider_id =
        lb_core::sdp::ProviderId::try_from(provider_signing_key.public_key().to_bytes())
            .expect("provider signing key should yield a provider id");

    // Two declarations: SAME zk_id + provider_id, DIFFERENT locators + notes
    // (so distinct DeclarationIds).
    let declaration = |locator: &str, locked_note_id| DeclarationMessage {
        service_type: ServiceType::BlendNetwork,
        locators: locator.parse::<Locator>().expect("valid locator").into(),
        provider_id,
        zk_id,
        locked_note_id,
    };
    let declaration_a = declaration("/ip4/127.0.0.1/tcp/9101", spare1_note_id);
    let declaration_b = declaration("/ip4/127.0.0.1/tcp/9102", spare2_note_id);
    assert_ne!(declaration_a.id(), declaration_b.id());

    // Submit + include sequentially so the funding source for the second tx
    // reflects the first tx's spend (no double-spend of the funding note).
    // Inclusion implies the declaration was applied to SDP ledger state.
    for (declaration, spare_secret_key, label) in [
        (declaration_a, &spare1.secret_key, "first"),
        (
            declaration_b,
            &spare2.secret_key,
            "second (duplicate zk_id)",
        ),
    ] {
        let hash = submit_sdp_declare(
            &node0,
            &genesis_utxos,
            &funding_wallet,
            &provider_signing_key,
            &provider_zk_key,
            spare_secret_key,
            declaration,
        )
        .await;
        assert!(
            wait_for_transactions_inclusion(&node0, &[hash], Duration::from_mins(1)).await,
            "{label} declare should be included"
        );
    }

    let height_before = node0
        .consensus_info()
        .await
        .expect("node should still be healthy before the epoch boundary")
        .cryptarchia_info
        .height;

    // Wait across several epoch boundaries so a frozen SDP snapshot containing
    // BOTH declarations feeds the per-epoch membership build.
    let epoch_wall_time = u32::try_from(slots_per_epoch).unwrap() * slot_duration;
    sleep(epoch_wall_time * 5).await;

    // Desired (post-fix) behavior: the Blend membership build dedupes the
    // duplicate zk_ids instead of `.expect()`-panicking, so the node keeps
    // producing blocks across the epoch boundary. Fails today: the node panics
    // in the membership build and exits, so it never reaches this height.
    let advanced =
        wait_for_manual_cluster_height(&node0, height_before + 3, Duration::from_secs(30)).await;
    assert!(
        advanced.is_ok(),
        "node must keep producing blocks across the epoch boundary with \
         duplicate zk_ids on chain (it must not panic in the membership build); \
         height stuck near {height_before}"
    );

    // And it must not have logged the membership Merkle-tree panic.
    let logs = read_manual_node_logs(cluster_harness.scenario_base_dir(), &node0_name);
    assert!(
        !logs.contains("Should not fail to build Merkle tree"),
        "node must not panic building the membership Merkle tree over duplicate zk_ids"
    );
}

/// Build, sign and submit a single SDP `Declare` transaction. Returns the tx
/// hash. `locked_note_secret_key` is the secret key owning `locked_note_id`.
async fn submit_sdp_declare(
    node: &NodeHttpClient,
    genesis_utxos: &[Utxo],
    funding_wallet: &WalletAccount,
    provider_signing_key: &Ed25519Key,
    provider_zk_key: &ZkKey,
    locked_note_secret_key: &ZkKey,
    declaration: DeclarationMessage,
) -> lb_core::mantle::TxHash {
    let (mantle_tx, transfer_signing_keys) = fund_sdp_transaction(
        node,
        genesis_utxos,
        funding_wallet,
        Op::SDPDeclare(declaration),
    )
    .await;
    let hash = mantle_tx.hash();

    let ed25519_sig = Ed25519Signature::from_bytes(
        &provider_signing_key
            .sign_payload(hash.as_signing_bytes().as_ref())
            .to_bytes(),
    );
    let zk_sig = ZkKey::multi_sign(
        &[locked_note_secret_key.clone(), provider_zk_key.clone()],
        &hash.to_fr(),
    )
    .expect("SDP declare zk proof should build");
    let transfer_proof = OpProof::ZkSig(
        ZkKey::multi_sign(&transfer_signing_keys, &hash.to_fr())
            .expect("transfer proof should build"),
    );

    let tx = SignedMantleTx::new(
        mantle_tx,
        vec![
            OpProof::ZkAndEd25519Sigs {
                zk_sig,
                ed25519_sig,
            },
            transfer_proof,
        ],
    )
    .expect("funded SDP declare transaction should be valid");

    node.submit_transaction(&tx)
        .await
        .expect("submit declare transaction");

    hash
}

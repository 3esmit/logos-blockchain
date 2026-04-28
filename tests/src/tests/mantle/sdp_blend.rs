use std::{num::NonZero, time::Duration};

use lb_core::sdp::ServiceType;
use lb_cryptarchia_engine::{base_period_length, time::epoch_length};
use lb_node::config::{RunConfig, cryptarchia::deployment::EpochConfig};
use lb_testing_framework::{DeploymentBuilder, NodeHttpClient, TopologyConfig as TfTopologyConfig};
use lb_utils::math::NonNegativeRatio;
use logos_blockchain_tests::common::manual_cluster::{
    ManualNodeLayout, start_local_manual_cluster_with_layout, wait_for_nodes_height,
};
use testing_framework_core::scenario::DynError;
use tokio::time::sleep;
use tracing::Level;

/// Inactivity period in sessions. If no SDP_ACTIVE proof is submitted for this
/// many sessions, the declaration enters the inactive state.
const INACTIVITY_PERIOD: u64 = 1;

/// Retention period in sessions. After inactivity_period + retention_period
/// sessions without activity, the declaration is removed from the ledger.
const RETENTION_PERIOD: u64 = 1;

/// End-to-end test for blend SDP activity proofs:
///
/// 1. Spawn two validators with blend declarations in the genesis transaction.
/// 2. Wait long enough that declarations would be removed if no SDP_ACTIVE
///    activity proofs were submitted (inactivity_period + retention_period
///    sessions past genesis).
/// 3. Verify that both declarations are still present, proving that the nodes
///    automatically submitted valid activity proofs that the ledger accepted.
#[tokio::test]
#[expect(
    clippy::large_futures,
    reason = "Manual-cluster startup futures are large in these integration tests; boxing would not improve readability"
)]
async fn blend_sdp_activity_e2e() {
    let (_base, nodes) = start_local_manual_cluster_with_layout(
        "blend-sdp",
        "mantle-blend-sdp",
        DeploymentBuilder::new(
            TfTopologyConfig::with_node_numbers(2).with_test_context(Some("blend_sdp".to_owned())),
        ),
        2,
        ManualNodeLayout::SelectNodeSeed(0),
        |config| Ok::<_, DynError>(blend_sdp_test_config(config)),
    )
    .await;

    let node0 = &nodes[0];
    let node1 = &nodes[1];

    // Verify both nodes have blend declarations from genesis.
    let declarations = wait_for_declarations(&node0.client, Duration::from_secs(30)).await;
    assert_eq!(
        declarations.len(),
        2,
        "genesis should include blend declarations for both validators, got {}",
        declarations.len()
    );
    println!("declarations_before: {declarations:?}");

    // Wait past the point where declarations would be removed if no activity
    // proofs were submitted.
    //
    // session_duration = blocks_per_epoch = 6 blocks
    //
    // A declaration created at genesis with no activity would be removed
    // after (inactivity_period + retention_period) * session_duration
    // = (1 + 1) * 6 = 12 blocks.
    //
    // We wait for more sessions to give a solid margin and ensure that if
    // activity proofs are being submitted, they keep the declarations alive.
    let blocks_per_session = blend_sdp_blocks_per_session();
    let survival_sessions = INACTIVITY_PERIOD + RETENTION_PERIOD + 2;
    let target_height = survival_sessions * blocks_per_session + 2;
    println!(
        "Waiting for {target_height} blocks ({survival_sessions} sessions, {blocks_per_session} blocks/session) to ensure blend declarations would be removed without activity proofs",
    );

    wait_for_nodes_height(
        &[&node0.client, &node1.client],
        target_height,
        Duration::from_mins(60),
    )
    .await;

    // Declarations must still be present — this proves SDP_ACTIVE proofs were
    // submitted and accepted, keeping the declarations alive.
    let declarations_after = node0
        .client
        .get_sdp_declarations()
        .await
        .expect("querying SDP declarations should succeed");

    assert!(
        !declarations_after.is_empty(),
        "at least one blend declaration should survive past the inactivity window, \
         but none remain. activity proofs may not have been submitted"
    );
    println!("declarations_after: {declarations_after:?}");
}

fn blend_sdp_test_config(mut config: RunConfig) -> RunConfig {
    let (epoch_config, security_param, slot_activation_coeff) = cryptarchia_config();
    config.deployment.time.slot_duration = Duration::from_secs(1);
    config.deployment.cryptarchia.epoch_config = epoch_config;
    config.deployment.cryptarchia.security_param = security_param;
    config.deployment.cryptarchia.slot_activation_coeff = slot_activation_coeff;

    // Set small inactivity/retention periods so that declarations are removed
    // quickly if no activity proofs are submitted.
    let blend_params = config
        .deployment
        .cryptarchia
        .sdp_config
        .service_params
        .get_mut(&ServiceType::BlendNetwork)
        .expect("blend network params should exist");
    blend_params.inactivity_period = INACTIVITY_PERIOD;
    blend_params.retention_period = RETENTION_PERIOD;

    config.user.tracing.level = Level::DEBUG;

    config
}

fn blend_sdp_blocks_per_session() -> u64 {
    let (epoch_config, security_param, slot_activation_coeff) = cryptarchia_config();
    let base_period = base_period_length(security_param, slot_activation_coeff);
    let slots_per_epoch = epoch_length(
        epoch_config.epoch_stake_distribution_stabilization,
        epoch_config.epoch_period_nonce_buffer,
        epoch_config.epoch_period_nonce_stabilization,
        base_period,
    );

    let avg_slots_per_block =
        base_period_length(NonZero::<u32>::new(1).unwrap(), slot_activation_coeff).get();
    slots_per_epoch / avg_slots_per_block
}

fn cryptarchia_config() -> (EpochConfig, NonZero<u32>, NonNegativeRatio) {
    let epoch_config = EpochConfig {
        epoch_stake_distribution_stabilization: 1.try_into().unwrap(),
        epoch_period_nonce_buffer: 1.try_into().unwrap(),
        epoch_period_nonce_stabilization: 1.try_into().unwrap(),
    };
    let security_param = NonZero::new(2).unwrap();
    let slot_activation_coeff = NonNegativeRatio::new(1, 2.try_into().unwrap());
    (epoch_config, security_param, slot_activation_coeff)
}

async fn wait_for_declarations(
    node: &NodeHttpClient,
    duration: Duration,
) -> Vec<lb_core::sdp::Declaration> {
    tokio::time::timeout(duration, async {
        loop {
            if let Ok(declarations) = node.get_sdp_declarations().await {
                if !declarations.is_empty() {
                    break declarations;
                }
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("SDP declarations should become available")
}

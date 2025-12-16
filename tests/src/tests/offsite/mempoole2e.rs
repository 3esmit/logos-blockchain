use std::time::Duration;

use common_http_client::CommonHttpClient;
use key_management_system_service::keys::Ed25519Key;
use nomos_core::mantle::ops::{
    Op,
    channel::{ChannelId, MsgId},
};
use serial_test::serial;
use tests::{
    adjust_timeout,
    common::mantle_tx::create_channel_inscribe_tx,
    nodes::{executor::Executor, validator::Validator},
    topology::{Topology, TopologyConfig},
};
use tokio::time::{sleep, timeout};

fn test_signing_key() -> Ed25519Key {
    Ed25519Key::from_bytes(&[42u8; 32])
}

fn test_channel_id() -> ChannelId {
    ChannelId::from([1u8; 32])
}

struct ExpectedInscription {
    channel_id: ChannelId,
    inscription: Vec<u8>,
    parent: MsgId,
    label: &'static str,
}

async fn wait_for_inscriptions(
    executor: &Executor,
    inscriptions: Vec<ExpectedInscription>,
    duration: Duration,
) -> Result<(), &'static str> {
    use std::collections::HashSet;

    use nomos_core::header::HeaderId;

    timeout(duration, async {
        let mut remaining: Vec<_> = inscriptions.into_iter().collect();
        let mut checked_blocks: HashSet<HeaderId> = HashSet::new();

        while !remaining.is_empty() {
            let info = executor.consensus_info().await;
            let mut current_id = Some(info.tip);

            // Walk back from tip, checking any blocks we haven't seen yet
            while let Some(block_id) = current_id {
                if checked_blocks.contains(&block_id) {
                    break; // Already checked this block and its ancestors
                }

                if let Some(block) = executor.get_block(block_id).await {
                    checked_blocks.insert(block_id);

                    for tx in block.transactions() {
                        for op in &tx.mantle_tx.ops {
                            if let Op::ChannelInscribe(inscribe) = op {
                                remaining.retain(|expected| {
                                    let found = inscribe.channel_id == expected.channel_id
                                        && inscribe.inscription == expected.inscription
                                        && inscribe.parent == expected.parent;
                                    if found {
                                        println!("Found {} inscription", expected.label);
                                    }
                                    !found
                                });
                            }
                        }
                    }

                    current_id = Some(block.header().parent());
                } else {
                    break;
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| "Timed out waiting for inscriptions")
}

// Create 200KB data for each transaction
const DATA_SIZE: usize = 200 * 1024; // 200KB

#[tokio::test]
#[serial]
async fn inscription_ops_e2e() {
    let topology = Topology::spawn(TopologyConfig::validator_and_executor()).await;
    topology.wait_network_ready().await;
    let validator = &topology.validators()[0];
    let executor = &topology.executors()[0];

    topology.wait_network_ready().await;

    // Wait for validator to produce the first block
    wait_for_validator_height(validator, 1, adjust_timeout(Duration::from_secs(30)))
        .await
        .expect("validator should produce the first block");

    // Wait for executor to sync
    wait_for_executor_height(executor, 1, adjust_timeout(Duration::from_secs(30)))
        .await
        .expect("executor should sync the first block");

    let validator_url = validator.url();
    let client = CommonHttpClient::new(None);
    let signing_key = test_signing_key();
    let channel_id = test_channel_id();

    // Build all transactions upfront
    let mut genesis_data = vec![0u8; DATA_SIZE];
    genesis_data[..26].copy_from_slice(b"sequencer block 0: genesis");
    let genesis_tx = create_channel_inscribe_tx(
        &signing_key,
        channel_id,
        genesis_data.clone(),
        MsgId::root(),
    );

    let genesis_msg_id = match &genesis_tx.mantle_tx.ops[0] {
        Op::ChannelInscribe(op) => op.id(),
        _ => panic!("Expected ChannelInscribe op"),
    };

    let mut block1_data = vec![0u8; DATA_SIZE];
    block1_data[..34].copy_from_slice(b"sequencer block 1: the hobbit book");
    let block1_tx = create_channel_inscribe_tx(
        &signing_key,
        channel_id,
        block1_data.clone(),
        genesis_msg_id,
    );

    let block1_msg_id = match &block1_tx.mantle_tx.ops[0] {
        Op::ChannelInscribe(op) => op.id(),
        _ => panic!("Expected ChannelInscribe op"),
    };

    let mut block2_data = vec![0u8; DATA_SIZE];
    block2_data[..38].copy_from_slice(b"sequencer block 2: lord of the rings 1");
    let block2_tx =
        create_channel_inscribe_tx(&signing_key, channel_id, block2_data.clone(), block1_msg_id);

    client
        .post_transaction(validator_url.clone(), genesis_tx)
        .await
        .expect("Failed to submit genesis inscription");

    wait_for_inscriptions(
        executor,
        vec![ExpectedInscription {
            channel_id,
            inscription: genesis_data.clone(),
            parent: MsgId::root(),
            label: "genesis",
        }],
        adjust_timeout(Duration::from_secs(120)),
    )
    .await
    .expect("Genesis inscription should be found on executor");

    client
        .post_transaction(validator_url.clone(), block1_tx)
        .await
        .expect("Failed to submit block 1 inscription");

    wait_for_inscriptions(
        executor,
        vec![ExpectedInscription {
            channel_id,
            inscription: block1_data.clone(),
            parent: genesis_msg_id,
            label: "block 1",
        }],
        adjust_timeout(Duration::from_secs(120)),
    )
    .await
    .expect("Block 1 inscription should be found on executor");

    client
        .post_transaction(validator_url.clone(), block2_tx)
        .await
        .expect("Failed to submit block 2 inscription");

    wait_for_inscriptions(
        executor,
        vec![ExpectedInscription {
            channel_id,
            inscription: block2_data,
            parent: block1_msg_id,
            label: "block 2",
        }],
        adjust_timeout(Duration::from_secs(120)),
    )
    .await
    .expect("Block 2 inscription should be found on executor");
}

async fn wait_for_validator_height(
    validator: &Validator,
    target_height: u64,
    duration: Duration,
) -> Option<()> {
    timeout(duration, async {
        loop {
            let info = validator.consensus_info().await;
            if info.height >= target_height {
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .ok()
}

async fn wait_for_executor_height(
    executor: &Executor,
    target_height: u64,
    duration: Duration,
) -> Option<()> {
    timeout(duration, async {
        loop {
            let info = executor.consensus_info().await;
            if info.height >= target_height {
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .ok()
}

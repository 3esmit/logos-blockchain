use std::{collections::HashMap, num::NonZero, pin::pin, time::Duration};

use futures::{StreamExt as _, future::join_all};
use lb_core::mantle::ops::channel::ChannelId;
use lb_key_management_system_service::keys::Ed25519Key;
use lb_zone_sdk::{
    BlockStatus,
    indexer::{Cursor, ZoneIndexer},
    sequencer::{SequencerConfig, ZoneSequencer},
};
use logos_blockchain_tests::{
    nodes::{Validator, create_validator_config},
    topology::configs::{
        create_general_configs, deployment::e2e_deployment_settings_with_genesis_tx,
    },
};
use rand::{Rng as _, thread_rng};
use serial_test::serial;
use tokio::time::{sleep, timeout};

fn channel_id_from_key(key: &Ed25519Key) -> ChannelId {
    ChannelId::from(key.public_key().to_bytes())
}

async fn wait_for_height(validator: &Validator, target_height: u64, duration: Duration) -> bool {
    timeout(duration, async {
        loop {
            let info = validator.consensus_info(false).await;
            if info.height >= target_height {
                return;
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
#[serial]
async fn test_sequencer_publish_and_indexer_read() {
    // Use custom config with faster block production for test reliability:
    // - slot_duration: 1s (faster slots)
    // - security_param (k): 5 (fewer blocks needed for LIB to advance)
    let (configs, genesis_tx) = create_general_configs(2);
    let deployment_settings = e2e_deployment_settings_with_genesis_tx(genesis_tx);
    let configs: Vec<_> = configs
        .into_iter()
        .map(|c| {
            let mut config = create_validator_config(c, deployment_settings.clone());
            config.deployment.time.slot_duration = Duration::from_secs(1);
            config
                .user
                .cryptarchia
                .service
                .bootstrap
                .prolonged_bootstrap_period = Duration::ZERO;
            config.deployment.cryptarchia.security_param = NonZero::new(5).unwrap();
            config
        })
        .collect();

    let validators: Vec<Validator> = join_all(configs.into_iter().map(Validator::spawn))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to spawn validators");

    let validator = &validators[0];

    // Wait for the chain to produce at least one block.
    // Use generous timeout since leader election is probabilistic.
    assert!(
        wait_for_height(validator, 1, Duration::from_secs(120)).await,
        "Chain should produce the first block"
    );
    let node_url = validator.url();

    // Random signing key per test run to avoid channel collisions
    let mut key_bytes = [0u8; 32];
    thread_rng().fill(&mut key_bytes);
    let signing_key = Ed25519Key::from_bytes(&key_bytes);
    let channel_id = channel_id_from_key(&signing_key);

    // Create indexer BEFORE publishing so we can catch messages as Safe
    let indexer = ZoneIndexer::new(channel_id, node_url.clone(), None);

    // Start follow() stream BEFORE publishing - this way we'll see messages
    // arrive as Safe (from live stream) rather than Finalized (from backfill)
    let stream = indexer.follow(None).await.expect("follow should succeed");
    let mut stream = pin!(stream);

    // Use short resubmit interval matching fast block production (1s slots).
    // Default 30s is too slow - if a tx gets orphaned, we miss many opportunities.
    let sequencer_config = SequencerConfig {
        resubmit_interval: Duration::from_secs(3),
        ..SequencerConfig::default()
    };
    let sequencer = ZoneSequencer::init_with_config(
        channel_id,
        signing_key,
        node_url.clone(),
        None,
        sequencer_config,
        None, // Fresh start, no checkpoint
    );

    // Publish inscriptions (with retry until sequencer is initialized)
    let test_data: Vec<Vec<u8>> = vec![
        b"Hello, Zone!".to_vec(),
        b"Second message".to_vec(),
        b"Third message".to_vec(),
    ];

    let publish_start = std::time::Instant::now();
    let publish_timeout = Duration::from_secs(30);

    for data in &test_data {
        loop {
            assert!(
                publish_start.elapsed() <= publish_timeout,
                "Timeout waiting for sequencer to initialize"
            );

            match sequencer.publish(data.clone()).await {
                Ok(_) => break,
                Err(_) => {
                    // Sequencer not ready yet, wait and retry
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    // === Receive messages via follow() stream ===
    // Messages follow Safe → Finalized lifecycle:
    // 1. Arrive as Safe (above LIB) - may happen multiple times during reorgs
    // 2. Later arrive as Finalized (when LIB advances past them)
    //
    // Since we start follow() BEFORE publishing, we're guaranteed to see Safe
    // events. (The "Finalized without Safe" edge case only applies when
    // consumer misses live events)
    let mut seen_safe: HashMap<Vec<u8>, u32> = HashMap::new(); // count Safe events
    let mut seen_finalized: HashMap<Vec<u8>, bool> = HashMap::new();
    let mut last_cursor = None;

    let start = std::time::Instant::now();
    let stream_timeout = Duration::from_secs(180);

    // Wait until all messages are Finalized
    while start.elapsed() < stream_timeout {
        let all_finalized = test_data
            .iter()
            .all(|data| seen_finalized.get(data) == Some(&true));
        if all_finalized {
            break;
        }

        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(msg)) => {
                last_cursor = Some(msg.cursor);

                if test_data.contains(&msg.block.data) {
                    match msg.status {
                        BlockStatus::Safe => {
                            // Safe can arrive multiple times (reorgs) - that's OK
                            *seen_safe.entry(msg.block.data.clone()).or_insert(0) += 1;
                        }
                        BlockStatus::Finalized => {
                            seen_finalized.insert(msg.block.data.clone(), true);
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {} // Timeout on stream.next(), keep trying
        }
    }

    // Verify Safe → Finalized lifecycle for all messages.
    // We started following before publishing, so we must see both.
    for data in &test_data {
        assert!(
            seen_safe.get(data).is_some_and(|&count| count >= 1),
            "Message should have been Safe at least once: {:?}",
            String::from_utf8_lossy(data)
        );
        assert_eq!(
            seen_finalized.get(data),
            Some(&true),
            "Message should be Finalized: {:?}",
            String::from_utf8_lossy(data)
        );
    }

    // === Test cursor resumption ===
    // Save cursor and resume - verify we receive new messages
    let saved_cursor = last_cursor.expect("Should have cursor after receiving messages");
    // Stream goes out of scope here (pin! creates a local binding)

    // Publish one more message
    let new_msg = b"Fourth message after cursor".to_vec();
    loop {
        match sequencer.publish(new_msg.clone()).await {
            Ok(_) => break,
            Err(_) => sleep(Duration::from_millis(500)).await,
        }
    }

    // Resume from saved cursor
    let resumed_stream = indexer
        .follow(Some(saved_cursor))
        .await
        .expect("follow with cursor should succeed");
    let mut resumed_stream = pin!(resumed_stream);

    let mut safe_count = 0u32;
    let mut found_new_msg_finalized = false;
    let resume_timeout = Duration::from_secs(180);
    let resume_start = std::time::Instant::now();

    // Wait for the new message to reach Finalized
    while resume_start.elapsed() < resume_timeout && !found_new_msg_finalized {
        match timeout(Duration::from_millis(500), resumed_stream.next()).await {
            Ok(Some(msg)) => {
                if msg.block.data == new_msg {
                    match msg.status {
                        BlockStatus::Safe => {
                            // Safe can arrive multiple times (reorgs) - that's OK
                            safe_count += 1;
                        }
                        BlockStatus::Finalized => {
                            // Finalized is the hard invariant we're testing
                            found_new_msg_finalized = true;
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }

    // We published after saving cursor and resumed immediately - should see Safe
    // then Finalized
    assert!(
        safe_count >= 1,
        "New message should have been Safe at least once"
    );
    assert!(
        found_new_msg_finalized,
        "New message should reach Finalized status after cursor resume"
    );
}

#[tokio::test]
#[serial]
async fn test_sequencer_checkpoint_resume() {
    // Setup network with faster block production
    let (configs, genesis_tx) = create_general_configs(2);
    let deployment_settings = e2e_deployment_settings_with_genesis_tx(genesis_tx);
    let configs: Vec<_> = configs
        .into_iter()
        .map(|c| {
            let mut config = create_validator_config(c, deployment_settings.clone());
            config.deployment.time.slot_duration = Duration::from_secs(1);
            config
                .user
                .cryptarchia
                .service
                .bootstrap
                .prolonged_bootstrap_period = Duration::ZERO;
            config.deployment.cryptarchia.security_param = NonZero::new(5).unwrap();
            config
        })
        .collect();

    let validators: Vec<Validator> = join_all(configs.into_iter().map(Validator::spawn))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to spawn validators");

    let validator = &validators[0];

    assert!(
        wait_for_height(validator, 1, Duration::from_secs(120)).await,
        "Chain should produce the first block"
    );
    let node_url = validator.url();

    // Random signing key per test run
    let mut key_bytes = [0u8; 32];
    thread_rng().fill(&mut key_bytes);
    let signing_key = Ed25519Key::from_bytes(&key_bytes);
    let channel_id = channel_id_from_key(&signing_key);

    let sequencer_config = SequencerConfig {
        resubmit_interval: Duration::from_secs(3),
        ..SequencerConfig::default()
    };

    // Phase 1: Start fresh sequencer and publish messages
    let sequencer = ZoneSequencer::init_with_config(
        channel_id,
        signing_key.clone(),
        node_url.clone(),
        None,
        sequencer_config.clone(),
        None, // Fresh start
    );

    let test_data_phase1: Vec<Vec<u8>> = vec![b"Message 1".to_vec(), b"Message 2".to_vec()];

    let publish_timeout = Duration::from_secs(30);
    let publish_start = std::time::Instant::now();
    let mut first_checkpoint = None;
    let mut last_checkpoint = None;

    for data in &test_data_phase1 {
        loop {
            assert!(
                publish_start.elapsed() <= publish_timeout,
                "Timeout waiting for sequencer to initialize"
            );

            match sequencer.publish(data.clone()).await {
                Ok(result) => {
                    // Save first checkpoint for indexer cursor
                    if first_checkpoint.is_none() {
                        first_checkpoint = Some(result.checkpoint.clone());
                    }
                    last_checkpoint = Some(result.checkpoint);
                    break;
                }
                Err(_) => {
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    // Get checkpoints
    let first_checkpoint = first_checkpoint.expect("Should have checkpoint after first publish");
    let checkpoint = last_checkpoint.expect("Should have checkpoint after publishing");

    // Start indexer with cursor from first publish's checkpoint.
    // This tests both: sequencer checkpoint resume AND indexer cursor backfill.
    let indexer_cursor = Cursor {
        slot: first_checkpoint.lib_slot.into(),
        last_id: None, // Start from beginning of this slot
    };
    let indexer = ZoneIndexer::new(channel_id, node_url.clone(), None);
    let stream = indexer
        .follow(Some(indexer_cursor))
        .await
        .expect("follow should succeed");
    let mut stream = pin!(stream);

    // Drop the old sequencer (simulating stop)
    drop(sequencer);

    // Phase 2: Resume with checkpoint and publish more messages
    let sequencer = ZoneSequencer::init_with_config(
        channel_id,
        signing_key,
        node_url.clone(),
        None,
        sequencer_config,
        Some(checkpoint), // Resume from checkpoint
    );

    let test_data_phase2: Vec<Vec<u8>> = vec![b"Message 3".to_vec(), b"Message 4".to_vec()];

    let publish_start = std::time::Instant::now();
    for data in &test_data_phase2 {
        loop {
            assert!(
                publish_start.elapsed() <= publish_timeout,
                "Timeout waiting for sequencer to initialize"
            );

            match sequencer.publish(data.clone()).await {
                Ok(_) => break,
                Err(_) => {
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    // Collect all test data
    let all_test_data: Vec<Vec<u8>> = test_data_phase1
        .into_iter()
        .chain(test_data_phase2)
        .collect();

    // Track Safe (count, since reorgs can cause multiple) and Finalized status
    let mut seen_safe: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut seen_finalized: HashMap<Vec<u8>, bool> = HashMap::new();

    let start = std::time::Instant::now();
    let stream_timeout = Duration::from_secs(180);

    // Wait for all messages to be Finalized
    while start.elapsed() < stream_timeout {
        let all_finalized = all_test_data
            .iter()
            .all(|data| seen_finalized.get(data) == Some(&true));
        if all_finalized {
            break;
        }

        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(msg)) => {
                if all_test_data.contains(&msg.block.data) {
                    match msg.status {
                        BlockStatus::Safe => {
                            *seen_safe.entry(msg.block.data.clone()).or_insert(0) += 1;
                        }
                        BlockStatus::Finalized => {
                            seen_finalized.insert(msg.block.data.clone(), true);
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }

    // All messages must go through Safe → Finalized lifecycle.
    // Safe can appear multiple times (reorgs), Finalized tests sequencer resubmit.
    for data in &all_test_data {
        assert!(
            seen_safe.get(data).is_some_and(|&count| count >= 1),
            "Message should have been Safe at least once: {:?}",
            String::from_utf8_lossy(data)
        );
        assert_eq!(
            seen_finalized.get(data),
            Some(&true),
            "Message should be Finalized: {:?}",
            String::from_utf8_lossy(data)
        );
    }
}

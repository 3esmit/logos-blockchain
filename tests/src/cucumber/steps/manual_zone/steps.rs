use std::{collections::HashMap, time::Duration};

use cucumber::{gherkin::Step, given, when};
use lb_common_http_client::CommonHttpClient;
use lb_core::mantle::{gas::GasCost, ops::channel::deposit::DepositOp};
use lb_http_api_common::bodies::channel::{ChannelDepositRequestBody, ChannelDepositResponseBody};
use lb_zone_sdk::{
    adapter::NodeHttpClient as ZoneNodeHttpClient,
    indexer::ZoneIndexer,
    sequencer::{InscriptionId, SequencerCheckpoint, ZoneSequencer},
};
use tracing::{info, warn};

use crate::{
    common::{
        manual_cluster::wait_for_height,
        zone::{
            ZoneTestError, build_zone_cluster, collect_indexed_messages,
            collect_indexed_messages_exactly_once, ensure_zone_transactions_included,
            publish_message_with_retry, random_second_public_key, sequencer_config,
            start_zone_node, wait_for_deposit, wait_for_transactions_finalized,
            wait_for_zone_network_ready,
        },
    },
    cucumber::{
        error::{StepError, StepResult},
        steps::TARGET,
        world::{CucumberWorld, NodeInfo},
    },
};

struct PublishedZoneMessage {
    alias: String,
    payload: Vec<u8>,
    inscription_id: InscriptionId,
    checkpoint: SequencerCheckpoint,
}

#[given("I have a zone cluster")]
async fn step_zone_cluster(world: &mut CucumberWorld, step: &Step) -> StepResult {
    let zone_cluster = build_zone_cluster(world.scenario_base_dir.clone()).map_err(|error| {
        log_zone_error(step, &error);

        StepError::LogicalError {
            message: error.to_string(),
        }
    })?;

    let channel_signing_key = zone_cluster.channel_signing_key.clone();
    let funding_public_key = zone_cluster.funding_public_key;
    let cluster = zone_cluster.cluster;

    let started_zone_node = start_zone_node(&cluster, &world.scenario_base_dir)
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    wait_for_zone_network_ready(&cluster)
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    let node_name = "NODE_1".to_owned();
    let client = started_zone_node.started_node.client.clone();

    world.local_cluster = Some(cluster);

    world.nodes_info.insert(
        node_name.clone(),
        NodeInfo {
            name: node_name.clone(),
            started_node: started_zone_node.started_node,
            run_config: None,
            chain_info: HashMap::default(),
            wallet_info: HashMap::default(),
            runtime_dir: started_zone_node.runtime_dir,
        },
    );

    world
        .zone
        .initialize_cluster(node_name, channel_signing_key, funding_public_key);

    info!(target: TARGET, node_url = %client.base_url(), "Started zone cluster");

    Ok(())
}

#[cucumber::when(expr = "the zone node is at height {int} in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_node_is_at_height(
    world: &mut CucumberWorld,
    step: &Step,
    height: u64,
    timeout_seconds: u64,
) -> StepResult {
    let client = world.zone_node_http_client().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    wait_for_height(&client, height, Duration::from_secs(timeout_seconds))
        .await
        .map_err(|_| StepError::Timeout {
            message: format!(
                "Zone node did not reach height {height} in {timeout_seconds} seconds"
            ),
        })
}

#[given("a zone sequencer is initialized")]
#[when("a zone sequencer is initialized")]
async fn step_zone_sequencer_initialized(world: &mut CucumberWorld, step: &Step) -> StepResult {
    let channel_id = world.zone.channel_id().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let signing_key = world.zone.channel_signing_key().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let (sequencer, mut handle) = ZoneSequencer::init_with_config(
        channel_id,
        signing_key.clone(),
        ZoneNodeHttpClient::new(CommonHttpClient::new(None), node_url),
        sequencer_config(),
        None,
    );

    let task = sequencer.spawn();

    handle.wait_ready().await;

    world.zone.set_sequencer(handle, task);

    Ok(())
}

#[given("a zone indexer is initialized")]
#[when("a zone indexer is initialized")]
fn step_zone_indexer_initialized(world: &mut CucumberWorld, step: &Step) -> StepResult {
    let channel_id = world.zone.channel_id().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let indexer = ZoneIndexer::new(
        channel_id,
        ZoneNodeHttpClient::new(CommonHttpClient::new(None), node_url),
    );

    world.zone.set_indexer(indexer);

    Ok(())
}

#[when("I publish the following zone messages:")]
async fn step_publish_zone_messages(world: &mut CucumberWorld, step: &Step) -> StepResult {
    let rows = zone_message_rows(step)?;

    let node = world.zone_node_http_client().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let published = {
        let sequencer = world.zone.sequencer_handle().inspect_err(|e| {
            warn!(target: TARGET, "Step `{}` error: {e}", step.value);
        })?;

        let publish_start = std::time::Instant::now();
        let publish_timeout = Duration::from_secs(180);
        let mut published = Vec::with_capacity(rows.len());

        for (alias, payload) in &rows {
            let result =
                publish_message_with_retry(sequencer, payload, publish_start, publish_timeout)
                    .await
                    .map_err(|error| {
                        log_zone_error(step, &error);

                        StepError::LogicalError {
                            message: error.to_string(),
                        }
                    })?;

            ensure_zone_transactions_included(
                &node,
                &[result.inscription_id],
                Duration::from_secs(180),
            )
            .await
            .map_err(|error| {
                log_zone_error(step, &error);

                StepError::LogicalError {
                    message: error.to_string(),
                }
            })?;

            published.push(PublishedZoneMessage {
                alias: alias.clone(),
                payload: payload.clone(),
                inscription_id: result.inscription_id,
                checkpoint: result.checkpoint,
            });
        }

        published
    };

    for message in published {
        world.zone.remember_published_message(
            message.alias,
            message.payload,
            message.inscription_id,
            message.checkpoint,
        );
    }

    Ok(())
}

#[when(expr = "I save the current zone sequencer checkpoint as {string}")]
fn step_save_zone_checkpoint(
    world: &mut CucumberWorld,
    step: &Step,
    checkpoint_alias: String,
) -> StepResult {
    let checkpoint = world.zone.current_checkpoint().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    world.zone.remember_checkpoint(checkpoint_alias, checkpoint);

    Ok(())
}

#[when(expr = "I restart the zone sequencer from checkpoint {string}")]
async fn step_restart_zone_sequencer(
    world: &mut CucumberWorld,
    step: &Step,
    checkpoint_alias: String,
) -> StepResult {
    let checkpoint = world.zone.resolve_checkpoint(checkpoint_alias)?;

    let channel_id = world.zone.channel_id().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let signing_key = world.zone.channel_signing_key().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let (sequencer, mut handle) = ZoneSequencer::init_with_config(
        channel_id,
        signing_key.clone(),
        ZoneNodeHttpClient::new(CommonHttpClient::new(None), node_url),
        sequencer_config(),
        Some(checkpoint),
    );

    let task = sequencer.spawn();

    handle.wait_ready().await;

    world.zone.set_sequencer(handle, task);

    Ok(())
}

#[when("I restart the zone sequencer fresh")]
async fn step_restart_zone_sequencer_fresh(world: &mut CucumberWorld, step: &Step) -> StepResult {
    let channel_id = world.zone.channel_id().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let signing_key = world.zone.channel_signing_key().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let (sequencer, mut handle) = ZoneSequencer::init_with_config(
        channel_id,
        signing_key.clone(),
        ZoneNodeHttpClient::new(CommonHttpClient::new(None), node_url),
        sequencer_config(),
        None,
    );

    let task = sequencer.spawn();

    handle.wait_ready().await;

    world.zone.set_sequencer(handle, task);

    Ok(())
}

#[when(expr = "I submit zone set keys transaction {string}")]
async fn step_submit_zone_set_keys_transaction(
    world: &mut CucumberWorld,
    step: &Step,
    transaction_alias: String,
) -> StepResult {
    let second_key = random_second_public_key();

    let result = {
        let sequencer = world.zone.sequencer_handle().inspect_err(|e| {
            warn!(target: TARGET, "Step `{}` error: {e}", step.value);
        })?;

        let admin_signing_key = world.zone.channel_signing_key().inspect_err(|e| {
            warn!(target: TARGET, "Step `{}` error: {e}", step.value);
        })?;

        let admin_key = admin_signing_key.public_key();

        let (result, finalized) = sequencer
            .set_keys(vec![admin_key, second_key])
            .await
            .map_err(|error| StepError::LogicalError {
                message: format!("Zone set_keys failed: {error}"),
            })?;

        drop(finalized);

        result
    };

    world.zone.remember_checkpoint(
        format!("{transaction_alias}_CHECKPOINT"),
        result.checkpoint.clone(),
    );

    let tx_hash = result.inscription_id;

    world.remember_submitted_transaction(transaction_alias, tx_hash);

    Ok(())
}

#[when(expr = "I submit zone deposit transaction {string} of {int} with metadata {string}")]
async fn step_submit_zone_deposit_transaction(
    world: &mut CucumberWorld,
    step: &Step,
    transaction_alias: String,
    amount: u64,
    metadata: String,
) -> StepResult {
    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let channel_id = world.zone.channel_id().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let funding_public_key = world.zone.funding_public_key().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let deposit = DepositOp {
        channel_id,
        amount,
        metadata: metadata.into_bytes(),
    };

    let body = ChannelDepositRequestBody {
        tip: None,
        deposit: deposit.clone(),
        change_public_key: funding_public_key,
        funding_public_keys: vec![funding_public_key],
        max_tx_fee: GasCost::new(10),
    };

    let request_url = node_url
        .join("/channel/deposit")
        .map_err(|e| StepError::LogicalError {
            message: format!("Failed to build channel deposit URL: {e}"),
        })?;

    let response: ChannelDepositResponseBody = CommonHttpClient::new(None)
        .post(request_url, &body)
        .await
        .map_err(|error| StepError::LogicalError {
            message: format!("Zone channel deposit failed: {error}"),
        })?;

    world
        .zone
        .remember_submitted_deposit(transaction_alias.clone(), deposit);
    world.remember_submitted_transaction(transaction_alias, response.hash);

    Ok(())
}

#[cucumber::then(expr = "all zone messages are safe in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_all_zone_messages_are_safe(
    world: &mut CucumberWorld,
    step: &Step,
    timeout_seconds: u64,
) -> StepResult {
    let inscription_ids = world.zone.ordered_inscription_ids().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    if !world.zone.has_published_messages() {
        return Err(StepError::LogicalError {
            message: "No zone messages have been published".to_owned(),
        });
    }

    let node = world.zone_node_http_client().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    ensure_zone_transactions_included(
        &node,
        &inscription_ids,
        Duration::from_secs(timeout_seconds),
    )
    .await
    .map_err(|error| {
        log_zone_error(step, &error);

        StepError::LogicalError {
            message: error.to_string(),
        }
    })?;

    Ok(())
}

#[cucumber::then(expr = "all zone messages are finalized in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_all_zone_messages_are_finalized(
    world: &mut CucumberWorld,
    step: &Step,
    timeout_seconds: u64,
) -> StepResult {
    let inscription_ids = world.zone.ordered_inscription_ids().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    if !world.zone.has_published_messages() {
        return Err(StepError::LogicalError {
            message: "No zone messages have been published".to_owned(),
        });
    }

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    wait_for_transactions_finalized(
        node_url,
        &inscription_ids,
        Duration::from_secs(timeout_seconds),
    )
    .await
    .map_err(|error| {
        log_zone_error(step, &error);

        StepError::LogicalError {
            message: error.to_string(),
        }
    })?;

    Ok(())
}

#[cucumber::then("the zone indexer returns messages in this order:")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_indexer_returns_messages_in_order(
    world: &mut CucumberWorld,
    step: &Step,
) -> StepResult {
    let aliases = zone_message_aliases(step)?;

    let expected = world
        .zone
        .message_payloads_for_aliases(&aliases)
        .inspect_err(|e| {
            warn!(target: TARGET, "Step `{}` error: {e}", step.value);
        })?;

    let indexer = world.zone.indexer().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let actual = collect_indexed_messages(indexer, &expected, Duration::from_secs(180))
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    if actual == expected {
        return Ok(());
    }

    Err(StepError::LogicalError {
        message: format!(
            "Zone indexer returned messages in unexpected order: expected {} messages, got {}",
            expected.len(),
            actual.len()
        ),
    })
}

#[cucumber::then("the zone indexer returns each of these messages exactly once in this order:")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_indexer_returns_messages_exactly_once_in_order(
    world: &mut CucumberWorld,
    step: &Step,
) -> StepResult {
    let aliases = zone_message_aliases(step)?;

    let expected = world
        .zone
        .message_payloads_for_aliases(&aliases)
        .inspect_err(|e| {
            warn!(target: TARGET, "Step `{}` error: {e}", step.value);
        })?;

    let indexer = world.zone.indexer().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    let actual =
        collect_indexed_messages_exactly_once(indexer, &expected, Duration::from_secs(180))
            .await
            .map_err(|error| {
                log_zone_error(step, &error);

                StepError::LogicalError {
                    message: error.to_string(),
                }
            })?;

    if actual == expected {
        return Ok(());
    }

    Err(StepError::LogicalError {
        message: format!(
            "Zone indexer returned duplicate or out-of-order messages: expected {} messages, got {}",
            expected.len(),
            actual.len()
        ),
    })
}

#[cucumber::then(expr = "zone transaction {string} is included in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_transaction_is_included(
    world: &mut CucumberWorld,
    step: &Step,
    transaction_alias: String,
    timeout_seconds: u64,
) -> StepResult {
    let tx_hash = world.resolve_submitted_transaction(&transaction_alias)?;

    let node = world.zone_node_http_client().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    ensure_zone_transactions_included(&node, &[tx_hash], Duration::from_secs(timeout_seconds))
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    Ok(())
}

#[cucumber::then(expr = "zone transaction {string} is finalized in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_transaction_is_finalized(
    world: &mut CucumberWorld,
    step: &Step,
    transaction_alias: String,
    timeout_seconds: u64,
) -> StepResult {
    let tx_hash = world.resolve_submitted_transaction(&transaction_alias)?;

    let node_url = world.zone_node_url().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    wait_for_transactions_finalized(node_url, &[tx_hash], Duration::from_secs(timeout_seconds))
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    Ok(())
}

#[cucumber::then(expr = "the zone indexer returns finalized deposit {string} in {int} seconds")]
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Cucumber step functions require `&mut World` as the first parameter"
)]
async fn step_zone_indexer_returns_finalized_deposit(
    world: &mut CucumberWorld,
    step: &Step,
    deposit_alias: String,
    timeout_seconds: u64,
) -> StepResult {
    let deposit = world
        .zone
        .resolve_submitted_deposit(&deposit_alias)?
        .clone();
    let indexer = world.zone.indexer().inspect_err(|e| {
        warn!(target: TARGET, "Step `{}` error: {e}", step.value);
    })?;

    wait_for_deposit(indexer, &deposit, Duration::from_secs(timeout_seconds))
        .await
        .map_err(|error| {
            log_zone_error(step, &error);

            StepError::LogicalError {
                message: error.to_string(),
            }
        })?;

    Ok(())
}

fn zone_message_rows(step: &Step) -> Result<Vec<(String, Vec<u8>)>, StepError> {
    let table = step.table.as_ref().ok_or(StepError::MissingTable)?;

    if table.rows.is_empty() {
        return Err(StepError::InvalidArgument {
            message: "Zone message table must include a header row".to_owned(),
        });
    }

    table
        .rows
        .iter()
        .skip(1)
        .map(|row| match row.as_slice() {
            [alias, data] => Ok((alias.clone(), data.as_bytes().to_vec())),
            _ => Err(StepError::InvalidArgument {
                message: format!(
                    "Zone message rows must have exactly 2 columns (`alias`, `data`), got {}",
                    row.len()
                ),
            }),
        })
        .collect()
}

fn zone_message_aliases(step: &Step) -> Result<Vec<String>, StepError> {
    let table = step.table.as_ref().ok_or(StepError::MissingTable)?;

    if table.rows.is_empty() {
        return Err(StepError::InvalidArgument {
            message: "Zone message alias table must include a header row".to_owned(),
        });
    }

    table
        .rows
        .iter()
        .skip(1)
        .map(|row| match row.as_slice() {
            [alias] => Ok(alias.clone()),
            _ => Err(StepError::InvalidArgument {
                message: format!(
                    "Zone message alias rows must have exactly 1 column (`alias`), got {}",
                    row.len()
                ),
            }),
        })
        .collect()
}

fn log_zone_error(step: &Step, error: &ZoneTestError) {
    warn!(target: TARGET, "Step `{}` error: {error}", step.value);
}

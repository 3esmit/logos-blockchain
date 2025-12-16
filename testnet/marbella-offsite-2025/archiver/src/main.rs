use std::{path::Path, sync::Arc};

use broadcast_service::BlockInfo;
use clap::Parser;
use common_http_client::{BasicAuthCredentials, CommonHttpClient};
use cryptarchia_engine::Slot;
use futures::StreamExt as _;
use nomos_core::{
    block::Block,
    codec::{DeserializeOp as _, SerializeOp as _},
    header::HeaderId,
    mantle::{
        Op, SignedMantleTx,
        ops::channel::{ChannelId, inscribe::InscriptionOp},
    },
};
use serde::{Deserialize, Serialize};
use tokio::{fs, select, signal::ctrl_c};
use url::Url;

#[derive(Parser, Debug)]
struct CliArgs {
    #[clap(short = 'e', env = "ENDPOINT")]
    nomos_node_http_endpoint: Url,
    #[clap(short = 'u', env = "USERNAME")]
    username: String,
    #[clap(short = 'p', env = "PASSWORD")]
    password: String,
    #[clap(short = 'c', env = "CHANNEL_ID")]
    channel_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArchiverState {
    last_processed_height: Slot,
    last_processed_header_id: HeaderId,
}

impl ArchiverState {
    async fn load(path: &Path) -> Option<Self> {
        let content = fs::read(path).await.ok()?;
        Some(Self::from_bytes(&content).unwrap())
    }

    async fn save(&self, path: &Path) -> std::io::Result<()> {
        let content = self.to_bytes().unwrap();
        fs::write(path, content).await
    }
}

fn process_block(block: Block<SignedMantleTx>, decoded_channel_id: &ChannelId) {
    println!(
        "Processing block at height (slot): {:?}",
        block.header().slot()
    );
    let block_channel_ops = block
        .into_transactions()
        .into_iter()
        .flat_map(|tx| tx.mantle_tx.ops)
        .filter_map(|op| match op {
            Op::ChannelInscribe(InscriptionOp {
                channel_id,
                inscription,
                ..
            }) if &channel_id == decoded_channel_id => Some(inscription),
            _ => None,
        })
        .collect::<Vec<_>>();

    if block_channel_ops.is_empty() {
        println!("No inscriptions for specified channel in the received block.");
    } else {
        println!(
            "New inscriptions for specified {}: {block_channel_ops:?}",
            hex::encode(decoded_channel_id.as_ref())
        );
    }
}

async fn backfill_blocks(
    client: &CommonHttpClient,
    endpoint: &Url,
    from_header_id: HeaderId,
    until_header_id: Option<HeaderId>,
    decoded_channel_id: &ChannelId,
    state_file: &Path,
) -> Option<ArchiverState> {
    // Collect blocks by walking backwards from `from_header_id` until we reach
    // `until_header_id`
    let mut blocks_to_process = Vec::new();
    let mut current_id = from_header_id;

    loop {
        let block = match client.get_block_by_id(endpoint.clone(), current_id).await {
            Ok(Some(block)) => block,
            Ok(None) => {
                println!("Block not found: {current_id:?}");
                break;
            }
            Err(e) => {
                println!("Error fetching block {current_id:?}: {e}");
                break;
            }
        };

        let parent_id = block.header().parent();
        blocks_to_process.push(block);

        // Check if we've reached the last processed block
        if let Some(until_id) = until_header_id
            && parent_id == until_id
        {
            break;
        }

        current_id = parent_id;

        // If parent is zero (genesis), stop
        if current_id == HeaderId::from([0; _]) {
            break;
        }
    }

    // Process blocks in chronological order (oldest first)
    blocks_to_process.reverse();

    let mut last_state = None;
    for block in blocks_to_process {
        let header_id = block.header().id();
        let slot = block.header().slot();

        process_block(block, decoded_channel_id);

        let state = ArchiverState {
            last_processed_height: slot,
            last_processed_header_id: header_id,
        };

        if let Err(e) = state.save(state_file).await {
            println!("Warning: Failed to save state: {e}");
        }

        last_state = Some(state);
    }

    last_state
}

#[tokio::main]
async fn main() {
    let CliArgs {
        nomos_node_http_endpoint,
        username,
        password,
        channel_id,
    } = CliArgs::parse();

    let decoded_channel_id: ChannelId = <[u8; 32]>::try_from(hex::decode(&channel_id).unwrap())
        .unwrap()
        .into();

    println!("Nomos Node HTTP Endpoint: {nomos_node_http_endpoint}");
    println!("Channel ID: {channel_id:?}");

    // Load previous state if exists
    let state_file_path = Path::new("restore.dat");
    let mut last_state = ArchiverState::load(state_file_path).await;
    if let Some(state) = &last_state {
        println!(
            "Loaded previous state: height={:?}, header_id={:?}",
            state.last_processed_height, state.last_processed_header_id
        );
    } else {
        println!("No previous state found, starting fresh.");
    }

    let client = Arc::new(CommonHttpClient::new(Some(BasicAuthCredentials::new(
        username,
        Some(password),
    ))));

    let mut lib_stream = Box::pin(
        client
            .get_lib_stream(nomos_node_http_endpoint.clone())
            .await
            .unwrap(),
    );

    loop {
        select! {
            block_info = lib_stream.next() => {
                let Some(BlockInfo { header_id, height }) = block_info else {
                    println!("Stream ended.");
                    break;
                };

                println!("Received block info: height={height}, header_id={header_id:?}");

                // Check if we need to backfill
                let needs_backfill = last_state.as_ref().map_or(height > 0, |state| Slot::from(height) > state.last_processed_height + 1 || (Slot::from(height) == state.last_processed_height + 1 && {
                            // We should verify the parent matches, but for now just process
                            false
                        }));

                if needs_backfill {
                    let until_header_id = last_state.as_ref().map(|s| s.last_processed_header_id);
                    println!("Backfilling blocks from {header_id:?} until {until_header_id:?}");

                    if let Some(new_state) = backfill_blocks(
                        &client,
                        &nomos_node_http_endpoint,
                        header_id,
                        until_header_id,
                        &decoded_channel_id,
                        state_file_path,
                    ).await {
                        last_state = Some(new_state);
                    }
                } else {
                    // Just process the single new block
                    match client.get_block_by_id(nomos_node_http_endpoint.clone(), header_id).await {
                        Ok(Some(block)) => {
                            process_block(block, &decoded_channel_id);

                            let state = ArchiverState {
                                last_processed_height: height.into(),
                                last_processed_header_id: header_id,
                            };

                            if let Err(e) = state.save(state_file_path).await {
                                println!("Warning: Failed to save state: {e}");
                            }

                            last_state = Some(state);
                        }
                        Ok(None) => {
                            println!("Block not found: {header_id:?}");
                        }
                        Err(e) => {
                            println!("Error fetching block: {e}");
                        }
                    }
                }
            }
            _ = ctrl_c() => {
                println!("Received Ctrl-C, shutting down.");
                break;
            }
        }
    }
}

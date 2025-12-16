use broadcast_service::BlockInfo;
use clap::Parser;
use common_http_client::{BasicAuthCredentials, CommonHttpClient};
use futures::StreamExt;
use nomos_core::mantle::{
    Op,
    ops::channel::{ChannelId, inscribe::InscriptionOp},
};
use tokio::{select, signal::ctrl_c};
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
        .try_into()
        .unwrap();
    println!("Nomos Node HTTP Endpoint: {nomos_node_http_endpoint}");
    println!("Channel ID: {channel_id:?}");

    let client = CommonHttpClient::new(Some(BasicAuthCredentials::new(username, Some(password))));
    let lib_stream = client
        .get_lib_stream(nomos_node_http_endpoint.clone())
        .await
        .unwrap();

    let mut lib_stream = Box::pin(lib_stream.then(|BlockInfo { header_id, .. }| {
        let endpoint = nomos_node_http_endpoint.clone();
        let client = &client;
        async move {
            client
                .get_block_by_id(endpoint, header_id)
                .await
                .unwrap()
                .unwrap()
        }
    }));

    loop {
        select! {
            block = lib_stream.next() => {
                let block = block.unwrap();
                println!("Block: {block:?}");
                let block_channel_ops = block.into_transactions().into_iter()
                .flat_map(|tx| tx.mantle_tx.ops)
                .filter_map(|op| match op {
                    Op::ChannelInscribe(InscriptionOp {
                        channel_id,
                        inscription,
                        ..
                    }) if channel_id == decoded_channel_id => Some(inscription),
                    _ => None,
                })
                .collect::<Vec<_>>();
                if block_channel_ops.is_empty() {
                    println!("No inscriptions for specified channel in the received block.");
                } else {
                    println!("New inscriptions for specified {channel_id}: {block_channel_ops:?}");
                }
            }
            _ = ctrl_c() => {
                println!("Received Ctrl-C, shutting down.");
                break;
            }
        }
    }
}

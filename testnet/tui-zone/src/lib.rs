mod message;
mod state;
mod ui;

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use lb_core::mantle::ops::channel::ChannelId;
use lb_key_management_system_service::keys::{
    ED25519_SECRET_KEY_SIZE, Ed25519Key, Ed25519PublicKey,
};
use lb_zone_sdk::{
    CommonHttpClient,
    adapter::NodeHttpClient,
    sequencer::{Event, ZoneSequencer},
};
use reqwest::Url;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::{
    message::AppMessage,
    state::{InMemoryZoneState, ZoneState as _, resolve_conflicts},
};

const CHANNEL_ID_SIDECAR: &str = "channel.id";

#[derive(Parser, Debug)]
#[command(about = "Terminal UI zone sequencer")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the interactive publishing TUI.
    Inscribe(InscribeArgs),
    /// Bootstrap a shared channel: create N keys, authorize them via
    /// `set_keys`, and write a `channel.id` sidecar so other sequencers can
    /// join the same channel without copying hex by hand.
    Setup(SetupArgs),
}

#[derive(Parser, Debug)]
pub struct InscribeArgs {
    /// Logos blockchain node HTTP endpoint.
    #[arg(long, default_value = "http://localhost:8080", env = "NODE_URL")]
    node_url: String,

    /// Path to the signing key file (created if it doesn't exist).
    #[arg(long, default_value = "sequencer.key", env = "KEY_PATH")]
    key_path: String,

    /// Channel ID hex (32 bytes = 64 hex chars). If not provided, the channel
    /// id is auto-discovered from a `channel.id` sidecar in the same directory
    /// as the signing key, falling back to deriving it from the signing key's
    /// public key (i.e. running as the channel admin/sole owner).
    #[arg(long, env = "CHANNEL_ID")]
    channel_id: Option<String>,
}

#[derive(Parser, Debug)]
pub struct SetupArgs {
    /// Logos blockchain node HTTP endpoint.
    #[arg(long, default_value = "http://localhost:8080", env = "NODE_URL")]
    node_url: String,

    /// Directory to write keys + `channel.id` into. Created if it doesn't
    /// exist. Existing key files are preserved (this command will fail rather
    /// than overwrite).
    #[arg(long, default_value = "./demo")]
    out_dir: PathBuf,

    /// Number of sequencer keys to create and authorize. The first key
    /// (`seq0.key`) is the channel admin.
    #[arg(long, default_value_t = 3)]
    n: usize,
}

pub async fn run(cli: Cli) {
    match cli.command {
        Command::Inscribe(args) => run_inscribe(args).await,
        Command::Setup(args) => run_setup(args).await,
    }
}

pub async fn run_inscribe(args: InscribeArgs) {
    let node_url: Url = args.node_url.parse().expect("invalid node URL");
    let signing_key = load_or_create_signing_key(Path::new(&args.key_path));
    let channel_id = resolve_channel_id(
        args.channel_id.as_deref(),
        Path::new(&args.key_path),
        &signing_key,
    );

    println!("TUI Zone Sequencer");
    println!("  Node:       {node_url}");
    println!("  Key:        {}", args.key_path);
    println!(
        "  My pubkey:  {}",
        hex::encode(signing_key.public_key().to_bytes())
    );
    println!("  Channel ID: {}", hex::encode(channel_id.as_ref()));
    println!();

    let mut state = InMemoryZoneState::default();
    let checkpoint = state.load_checkpoint().cloned();

    let my_signer = signing_key.public_key();
    let node = NodeHttpClient::new(CommonHttpClient::new(None), node_url);
    let (mut sequencer, handle) = ZoneSequencer::init(channel_id, signing_key, node, checkpoint);

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let mut stdin_rx = spawn_stdin_reader(ready_rx);
    let mut ready_tx = Some(ready_tx);

    println!("Bootstrapping sequencer...");

    loop {
        tokio::select! {
            event = sequencer.next_event() => {
                if let Some(event) = event {
                    handle_event(event, &mut state, &handle, &my_signer, &mut ready_tx).await;
                }
            }

            input = stdin_rx.recv() => {
                let Some(text) = input else {
                    println!();
                    break;
                };

                let msg = AppMessage::new(text);
                debug!(tx_uuid = %msg.tx_uuid, text = %msg.text, "Publishing message");
                if let Err(e) = handle.publish_message(msg.to_bytes()).await {
                    error!("failed to publish: {e}");
                    break;
                }
                eprintln!("  \x1b[90mpending...\x1b[0m");
                ui::prompt();
            }

            _ = tokio::signal::ctrl_c() => {
                println!();
                break;
            }
        }
    }

    println!("Goodbye!");
}

pub async fn run_setup(args: SetupArgs) {
    let node_url: Url = args.node_url.parse().expect("invalid node URL");
    fs::create_dir_all(&args.out_dir).expect("failed to create out_dir");
    assert!(args.n >= 1, "--n must be at least 1");

    println!("TUI Zone Sequencer - Channel Setup");
    println!("  Node:    {node_url}");
    println!("  Out:     {}", args.out_dir.display());
    println!("  N keys:  {}", args.n);
    println!();

    // Generate N keys; refuse to overwrite existing files.
    let mut keys: Vec<Ed25519Key> = Vec::with_capacity(args.n);
    let mut key_paths: Vec<PathBuf> = Vec::with_capacity(args.n);
    for i in 0..args.n {
        let path = args.out_dir.join(format!("seq{i}.key"));
        assert!(
            !path.exists(),
            "key already exists at {} (refusing to overwrite)",
            path.display()
        );
        let key = create_signing_key(&path);
        println!(
            "  seq{i}: {} -> {}",
            hex::encode(key.public_key().to_bytes()),
            path.display()
        );
        keys.push(key);
        key_paths.push(path);
    }
    println!();

    // First key is the admin; channel_id is derived from its pubkey.
    let admin_key = keys[0].clone();
    let admin_pk = admin_key.public_key();
    let channel_id_bytes: [u8; 32] = admin_pk.to_bytes();
    let channel_id = ChannelId::from(channel_id_bytes);
    let channel_id_hex = hex::encode(channel_id.as_ref());

    let pubkeys: Vec<_> = keys.iter().map(Ed25519Key::public_key).collect();

    println!(
        "Authorizing all {} keys via set_keys (admin = seq0)...",
        args.n
    );
    let node = NodeHttpClient::new(CommonHttpClient::new(None), node_url);
    let (mut sequencer, mut handle) = ZoneSequencer::init(channel_id, admin_key, node, None);

    // Drive the event loop in the background while we issue set_keys.
    let driver = tokio::spawn(async move {
        loop {
            drop(sequencer.next_event().await);
        }
    });

    handle.wait_ready().await;
    let (_result, finalized) = handle
        .set_keys(pubkeys)
        .await
        .expect("set_keys submission failed");
    finalized.await.expect("set_keys finalization failed");
    driver.abort();

    // Write sidecar so other sequencers can auto-discover the channel.
    let sidecar_path = args.out_dir.join(CHANNEL_ID_SIDECAR);
    fs::write(&sidecar_path, &channel_id_hex).expect("failed to write channel.id");

    println!();
    println!("Setup complete.");
    println!("  Channel ID: {channel_id_hex}");
    println!("  Sidecar:    {}", sidecar_path.display());
    println!();
    println!("Run sequencers (channel id auto-discovered from sidecar):");
    for path in &key_paths {
        println!("  tui-sequencer inscribe --key-path {}", path.display());
    }
}

fn resolve_channel_id(arg: Option<&str>, key_path: &Path, signing_key: &Ed25519Key) -> ChannelId {
    if let Some(hex_str) = arg {
        return parse_channel_id_hex(hex_str.trim());
    }
    if let Some(parent) = key_path.parent() {
        let sidecar = parent.join(CHANNEL_ID_SIDECAR);
        if sidecar.exists() {
            let hex_str = fs::read_to_string(&sidecar).expect("failed to read channel.id sidecar");
            return parse_channel_id_hex(hex_str.trim());
        }
    }
    ChannelId::from(signing_key.public_key().to_bytes())
}

fn parse_channel_id_hex(hex_str: &str) -> ChannelId {
    let bytes = hex::decode(hex_str).expect("invalid channel id hex");
    let arr: [u8; 32] = bytes
        .try_into()
        .expect("channel id must be exactly 32 bytes (64 hex chars)");
    ChannelId::from(arr)
}

async fn handle_event(
    event: Event,
    state: &mut InMemoryZoneState,
    handle: &lb_zone_sdk::sequencer::SequencerHandle<NodeHttpClient>,
    my_signer: &Ed25519PublicKey,
    ready_tx: &mut Option<tokio::sync::oneshot::Sender<()>>,
) {
    match event {
        Event::Ready => handle_ready(state, ready_tx),
        Event::ChannelUpdate {
            orphaned,
            adopted,
            pending,
        } => {
            handle_channel_update(state, handle, &orphaned, &adopted, &pending, my_signer).await;
        }
        Event::TxsFinalized { inscriptions, .. } => {
            finalize_inscriptions(state, &inscriptions, true);
        }
        Event::Published {
            payload,
            checkpoint,
            ..
        } => {
            debug!("Inscription published, checkpoint saved");
            if let Some(msg) = AppMessage::from_bytes(&payload) {
                state.apply(msg);
                ui::render_state(state);
                ui::prompt();
            }
            state.save_checkpoint(checkpoint);
        }
        Event::FinalizedInscriptions { inscriptions } => {
            finalize_inscriptions(state, &inscriptions, false);
        }
    }
}

fn handle_ready(
    state: &InMemoryZoneState,
    ready_tx: &mut Option<tokio::sync::oneshot::Sender<()>>,
) {
    info!("Sequencer ready");
    if let Some(tx) = ready_tx.take() {
        let _ = tx.send(());
    }
    println!("Ready.");
    println!();
    println!("Type a message and press Enter to publish.");
    println!("Press Ctrl-D or type an empty line to exit.");
    println!();
    ui::render_state(state);
    ui::prompt();
}

async fn handle_channel_update(
    state: &mut InMemoryZoneState,
    handle: &lb_zone_sdk::sequencer::SequencerHandle<NodeHttpClient>,
    orphaned: &[lb_zone_sdk::state::InscriptionInfo],
    adopted: &[lb_zone_sdk::state::InscriptionInfo],
    pending: &[lb_zone_sdk::state::InscriptionInfo],
    my_signer: &Ed25519PublicKey,
) {
    if orphaned.is_empty() && adopted.is_empty() {
        return;
    }

    debug!(
        orphaned = orphaned.len(),
        adopted = adopted.len(),
        pending = pending.len(),
        "Channel update"
    );

    let to_republish = resolve_conflicts(state, orphaned, adopted, pending, my_signer);
    republish(handle, to_republish).await;

    ui::render_state(state);
    ui::prompt();
}

async fn republish(
    handle: &lb_zone_sdk::sequencer::SequencerHandle<NodeHttpClient>,
    messages: Vec<AppMessage>,
) {
    if messages.is_empty() {
        return;
    }
    info!(count = messages.len(), "Re-publishing after conflict");
    for msg in messages {
        if let Err(e) = handle.publish_message(msg.to_bytes()).await {
            error!("failed to re-publish: {e}");
            break;
        }
    }
}

fn finalize_inscriptions(
    state: &mut InMemoryZoneState,
    inscriptions: &[lb_zone_sdk::state::InscriptionInfo],
    render: bool,
) {
    debug!(count = inscriptions.len(), "Inscriptions finalized");
    let payloads: Vec<Vec<u8>> = inscriptions.iter().map(|i| i.payload.clone()).collect();
    state.finalize(&payloads);
    if render {
        ui::render_state(state);
        ui::prompt();
    }
}

fn load_or_create_signing_key(path: &Path) -> Ed25519Key {
    if path.exists() {
        let key_bytes = fs::read(path).expect("failed to read key file");
        assert!(
            key_bytes.len() == ED25519_SECRET_KEY_SIZE,
            "invalid key file: expected {} bytes, got {}",
            ED25519_SECRET_KEY_SIZE,
            key_bytes.len()
        );
        let key_array: [u8; ED25519_SECRET_KEY_SIZE] =
            key_bytes.try_into().expect("length already checked");
        Ed25519Key::from_bytes(&key_array)
    } else {
        create_signing_key(path)
    }
}

fn create_signing_key(path: &Path) -> Ed25519Key {
    let mut key_bytes = [0u8; ED25519_SECRET_KEY_SIZE];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_bytes);
    fs::write(path, key_bytes).expect("failed to write key file");
    Ed25519Key::from_bytes(&key_bytes)
}

fn spawn_stdin_reader(ready: tokio::sync::oneshot::Receiver<()>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        // Wait until the sequencer is ready before accepting input
        if ready.blocking_recv().is_err() {
            return;
        }

        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = line.trim_end().to_owned();
                    if text.is_empty() || tx.blocking_send(text).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

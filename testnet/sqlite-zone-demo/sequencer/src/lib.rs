pub mod db;
pub mod sequencer;

mod ctrl_c;

use std::sync::Arc;

use clap::Parser;
use db::Menu;
use sequencer::Sequencer;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
use lb_core::mantle::ops::channel::ChannelId;

#[derive(Parser, Debug)]
#[command(about = "SQLite zone sequencer")]
pub struct SequencerArgs {
    /// Logos blockchain node HTTP endpoint
    #[arg(long, default_value = "http://localhost:8080", env = "SEQUENCER_NODE_ENDPOINT")]
    pub node_url: String,

    /// Path to the SQLite database file
    #[arg(long, default_value = "./data/database.db", env = "SEQUENCER_DB_PATH")]
    pub db_path: String,

    /// Path to the signing key file (created if it doesn't exist)
    #[arg(long, default_value = "./data/sequencer.key", env = "SEQUENCER_SIGNING_KEY_PATH")]
    pub key_path: String,

    /// Basic auth username for node endpoint
    #[arg(long, env = "SEQUENCER_NODE_AUTH_USERNAME")]
    pub node_auth_username: Option<String>,

    /// Basic auth password for node endpoint
    #[arg(long, env = "SEQUENCER_NODE_AUTH_PASSWORD")]
    pub node_auth_password: Option<String>,

    /// Path to the queue file for pending SQL statements
    #[arg(long, default_value = "./data/queue.txt", env = "SEQUENCER_QUEUE_FILE")]
    pub queue_file: String,

    /// Path to the checkpoint file for crash recovery
    #[arg(long, default_value = "./data/sequencer.checkpoint", env = "CHECKPOINT_PATH")]
    checkpoint_path: String,
}

pub async fn run(args: SequencerArgs) {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Sqlite Sequencer starting up...");
    info!("Configuration");
    info!("  Logos blockchain Node: {}", args.node_url);
    info!("  Database:              {}", args.db_path);

    let mut db = match Menu::new(&args.db_path) {
        Ok(db) => db,
        Err(e) => {
            error!("Database initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Database ready");

    let sequencer = match Sequencer::new(
        &args.node_url,
        &args.key_path,
        args.node_auth_username,
        args.node_auth_password,
        &args.queue_file,
        &args.checkpoint_path,
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("Sequencer initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Sequencer ready");

    let cancellation_token = CancellationToken::new();
    ctrl_c::listen_for_sigint(cancellation_token.clone());

    db.enable_tracing(&args.queue_file);

    let sequencer_clone = Arc::clone(&sequencer);
    tokio::spawn(async move {
        sequencer_clone.run_processing_loop().await;
    });
    info!("Background processor started");

    println!("Type SQL queries followed by ENTER");
    println!("Type 'q' or CTRL+C then ENTER to exit.");

    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    use tokio::io::AsyncBufReadExt;

    let mut lines = reader.lines();
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                break;
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(input)) => {
                        let input = input.trim().to_string();
                        if input.is_empty() {
                            continue;
                        }
                        if input.eq_ignore_ascii_case("q") {
                            cancellation_token.cancel();
                            continue;
                        }
                        match db.query(input).await {
                            Ok(Some(dishes)) => {
                                for dish in &dishes {
                                    println!("ID: {} | Name: {} | Data: {}", dish.id, dish.name, dish.data);
                                }
                                println!("({} row(s))", dishes.len());
                            }
                            Ok(None) => {
                                println!("OK");
                            }
                            Err(e) => {
                                eprintln!("Error: {e}");
                            }
                        }
                    }
                    Ok(None) => {
                        cancellation_token.cancel();
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

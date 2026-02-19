mod config;
mod ctrl_c;

use std::sync::Arc;

use demo_sqlite_sequencer::db::Menu;
use demo_sqlite_sequencer::sequencer::Sequencer;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{config::Config, ctrl_c::listen_for_sigint};

const QUEUE_FILE: &str = "./data/queue.txt";

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Sqlite Sequencer starting up...");

    // Load configuration
    let config = Config::from_env();
    info!("Configuration");
    info!("  Logos blockchain Node:     {}", config.node_endpoint);
    info!("  Database:                  {}", config.db_path);
    info!("  Channel ID:                {}", config.channel_id);

    // Initialize database
    let mut db = match Menu::new(&config.db_path) {
        Ok(db) => db,
        Err(e) => {
            error!("Database initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Database ready");

    // Initialize sequencer
    let sequencer = match Sequencer::new(
        &config.node_endpoint,
        &config.signing_key_path,
        &config.channel_id,
        config.node_auth_username,
        config.node_auth_password,
        QUEUE_FILE,
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("Sequencer initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Sequencer ready");

    // Setup cancellation token for graceful shutdown
    let cancellation_token = CancellationToken::new();
    listen_for_sigint(cancellation_token.clone());

    // Write sql traces to file
    db.enable_tracing(QUEUE_FILE);

    // Spawn background processing loop
    let sequencer_clone = Arc::clone(&sequencer);
    tokio::spawn(async move {
        sequencer_clone.run_processing_loop().await;
    });
    info!("Background processor started");

    // Start REPL
    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    use tokio::io::AsyncBufReadExt;

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    println!("Type SQL queries followed by ENTER");
    println!("Type 'q' or CTRL+C then ENTER to exit.");
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
                        // EOF
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

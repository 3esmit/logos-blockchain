pub mod db;
pub mod indexer;

mod ctrl_c;

use std::sync::Arc;

use clap::Parser;
use indexer::Indexer;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Parser, Debug)]
#[command(about = "SQLite zone indexer - replay zone blocks into a local SQLite database")]
pub struct IndexerArgs {
    /// Logos blockchain node HTTP endpoint
    #[arg(long, default_value = "http://localhost:18081", env = "INDEXER_NODE_ENDPOINT")]
    pub node_url: String,

    /// Path to the SQLite database file
    #[arg(long, default_value = "./data/indexer.db", env = "INDEXER_DB_PATH")]
    pub db_path: String,

    /// Channel ID to index (64 hex characters / 32 bytes)
    #[arg(long, env = "CHANNEL_ID")]
    pub channel_id: String,

    /// Basic auth username for node endpoint
    #[arg(long, env = "INDEXER_NODE_AUTH_USERNAME")]
    pub node_auth_username: Option<String>,

    /// Basic auth password for node endpoint
    #[arg(long, env = "INDEXER_NODE_AUTH_PASSWORD")]
    pub node_auth_password: Option<String>,
}

pub async fn run(args: IndexerArgs) {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Sqlite Indexer starting up...");
    info!("Configuration");
    info!("  Logos blockchain Node: {}", args.node_url);
    info!("  Database:              {}", args.db_path);
    info!("  Channel ID:            {}", args.channel_id);

    let indexer = match Indexer::new(
        &args.db_path,
        &args.node_url,
        &args.channel_id,
        args.node_auth_username,
        args.node_auth_password,
    ) {
        Ok(i) => Arc::new(i),
        Err(e) => {
            error!("Indexer initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Indexer ready");

    let cancellation_token = CancellationToken::new();
    ctrl_c::listen_for_sigint(cancellation_token.clone());

    let indexer_clone = Arc::clone(&indexer);
    tokio::spawn(async move {
        indexer_clone.run().await;
    });
    info!("Background indexer started");

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
                        if !input
                            .split_whitespace()
                            .next()
                            .is_some_and(|first| first.eq_ignore_ascii_case("SELECT"))
                        {
                            println!("Only SELECT queries permitted");
                            continue;
                        }
                        match indexer.db().lock().await.query(input).await {
                            Ok(dishes) => {
                                for dish in &dishes {
                                    println!("ID: {} | Name: {} | Data: {}", dish.id, dish.name, dish.data);
                                }
                                println!("({} row(s))", dishes.len());
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

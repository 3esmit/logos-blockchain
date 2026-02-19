mod config;
mod ctrl_c;

use std::sync::Arc;

use demo_sqlite_indexer::indexer::Indexer;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{config::Config, ctrl_c::listen_for_sigint};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Sqlite Indexer starting up...");

    let config = Config::from_env();
    info!("Configuration");
    info!("  Logos blockchain Node: {}", config.node_endpoint);
    info!("  Database:              {}", config.db_path);
    info!("  Channel ID:            {}", config.channel_id);

    let indexer = match Indexer::new(
        &config.db_path,
        &config.node_endpoint,
        &config.channel_id,
        config.node_auth_username,
        config.node_auth_password,
    ) {
        Ok(i) => Arc::new(i),
        Err(e) => {
            error!("Indexer initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Indexer ready");

    let cancellation_token = CancellationToken::new();
    listen_for_sigint(cancellation_token.clone());

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
                            .is_some_and(|first| first.eq_ignore_ascii_case("SELECT")) {
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

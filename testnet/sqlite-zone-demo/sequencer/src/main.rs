mod api;
mod config;
mod ctrl_c;
mod sequencer;

use std::sync::Arc;

use demo_sqlite_sequencer::db::Menu;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{api::create_router, config::Config, ctrl_c::listen_for_sigint, sequencer::Sequencer};

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
    info!("  HTTP API:                  {}", config.listen_addr);
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

    // Initialize sequencer (uses Zone SDK for transaction submission and chain inclusion)
    let sequencer = match Sequencer::new(
        &config.db_path,
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
    info!("Sequencer ready (using Zone SDK)");

    // Setup cancellation token for graceful shutdown
    let cancellation_token = CancellationToken::new();
    listen_for_sigint(cancellation_token.clone());

    //Write sql traces to file
    db.enable_tracing(QUEUE_FILE);

    // Spawn background processing loop
    let sequencer_clone = Arc::clone(&sequencer);
    tokio::spawn(async move {
        sequencer_clone.run_processing_loop().await;
    });
    info!("Background processor started");

    // Create HTTP router
    let app = create_router(db.into(), sequencer);

    // Start HTTP server
    info!("Sqlite Sequencer listening on {}", config.listen_addr);
    let listener = match tokio::net::TcpListener::bind(config.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind: {e}");
            std::process::exit(1);
        }
    };

    let shutdown_signal = cancellation_token.cancelled_owned();
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
    {
        error!("Server error: {e}");
        std::process::exit(1);
    }
}

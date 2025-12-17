mod api;
mod config;
mod db;
mod sequencer;

use std::sync::Arc;

use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{api::create_router, config::Config, db::AccountDb, sequencer::Sequencer};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting sequencer...");

    // Load configuration
    let config = Config::from_env();
    info!("Configuration loaded:");
    info!("  Listen address: {}", config.listen_addr);
    info!("  Node endpoint: {}", config.node_endpoint);
    info!("  Database path: {}", config.db_path);
    info!("  Signing key path: {}", config.signing_key_path);
    info!("  Initial balance: {}", config.initial_balance);
    info!(
        "  Node auth: {}",
        if config.node_auth_username.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );

    // Initialize database
    let db = match AccountDb::new(&config.db_path, config.initial_balance) {
        Ok(db) => db,
        Err(e) => {
            error!("Failed to initialize database: {e}");
            std::process::exit(1);
        }
    };
    info!("Database initialized");

    // Initialize sequencer
    let sequencer = match Sequencer::new(
        db,
        &config.node_endpoint,
        &config.signing_key_path,
        config.node_auth_username,
        config.node_auth_password,
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("Failed to initialize sequencer: {e}");
            std::process::exit(1);
        }
    };
    info!("Sequencer initialized");

    // Spawn background processing loop
    let sequencer_clone = Arc::clone(&sequencer);
    tokio::spawn(async move {
        info!("Starting background processing loop");
        sequencer_clone.run_processing_loop().await;
    });

    // Create HTTP router
    let app = create_router(sequencer);

    // Start HTTP server
    info!("Starting HTTP server on {}", config.listen_addr);
    let listener = match tokio::net::TcpListener::bind(config.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to {}: {e}", config.listen_addr);
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {e}");
        std::process::exit(1);
    }
}

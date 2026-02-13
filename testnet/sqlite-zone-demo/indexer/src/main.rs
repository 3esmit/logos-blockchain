mod config;
mod ctrl_c;

use std::sync::Arc;

use demo_sqlite_indexer::api::create_router;
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
    info!("  HTTP API:              {}", config.listen_addr);
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

    let app = create_router(indexer.db());

    info!("Sqlite Indexer listening on {}", config.listen_addr);
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

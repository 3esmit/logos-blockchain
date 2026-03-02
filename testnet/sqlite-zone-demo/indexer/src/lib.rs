#![forbid(unsafe_code)]

pub mod db;
pub mod indexer;
pub use demo_sqlite_common::{config, crypto, error, screen};

mod tui;

use std::sync::Arc;

use clap::Parser;
use indexer::Indexer;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
use demo_sqlite_common::logging::RawModeWriter;

#[derive(Parser, Debug)]
#[command(about = "SQLite zone indexer - replay zone blocks into a local SQLite database")]
pub struct IndexerArgs {
    /// Logos blockchain node HTTP endpoint
    #[arg(long, default_value = "http://localhost:8080", env = "INDEXER_NODE_ENDPOINT")]
    pub node_url: String,

    /// Path to the SQLite database file
    #[arg(long, default_value = "./data/indexer.db", env = "INDEXER_DB_PATH")]
    pub db_path: String,

    /// Path to the channel ID file
    #[arg(long, default_value = "./data/channel.txt")]
    channel_path: String,

    /// Basic auth username for node endpoint
    #[arg(long, env = "INDEXER_NODE_AUTH_USERNAME")]
    pub node_auth_username: Option<String>,

    /// Basic auth password for node endpoint
    #[arg(long, env = "INDEXER_NODE_AUTH_PASSWORD")]
    pub node_auth_password: Option<String>,
}

use crate::{
    config::Config,
    db::DatabaseReadOnly,
    tui::State,
    screen::ScreenGuard,
    error::Result,
};

#[derive(Debug)]
struct App {
    screen: ScreenGuard,
    state: State,
}

impl App {
    fn new(state: State) -> Result<Self> {
        Ok(App {
            screen: ScreenGuard::open()?,
            state,
        })
    }

    /// The main run loop.
    fn run_app(mut self) -> Result<()> {
        while self.state.is_running() {
            self.screen.draw(|frame| self.state.draw(frame))?;
            self.state.handle_events();
        }

        Ok(())
    }
}

pub async fn run(args: IndexerArgs) -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_writer(RawModeWriter))
        .init();

    info!("Sqlite Indexer starting up...");
    info!("  Logos blockchain Node: {}", args.node_url);

    let db = DatabaseReadOnly::open(&args.db_path)?;

    let indexer = match Indexer::new(
        &args.db_path,
        &args.node_url,
        &args.channel_path,
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

    let indexer_clone = Arc::clone(&indexer);
    tokio::spawn(async move {
        indexer_clone.run().await;
    });
    info!("Background indexer started");

    let config = Config::from_rc_file()?;
    let state = State::new(db, config.theme)?;
    let app = App::new(state)?;

    app.run_app()
}

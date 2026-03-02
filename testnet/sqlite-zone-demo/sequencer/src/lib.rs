#![forbid(unsafe_code)]
#![allow(clippy::allow_attributes_without_reason)]

pub mod db;
pub mod sequencer;
pub use demo_sqlite_common::{config, crypto, error, screen};

mod tui;

use std::sync::Arc;

use clap::Parser;
use demo_sqlite_common::logging::RawModeWriter;
use sequencer::Sequencer;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Parser, Debug)]
#[command(about = "SQLite zone sequencer")]
pub struct SequencerArgs {
    /// Logos blockchain node HTTP endpoint
    #[arg(
        long,
        default_value = "http://localhost:8080",
        env = "SEQUENCER_NODE_ENDPOINT"
    )]
    pub node_url: String,

    /// Path to the `SQLite` database file
    #[arg(long, default_value = "./data/database.db", env = "SEQUENCER_DB_PATH")]
    pub db_path: String,

    /// Path to the signing key file (created if it doesn't exist)
    #[arg(
        long,
        default_value = "./data/sequencer.key",
        env = "SEQUENCER_SIGNING_KEY_PATH"
    )]
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
    #[arg(
        long,
        default_value = "./data/sequencer.checkpoint",
        env = "CHECKPOINT_PATH"
    )]
    checkpoint_path: String,

    /// Path to the channel ID file
    #[arg(long, default_value = "./data/channel.txt", env = "CHANNEL_PATH")]
    channel_path: String,
}

use crate::{config::Config, db::Database, error::Result, screen::ScreenGuard, tui::State};

#[derive(Debug)]
struct App {
    screen: ScreenGuard,
    state: State,
}

impl App {
    fn new(state: State) -> Result<Self> {
        Ok(Self {
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

#[expect(clippy::unused_async)]
pub async fn run(args: SequencerArgs) -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_writer(RawModeWriter))
        .init();

    info!("Sqlite Sequencer starting up...");
    info!("  Logos blockchain Node: {}", args.node_url);

    let db = Database::open(&args.db_path, &args.queue_file)?;

    let sequencer = match Sequencer::new(
        &args.node_url,
        &args.key_path,
        args.node_auth_username,
        args.node_auth_password,
        &args.queue_file,
        &args.checkpoint_path,
        &args.channel_path,
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("Sequencer initialization failed: {e}");
            std::process::exit(1);
        }
    };
    info!("Sequencer ready");

    let sequencer_clone = Arc::clone(&sequencer);
    tokio::spawn(async move {
        sequencer_clone.run_processing_loop().await;
    });
    info!("Background processor started");

    let config = Config::from_rc_file()?;
    let state = State::new(db, config.theme)?;
    let app = App::new(state)?;

    app.run_app()
}

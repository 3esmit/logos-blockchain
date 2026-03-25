use std::{
    fs,
    fs::OpenOptions,
    io,
    io::{BufRead as _, BufReader},
    path::Path,
    time::Duration,
};

use fs2::FileExt as _;
use lb_common_http_client::BasicAuthCredentials;
use lb_core::mantle::ops::channel::ChannelId;
use lb_key_management_system_service::keys::{ED25519_SECRET_KEY_SIZE, Ed25519Key};
use logos_blockchain_zone_sdk::sequencer::{
    Error as ZoneSequencerError, SequencerCheckpoint, ZoneSequencer,
};
use nanosql::rusqlite::Error as SqliteError;
use reqwest::Url;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info};

#[derive(Debug, Error)]
pub enum SequencerError {
    #[error("Zone sequencer error: {0}")]
    ZoneSequencer(#[from] ZoneSequencerError),
    #[error("URL parse error: {0}")]
    Url(String),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] SqliteError),
    #[error("Invalid key file: expected {expected} bytes, got {actual}")]
    InvalidKeyFile { expected: usize, actual: usize },
    #[error("{0}")]
    InvalidChannelId(String),
    #[error("Timeout: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, SequencerError>;


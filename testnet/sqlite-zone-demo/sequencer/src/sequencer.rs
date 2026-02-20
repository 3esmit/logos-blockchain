use std::{fs::OpenOptions, io::{BufRead, BufReader}, path::Path, time::Duration, io, fs};
use fs2::FileExt;

use lb_common_http_client::BasicAuthCredentials;
use lb_key_management_system_service::keys::{ED25519_SECRET_KEY_SIZE, Ed25519Key};
use lb_core::mantle::ops::channel::ChannelId;
use logos_blockchain_zone_sdk::sequencer::{ZoneSequencer, Error as ZoneSequencerError, SequencerCheckpoint};
use reqwest::Url;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info};

use rusqlite::Error as SqliteError;

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

/// The sequencer that handles transactions using the Zone SDK
pub struct Sequencer {
    zone_sequencer: ZoneSequencer,
    queue_file: String,
    checkpoint_path: String,
}

/// Load signing key from file or generate a new one if it doesn't exist
fn load_or_create_signing_key(path: &Path) -> Result<Ed25519Key> {
    if path.exists() {
        debug!("Loading existing signing key from {:?}", path);
        let key_bytes = fs::read(path)?;
        if key_bytes.len() != ED25519_SECRET_KEY_SIZE {
            return Err(SequencerError::InvalidKeyFile {
                expected: ED25519_SECRET_KEY_SIZE,
                actual: key_bytes.len(),
            });
        }
        let key_array: [u8; ED25519_SECRET_KEY_SIZE] =
            key_bytes.try_into().expect("length already checked");
        Ok(Ed25519Key::from_bytes(&key_array))
    } else {
        debug!("Generating new signing key and saving to {:?}", path);
        let mut key_bytes = [0u8; ED25519_SECRET_KEY_SIZE];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_bytes);
        fs::write(path, key_bytes)?;
        Ok(Ed25519Key::from_bytes(&key_bytes))
    }
}

fn save_checkpoint(path: &Path, checkpoint: &SequencerCheckpoint) {
    let data = serde_json::to_vec(checkpoint).expect("failed to serialize checkpoint");
    fs::write(path, data).expect("failed to write checkpoint file");
}

fn load_checkpoint(path: &Path) -> Option<SequencerCheckpoint> {
    if !path.exists() {
        return None;
    }
    let data = fs::read(path).expect("failed to read checkpoint file");
    Some(serde_json::from_slice(&data).expect("failed to deserialize checkpoint"))
}

impl Sequencer {
    pub fn new(
        node_endpoint: &str,
        signing_key_path: &str,
        node_auth_username: Option<String>,
        node_auth_password: Option<String>,
        queue_file: &str,
        checkpoint_path: &str,
    ) -> Result<Self> {
        let node_url = Url::parse(node_endpoint).map_err(|e| SequencerError::Url(e.to_string()))?;

        let basic_auth = node_auth_username.map(|username| {
            BasicAuthCredentials::new(username, node_auth_password)
        });

        let checkpoint = load_checkpoint(Path::new(&checkpoint_path));
        if checkpoint.is_some() {
            println!("  Restored checkpoint from {}", checkpoint_path);
        }

        let signing_key = load_or_create_signing_key(Path::new(signing_key_path))?;
        let channel_id = ChannelId::from(signing_key.public_key().to_bytes());

        println!("Channel ID: {}", hex::encode(channel_id.as_ref()));

        let zone_sequencer = ZoneSequencer::init(
            channel_id,
            signing_key,
            node_url,
            basic_auth,
            checkpoint,
        );

        Ok(Self {
            zone_sequencer,
            queue_file: queue_file.to_string(),
            checkpoint_path: checkpoint_path.to_string(),
        })
    }

    /// Drain the queue file and return all pending queries
    async fn queue_drain(&self) -> Result<Vec<String>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.queue_file.clone())?;

        file.lock_exclusive()?;

        let reader = BufReader::new(&file);
        let mut queue_vec = Vec::new();
        for query in reader.lines() {
            queue_vec.push(query?.to_string());
        }

        file.set_len(0)?;

        Ok(queue_vec)
    }

    /// Process all pending queries as a single inscription
    async fn process_pending_batch(&self) -> Result<()> {
        let pending = self.queue_drain().await?;
        if pending.is_empty() {
            return Ok(());
        }

        let count = pending.len();
        debug!("Processing batch of {} queries", count);

        let data = pending.join("\n").into_bytes();
        let result = self.zone_sequencer.publish(data).await?;

        info!("Inscription published with tx_hash: {:?}", result.inscription_id);

        save_checkpoint(Path::new(&self.checkpoint_path), &result.checkpoint);

        Ok(())
    }

    /// Check if the queue file is empty
    pub async fn queue_is_empty(&self) -> Result<bool> {
        match fs::metadata(self.queue_file.clone()) {
            Ok(meta) => Ok(meta.len() == 0),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e.into()),
        }
    }

    /// Background processing loop - call this in a spawned task
    pub async fn run_processing_loop(&self) {
        let poll_interval = Duration::from_millis(100);

        loop {
            let is_empty = match self.queue_is_empty().await {
                Ok(empty) => empty,
                Err(e) => {
                    tracing::error!("Failed to check queue: {}", e);
                    sleep(poll_interval).await;
                    continue;
                }
            };

            if is_empty {
                sleep(poll_interval).await;
                continue;
            }

            if let Err(e) = self.process_pending_batch().await {
                tracing::error!("Batch processing failed: {}", e);
            }
        }
    }
}

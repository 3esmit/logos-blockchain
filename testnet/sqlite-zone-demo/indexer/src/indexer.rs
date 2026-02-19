use std::sync::Arc;
use tokio::sync::Mutex;

use crate::db::MenuReadOnly;
use futures::StreamExt as _;
use lb_common_http_client::BasicAuthCredentials;
use lb_core::mantle::ops::channel::ChannelId;
use logos_blockchain_zone_sdk::indexer::ZoneIndexer;
use reqwest::Url;
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("Zone indexer error: {0}")]
    ZoneIndexer(#[from] logos_blockchain_zone_sdk::indexer::Error),
    #[error("HTTP client error: {0}")]
    HttpClient(#[from] lb_common_http_client::Error),
    #[error("URL parse error: {0}")]
    Url(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    InvalidChannelId(String),
}

pub type Result<T> = std::result::Result<T, IndexerError>;

pub struct Indexer {
    zone_indexer: ZoneIndexer,
    db: Arc<Mutex<MenuReadOnly>>,
}

fn parse_channel_id(channel_id_str: &str) -> Result<ChannelId> {
    let decoded = hex::decode(channel_id_str).map_err(|_| {
        IndexerError::InvalidChannelId(format!(
            "INDEXER_CHANNEL_ID must be a valid hex string, got: '{channel_id_str}'"
        ))
    })?;
    let channel_bytes: [u8; 32] = decoded.try_into().map_err(|v: Vec<u8>| {
        IndexerError::InvalidChannelId(format!(
            "INDEXER_CHANNEL_ID must be exactly 64 hex characters (32 bytes), got {} characters ({} bytes)",
            v.len() * 2,
            v.len()
        ))
    })?;
    Ok(ChannelId::from(channel_bytes))
}

impl Indexer {
    pub fn new(
        db_path: &str,
        node_endpoint: &str,
        channel_id_str: &str,
        node_auth_username: Option<String>,
        node_auth_password: Option<String>,
    ) -> Result<Self> {
        let node_url = Url::parse(node_endpoint).map_err(|e| IndexerError::Url(e.to_string()))?;

        let basic_auth = node_auth_username
            .map(|username| BasicAuthCredentials::new(username, node_auth_password));

        let channel_id = parse_channel_id(channel_id_str)?;

        info!("Channel ID: {}", hex::encode(channel_id.as_ref()));

        let zone_indexer = ZoneIndexer::new(channel_id, node_url, basic_auth);

        let menu = MenuReadOnly::new(db_path)?;
        let db = Arc::new(Mutex::new(menu));

        Ok(Self { zone_indexer, db })
    }

    pub fn db(&self) -> Arc<Mutex<MenuReadOnly>> {
        Arc::clone(&self.db)
    }

    pub async fn run(&self) {
        loop {
            info!("Connecting to zone block stream...");
            let stream = match self.zone_indexer.follow().await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to connect to block stream: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };
            info!("Connected to zone block stream");

            futures::pin_mut!(stream);
            while let Some(zone_block) = stream.next().await {
                let sql_text = match String::from_utf8(zone_block.data) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Zone block data is not valid UTF-8: {e}");
                        continue;
                    }
                };

                let statements: Vec<&str> = sql_text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();

                if statements.is_empty() {
                    continue;
                }

                info!("Applying {} SQL statement(s)", statements.len());

                let db = self.db.lock().await;
                for stmt in &statements {
                    if let Err(e) = db.execute_batch(stmt) {
                        error!("Failed to execute SQL '{}': {e}", stmt);
                    }
                }
                info!("Applied {} statement(s)", statements.len());
            }

            error!("Zone block stream ended, reconnecting...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

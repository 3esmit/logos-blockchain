use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub node_endpoint: String,
    pub db_path: String,
    pub channel_id: String,
    pub node_auth_username: Option<String>,
    pub node_auth_password: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            listen_addr: std::env::var("INDEXER_LISTEN_ADDR")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| "0.0.0.0:8085".parse().unwrap()),
            node_endpoint: std::env::var("INDEXER_NODE_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:18081".to_owned()),
            db_path: std::env::var("INDEXER_DB_PATH")
                .unwrap_or_else(|_| "./data/indexer.db".to_owned()),
            channel_id: std::env::var("SEQUENCER_CHANNEL_ID")
                .expect("SEQUENCER_CHANNEL_ID env var is required"),
            node_auth_username: std::env::var("INDEXER_NODE_AUTH_USERNAME").ok(),
            node_auth_password: std::env::var("INDEXER_NODE_AUTH_PASSWORD").ok(),
        }
    }
}

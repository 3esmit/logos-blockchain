use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub node_endpoint: String,
    pub db_path: String,
    pub channel_id: String,
    pub node_auth_username: Option<String>,
    pub node_auth_password: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            node_endpoint: std::env::var("INDEXER_NODE_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:18081".to_owned()),
            db_path: std::env::var("INDEXER_DB_PATH")
                .unwrap_or_else(|_| "./data/indexer.db".to_owned()),
            channel_id: std::env::var("CHANNEL_ID")
                .expect("CHANNEL_ID env var is required"),
            node_auth_username: std::env::var("INDEXER_NODE_AUTH_USERNAME").ok(),
            node_auth_password: std::env::var("INDEXER_NODE_AUTH_PASSWORD").ok(),
        }
    }
}

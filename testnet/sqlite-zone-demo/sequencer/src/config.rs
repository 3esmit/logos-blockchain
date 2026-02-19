use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Logos blockchain node HTTP endpoint to submit transactions to (e.g., "<http://localhost:18080>")
    pub node_endpoint: String,
    /// Path to the rusqlite database file
    pub db_path: String,
    /// Path to the signing key file (will be created if it doesn't exist)
    pub signing_key_path: String,
    /// Channel ID for inscriptions (hex string, will be padded/truncated to 32
    /// bytes)
    pub channel_id: String,
    /// Basic auth username for node endpoint (optional)
    pub node_auth_username: Option<String>,
    /// Basic auth password for node endpoint (optional)
    pub node_auth_password: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            node_endpoint: std::env::var("SEQUENCER_NODE_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:18080".to_owned()),
            db_path: std::env::var("SEQUENCER_DB_PATH")
                .unwrap_or_else(|_| "./data/database.db".to_owned()),
            signing_key_path: std::env::var("SEQUENCER_SIGNING_KEY_PATH")
                .unwrap_or_else(|_| "./data/sequencer.key".to_owned()),
            channel_id: std::env::var("CHANNEL_ID")
                .expect("CHANNEL_ID env var is required"),
            node_auth_username: std::env::var("SEQUENCER_NODE_AUTH_USERNAME").ok(),
            node_auth_password: std::env::var("SEQUENCER_NODE_AUTH_PASSWORD").ok(),
        }
    }
}

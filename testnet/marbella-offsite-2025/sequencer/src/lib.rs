use serde::{Deserialize, Serialize};

/// Request to transfer funds between accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub from: String,
    pub to: String,
    pub amount: u64,
}

/// Transaction with unique ID for on-chain inscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
}

impl Transaction {
    /// Create a new transaction from a transfer request with a random ID
    #[must_use]
    pub fn from_transfer_request(request: &TransferRequest) -> Self {
        let mut id_bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id_bytes);
        Self {
            id: hex::encode(id_bytes),
            from: request.from.clone(),
            to: request.to.clone(),
            amount: request.amount,
        }
    }
}

/// Response after successful transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResponse {
    pub from_balance: u64,
    pub to_balance: u64,
    pub tx_hash: String,
}

/// Block data format inscribed on-chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockData {
    pub block_id: u64,
    pub transactions: Vec<Transaction>,
}

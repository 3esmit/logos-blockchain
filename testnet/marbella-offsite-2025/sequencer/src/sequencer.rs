use std::{collections::HashSet, fs, io, path::Path, time::Duration};

use common_http_client::CommonHttpClient;
use demo_sequencer::{BlockData, Transaction, TransferRequest, TransferResponse};
use key_management_system_service::keys::{ED25519_SECRET_KEY_SIZE, Ed25519Key};
use nomos_core::{
    header::HeaderId,
    mantle::{
        MantleTx, SignedMantleTx, Transaction as _,
        ledger::Tx as LedgerTx,
        ops::{
            Op, OpProof,
            channel::{ChannelId, Ed25519PublicKey, MsgId, inscribe::InscriptionOp},
        },
        tx::TxHash,
    },
};
use reqwest::Url;
use thiserror::Error;
use tokio::time::sleep;
use tracing::info;

use crate::db::AccountDb;

#[derive(Debug, Error)]
pub enum SequencerError {
    #[error("Database error: {0}")]
    Db(#[from] Box<crate::db::DbError>),
    #[error("HTTP client error: {0}")]
    Http(#[from] common_http_client::Error),
    #[error("URL parse error: {0}")]
    Url(String),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid key file: expected {expected} bytes, got {actual}")]
    InvalidKeyFile { expected: usize, actual: usize },
    #[error("Transaction not included after timeout")]
    Timeout,
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<crate::db::DbError> for SequencerError {
    fn from(err: crate::db::DbError) -> Self {
        Self::Db(Box::new(err))
    }
}

pub type Result<T> = std::result::Result<T, SequencerError>;

/// The sequencer that handles transactions
pub struct Sequencer {
    db: AccountDb,
    http_client: CommonHttpClient,
    node_url: Url,
    signing_key: Ed25519Key,
    channel_id: ChannelId,
}

fn empty_ledger_signature(tx_hash: &TxHash) -> key_management_system_service::keys::ZkSignature {
    key_management_system_service::keys::ZkKey::multi_sign(&[], tx_hash.as_ref())
        .expect("multi-sign with empty key set works")
}

/// Load signing key from file or generate a new one if it doesn't exist
fn load_or_create_signing_key(path: &Path) -> Result<Ed25519Key> {
    if path.exists() {
        info!("Loading existing signing key from {:?}", path);
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
        info!("Generating new signing key and saving to {:?}", path);
        let mut key_bytes = [0u8; ED25519_SECRET_KEY_SIZE];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_bytes);
        fs::write(path, key_bytes)?;
        Ok(Ed25519Key::from_bytes(&key_bytes))
    }
}

impl Sequencer {
    pub fn new(
        db: AccountDb,
        node_endpoint: &str,
        signing_key_path: &str,
        node_auth_username: Option<String>,
        node_auth_password: Option<String>,
    ) -> Result<Self> {
        let node_url = Url::parse(node_endpoint).map_err(|e| SequencerError::Url(e.to_string()))?;

        let basic_auth = node_auth_username.map(|username| {
            common_http_client::BasicAuthCredentials::new(username, node_auth_password)
        });
        let http_client = CommonHttpClient::new(basic_auth);

        // Load or generate the signing key
        let signing_key = load_or_create_signing_key(Path::new(signing_key_path))?;

        // Create a channel ID from the signing key's public key
        let channel_id = ChannelId::from(signing_key.public_key().to_bytes());
        info!("Sequencer channel ID: {}", hex::encode(channel_id.as_ref()));

        Ok(Self {
            db,
            http_client,
            node_url,
            signing_key,
            channel_id,
        })
    }

    /// Get the last message ID from the database, or root if not set
    async fn get_last_msg_id(&self) -> Result<MsgId> {
        (self.db.get_last_msg_id().await?)
            .map_or_else(|| Ok(MsgId::root()), |bytes| Ok(MsgId::from(bytes)))
    }

    /// Save the last message ID to the database
    async fn set_last_msg_id(&self, msg_id: MsgId) -> Result<()> {
        let bytes: [u8; 32] = msg_id.into();
        self.db.set_last_msg_id(&bytes).await?;
        Ok(())
    }

    /// Create and sign a transaction for inscribing data
    fn create_inscribe_tx(&self, data: Vec<u8>, parent: MsgId) -> SignedMantleTx {
        let verifying_key_bytes = self.signing_key.public_key().to_bytes();
        let verifying_key =
            Ed25519PublicKey::from_bytes(&verifying_key_bytes).expect("valid ed25519 public key");

        let inscribe_op = InscriptionOp {
            channel_id: self.channel_id,
            inscription: data,
            parent,
            signer: verifying_key,
        };

        let ledger_tx = LedgerTx::new(vec![], vec![]);

        let inscribe_tx = MantleTx {
            ops: vec![Op::ChannelInscribe(inscribe_op)],
            ledger_tx,
            storage_gas_price: 0,
            execution_gas_price: 0,
        };

        let tx_hash = inscribe_tx.hash();
        let signature_bytes = self
            .signing_key
            .sign_payload(tx_hash.as_signing_bytes().as_ref())
            .to_bytes();
        let signature =
            key_management_system_service::keys::Ed25519Signature::from_bytes(&signature_bytes);

        SignedMantleTx {
            ops_proofs: vec![OpProof::Ed25519Sig(signature)],
            ledger_tx_proof: empty_ledger_signature(&tx_hash),
            mantle_tx: inscribe_tx,
        }
    }

    /// Post a transaction to the node and wait for inclusion
    async fn post_and_wait(&self, tx: &SignedMantleTx) -> Result<()> {
        // Post the transaction
        self.http_client
            .post_transaction(self.node_url.clone(), tx.clone())
            .await?;

        info!("Transaction posted, waiting for inclusion...");

        // Wait for the transaction to be included
        self.wait_for_inclusion(tx).await?;

        Ok(())
    }

    /// Wait for a transaction to be included in a block.
    /// Uses `consensus_info` to get the tip, then walks back through blocks
    /// checking for the inscription.
    #[expect(
        clippy::cognitive_complexity,
        reason = "This is a demo, it is ok for now"
    )]
    async fn wait_for_inclusion(&self, tx: &SignedMantleTx) -> Result<()> {
        // Don't walk back more than 50 blocks per poll (tx should be in recent blocks)
        const MAX_DEPTH_PER_POLL: usize = 50;

        let expected_op = tx
            .mantle_tx
            .ops
            .first()
            .expect("transaction should have at least one op");

        let Op::ChannelInscribe(expected_inscription) = expected_op else {
            panic!("Expected ChannelInscribe op")
        };

        let timeout_duration = Duration::from_mins(5);
        let poll_interval = Duration::from_millis(500);
        let start = std::time::Instant::now();
        let mut checked_blocks: HashSet<HeaderId> = HashSet::new();

        tracing::debug!(
            "Waiting for inscription: channel={}, parent={}",
            hex::encode(expected_inscription.channel_id.as_ref()),
            hex::encode(<[u8; 32]>::from(expected_inscription.parent))
        );

        while start.elapsed() < timeout_duration {
            // Get current consensus info
            let info = self
                .http_client
                .consensus_info(self.node_url.clone())
                .await?;
            let mut current_id = Some(info.tip);

            tracing::debug!(
                "Polling: tip={}, height={}, checked_blocks={}",
                info.tip,
                info.height,
                checked_blocks.len()
            );

            // Walk back from tip, checking any blocks we haven't seen yet
            let mut depth = 0;
            while let Some(block_id) = current_id {
                if checked_blocks.contains(&block_id) {
                    break; // Already checked this block and its ancestors
                }

                if depth >= MAX_DEPTH_PER_POLL {
                    tracing::debug!("Reached max depth {}, will continue next poll", depth);
                    break;
                }

                if let Some(block) = self
                    .http_client
                    .get_block(self.node_url.clone(), block_id)
                    .await?
                {
                    checked_blocks.insert(block_id);
                    depth += 1;
                    let tx_count = block.transactions().len();

                    tracing::debug!(
                        "Checking block {} (depth {}): {} transactions",
                        block_id,
                        depth,
                        tx_count
                    );

                    for tx in block.transactions() {
                        for op in &tx.mantle_tx.ops {
                            if let Op::ChannelInscribe(inscribe) = op {
                                tracing::debug!(
                                    "Found inscription: channel={}, parent={}",
                                    hex::encode(inscribe.channel_id.as_ref()),
                                    hex::encode(<[u8; 32]>::from(inscribe.parent))
                                );

                                if inscribe.inscription == expected_inscription.inscription
                                    && inscribe.channel_id == expected_inscription.channel_id
                                    && inscribe.parent == expected_inscription.parent
                                {
                                    info!("Transaction included in block {}", block_id);
                                    return Ok(());
                                }
                            }
                        }
                    }

                    current_id = Some(block.header().parent());
                } else {
                    tracing::debug!("Block {} not found", block_id);
                    break;
                }
            }

            sleep(poll_interval).await;
        }

        tracing::warn!(
            "Timeout waiting for inscription after {:?}, checked {} blocks",
            timeout_duration,
            checked_blocks.len()
        );
        Err(SequencerError::Timeout)
    }

    /// Process a transfer request
    #[expect(
        clippy::cognitive_complexity,
        reason = "this is a demo, it is ok for now"
    )]
    pub async fn process_transfer(&self, request: TransferRequest) -> Result<TransferResponse> {
        info!(
            "Processing transfer: {} -> {} (amount: {})",
            request.from, request.to, request.amount
        );

        // Validate and update balances in the database
        let (from_balance, to_balance) = self
            .db
            .transfer(&request.from, &request.to, request.amount)
            .await?;

        // Get next block ID
        let block_id = self.db.next_block_id().await?;

        // Create transaction with random ID from the transfer request
        let transaction = Transaction::from_transfer_request(&request);

        // Create block data with block_id and transactions array
        let block_data = BlockData {
            block_id,
            transactions: vec![transaction],
        };

        // Serialize block data for inscription
        let inscription_data = serde_json::to_vec(&block_data)
            .map_err(|e| SequencerError::Serialization(e.to_string()))?;

        info!(
            "Posting block data: {}",
            serde_json::to_string(&block_data).unwrap_or_else(|_| "<serialization error>".into())
        );

        // Get the current parent message ID from database
        let parent = self.get_last_msg_id().await?;

        // Create and sign the transaction
        let tx = self.create_inscribe_tx(inscription_data, parent);

        // Calculate the new message ID (from the inscription operation)
        let new_msg_id = match tx.mantle_tx.ops.first() {
            Some(Op::ChannelInscribe(inscribe)) => inscribe.id(),
            _ => panic!("Expected ChannelInscribe op"),
        };

        let tx_hash = format!("{:?}", tx.mantle_tx.hash());

        // Post and wait for inclusion - revert transfer if it fails
        if let Err(e) = self.post_and_wait(&tx).await {
            // Revert the transfer by doing the opposite
            if let Err(revert_err) = self
                .db
                .transfer(&request.to, &request.from, request.amount)
                .await
            {
                tracing::error!(
                    "Failed to revert transfer after post failure: {}",
                    revert_err
                );
            } else {
                info!(
                    "Reverted transfer {} -> {} (amount: {}) after post failure",
                    request.from, request.to, request.amount
                );
            }
            return Err(e);
        }

        // Update the last message ID in database
        self.set_last_msg_id(new_msg_id).await?;

        info!(
            "Transfer complete: {} -> {}, new balances: {} / {}",
            request.from, request.to, from_balance, to_balance
        );

        Ok(TransferResponse {
            from_balance,
            to_balance,
            tx_hash,
        })
    }

    /// Get the balance of an account
    pub async fn get_balance(&self, account: &str) -> Result<u64> {
        Ok(self.db.get_or_create_balance(account).await?)
    }

    /// List all accounts
    pub async fn list_accounts(&self) -> Result<Vec<(String, u64)>> {
        Ok(self.db.list_accounts().await?)
    }
}

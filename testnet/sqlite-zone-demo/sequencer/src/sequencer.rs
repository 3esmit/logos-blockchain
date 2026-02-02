use std::{collections::HashSet, fs::OpenOptions, io::{BufRead, BufReader}, path::Path, time::Duration, io, fs};
use fs2::FileExt;

use lb_common_http_client::CommonHttpClient;
use demo_sqlite_sequencer::db::MessageIdTable;
use lb_key_management_system_service::keys::{ED25519_SECRET_KEY_SIZE, Ed25519Key};
use lb_core::{
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
use tracing::{debug, info, warn};

use rusqlite::Error as SqliteError;

#[derive(Debug, Error)]
pub enum SequencerError {
    #[error("HTTP client error: {0}")]
    Http(#[from] lb_common_http_client::Error),
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
    #[error("Transaction not included after timeout")]
    Timeout,
    #[error("UTF8 conversion error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

pub type Result<T> = std::result::Result<T, SequencerError>;

/// The sequencer that handles transactions
pub struct Sequencer {
    db_path: String,
    state_db: MessageIdTable,
    http_client: CommonHttpClient,
    node_url: Url,
    signing_key: Ed25519Key,
    channel_id: ChannelId,
    queue_file: String,
}

const MAX_DEPTH_PER_POLL: usize = 50;

fn empty_ledger_signature(tx_hash: &TxHash) -> lb_key_management_system_service::keys::ZkSignature {
    lb_key_management_system_service::keys::ZkKey::multi_sign(&[], tx_hash.as_ref())
        .expect("multi-sign with empty key set works")
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

impl Sequencer {
    pub fn new(
        db_path: &str,
        state_db: MessageIdTable,
        node_endpoint: &str,
        signing_key_path: &str,
        channel_id_str: &str,
        node_auth_username: Option<String>,
        node_auth_password: Option<String>,
        queue_file: &str,
    ) -> Result<Self> {
        let node_url = Url::parse(node_endpoint).map_err(|e| SequencerError::Url(e.to_string()))?;

        let basic_auth = node_auth_username.map(|username| {
            lb_common_http_client::BasicAuthCredentials::new(username, node_auth_password)
        });
        let http_client = CommonHttpClient::new(basic_auth);

        // Load or generate the signing key
        let signing_key = load_or_create_signing_key(Path::new(signing_key_path))?;

        // Decode channel ID from 64-character hex string (32 bytes)
        let decoded = hex::decode(channel_id_str).map_err(|_| {
            SequencerError::InvalidChannelId(format!(
                "SEQUENCER_CHANNEL_ID must be a valid hex string, got: '{channel_id_str}'"
            ))
        })?;
        let channel_bytes: [u8; 32] = decoded.try_into().map_err(|v: Vec<u8>| {
            SequencerError::InvalidChannelId(format!(
                "SEQUENCER_CHANNEL_ID must be exactly 64 hex characters (32 bytes), got {} characters ({} bytes)",
                v.len() * 2,
                v.len()
            ))
        })?;
        let channel_id = ChannelId::from(channel_bytes);
        info!("Channel ID: {}", hex::encode(channel_id.as_ref()));

        Ok(Self {
            db_path: db_path.to_string(),
            state_db,
            http_client,
            node_url,
            signing_key,
            channel_id,
            queue_file: queue_file.to_string(),
        })
    }

    /// Get the last message ID from the database, or root if not set
    async fn get_last_msg_id(&self) -> Result<MsgId> {
        (self.state_db.get_last_msg_id().await?)
            .map_or_else(|| Ok(MsgId::root()), |bytes| Ok(MsgId::from(bytes)))
    }

    /// Save the last message ID to the database
    async fn set_last_msg_id(&self, msg_id: MsgId) -> Result<()> {
        let bytes: [u8; 32] = msg_id.into();
        self.state_db.set_last_msg_id(&bytes).await?;
        Ok(())
    }

    fn block_contains_inscription(
        block: &lb_core::block::Block<SignedMantleTx>,
        expected: &InscriptionOp,
        block_id: HeaderId,
    ) -> bool {
        for tx in block.transactions() {
            for op in &tx.mantle_tx.ops {
                if let Op::ChannelInscribe(inscribe) = op {
                    tracing::debug!(
                        "Found inscription: channel={}, parent={}",
                        hex::encode(inscribe.channel_id.as_ref()),
                        hex::encode(<[u8; 32]>::from(inscribe.parent))
                    );

                    if inscribe.inscription == expected.inscription
                        && inscribe.channel_id == expected.channel_id
                        && inscribe.parent == expected.parent
                    {
                        debug!("Transaction included in block {}", block_id);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Walk back from tip checking blocks for the expected inscription
    async fn check_blocks_for_inscription(
        &self,
        expected: &InscriptionOp,
        checked_blocks: &mut HashSet<HeaderId>,
        tip: HeaderId,
    ) -> Result<bool> {
        let mut current_id = Some(tip);
        let mut depth = 0;

        while let Some(block_id) = current_id {
            if checked_blocks.contains(&block_id) || depth >= MAX_DEPTH_PER_POLL {
                break;
            }

            let Some(block) = self
                .http_client
                .get_block(self.node_url.clone(), block_id)
                .await?
            else {
                break;
            };

            checked_blocks.insert(block_id);
            depth += 1;

            tracing::debug!(
                "Checking block {} (depth {}): {} transactions",
                block_id,
                depth,
                block.transactions().len()
            );

            if Self::block_contains_inscription(&block, expected, block_id) {
                return Ok(true);
            }

            current_id = Some(block.header().parent());
        }

        Ok(false)
    }

    fn get_expected_inscription(tx: &SignedMantleTx) -> &InscriptionOp {
        let expected_op = tx
            .mantle_tx
            .ops
            .first()
            .expect("transaction should have at least one op");

        let Op::ChannelInscribe(expected_inscription) = expected_op else {
            panic!("Expected ChannelInscribe op")
        };

        expected_inscription
    }

    async fn poll_for_inclusion(
        &self,
        expected: &InscriptionOp,
        checked_blocks: &mut HashSet<HeaderId>,
    ) -> Result<bool> {
        let info = self
            .http_client
            .consensus_info(self.node_url.clone())
            .await?;

        tracing::debug!(
            "Polling: tip={}, height={}, checked_blocks={}",
            info.tip,
            info.height,
            checked_blocks.len()
        );

        self.check_blocks_for_inscription(expected, checked_blocks, info.tip)
            .await
    }

    /// Wait for a transaction to be included in a block.
    async fn wait_for_inclusion(&self, tx: &SignedMantleTx) -> Result<()> {
        let expected_inscription = Self::get_expected_inscription(tx);

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
            if self
                .poll_for_inclusion(expected_inscription, &mut checked_blocks)
                .await?
            {
                return Ok(());
            }
            sleep(poll_interval).await;
        }

        warn!(
            "Timeout waiting for chain inclusion after {:?}",
            timeout_duration
        );
        Err(SequencerError::Timeout)
    }

    /// Post a transaction to the node and wait for inclusion
    async fn post_and_wait(&self, tx: &SignedMantleTx) -> Result<()> {
        // Post the transaction
        self.http_client
            .post_transaction(self.node_url.clone(), tx.clone())
            .await?;

        debug!("Update posted, waiting for inclusion...");

        // Wait for the transaction to be included
        self.wait_for_inclusion(tx).await?;

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
            lb_key_management_system_service::keys::Ed25519Signature::from_bytes(&signature_bytes);

        SignedMantleTx {
            ops_proofs: vec![OpProof::Ed25519Sig(signature)],
            ledger_tx_proof: empty_ledger_signature(&tx_hash),
            mantle_tx: inscribe_tx,
        }
    }

    async fn create_and_post_block(
        &self,
        queries: Vec<String>,
    ) -> Result<MsgId> {
        let parent = self.get_last_msg_id().await?;

        let tx = self.create_inscribe_tx(
            queries.join("\n").into_bytes(),
            parent);

        let new_msg_id = match tx.mantle_tx.ops.first() {
            Some(Op::ChannelInscribe(inscribe)) => inscribe.id(),
            _ => panic!("Expected ChannelInscribe op"),
        };

        self.post_and_wait(&tx).await?;

        Ok(new_msg_id)
    }

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

        return Ok(queue_vec)
    }

    /// Process all pending queries as a single block
    async fn process_pending_batch(&self) -> Result<()> {
        let pending = self.queue_drain().await?;
        if pending.is_empty() {
            return Ok(());
        }

        let count = pending.len();
        debug!("Processing batch of {} queries", count);

        match self.create_and_post_block(pending).await {
            Ok(new_msg_id) => {
                self.set_last_msg_id(new_msg_id).await?;
                info!(
                    "Message confirmed on chain, ID: {}", str::from_utf8(&<[u8; 32]>::from(new_msg_id))?);
                Ok(())
            }
            Err(e) => {return Err(e)}
        }
    }

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
            // Check if there are pending transfers
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

            // Drain and process all pending transfers
            if let Err(e) = self.process_pending_batch().await {
                tracing::error!("Batch processing failed: {}", e);
            }
        }
    }
}

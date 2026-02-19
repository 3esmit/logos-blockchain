//! Write-Ahead Log (WAL) for sequencer event history.
//!
//! This module provides an append-only event log for audit, debugging, and
//! replay purposes. Uses NDJSON (newline-delimited JSON) format for:
//! - Easy debugging (`tail -f`, `grep`)
//! - Loki/Grafana ingestion
//! - Schema evolution (add fields safely)
//! - Corruption isolation (only affects last line)
//!
//! Note: WAL is for audit/replay, not recovery. Use checkpoints for recovery.
//!
//! # Configuration
//!
//! WAL is disabled by default. To enable, set `enabled: true` and provide a
//! `dir`:
//!
//! ```yaml
//! wal:
//!   enabled: true
//!   dir: "/var/lib/sequencer/wal"
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // At startup - validate config (fails fast if invalid)
//! config.wal.validate()?;
//!
//! // Open WAL (returns None if disabled)
//! let wal = config.wal.open()?;
//! ```

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Current WAL format version.
const WAL_VERSION: u8 = 1;

/// WAL configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WalConfig {
    /// Enable WAL (default: false).
    #[serde(default)]
    pub enabled: bool,

    /// Directory for WAL files. Required if `enabled` is true.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

/// WAL configuration error.
#[derive(Debug, thiserror::Error)]
pub enum WalConfigError {
    #[error("WAL enabled but 'dir' not specified")]
    MissingDir,
}

impl WalConfig {
    /// Validate config at startup. Returns error if invalid.
    ///
    /// Call this before `open()` to fail fast on misconfiguration.
    pub const fn validate(&self) -> Result<(), WalConfigError> {
        if self.enabled && self.dir.is_none() {
            return Err(WalConfigError::MissingDir);
        }
        Ok(())
    }

    /// Create WAL if enabled, None if disabled.
    ///
    /// # Panics
    ///
    /// Panics if `enabled` is true but `dir` is None. Call `validate()` first.
    pub fn open(&self) -> Result<Option<Wal>, WalError> {
        if !self.enabled {
            return Ok(None);
        }

        let dir = self
            .dir
            .as_ref()
            .expect("validate() should have been called - WAL enabled but dir is None");

        std::fs::create_dir_all(dir)?;
        let path = dir.join("sequencer.wal");
        Ok(Some(Wal::open(path)?))
    }
}

/// A single WAL entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Format version for forward compatibility.
    pub v: u8,
    /// Monotonic sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub ts: u64,
    /// The event payload.
    #[serde(flatten)]
    pub event: WalEvent,
}

/// WAL event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WalEvent {
    /// Transaction was created and published to the network.
    TxPublished {
        /// Channel ID (hex).
        channel_id: String,
        /// Transaction hash (hex).
        tx_hash: String,
        /// Message ID for this inscription (hex).
        msg_id: String,
        /// Parent message ID (hex).
        parent_msg_id: String,
        /// Size of signed transaction in bytes.
        tx_size: usize,
    },
    /// Transaction was observed as finalized (reached LIB).
    TxFinalized {
        /// Transaction hash (hex).
        tx_hash: String,
        /// L1 block ID where finalized (hex).
        l1_block_id: String,
        /// L1 slot number.
        l1_slot: u64,
    },
}

/// WAL writer for appending events.
pub struct Wal {
    file: File,
    seq: AtomicU64,
}

/// Error type for WAL operations.
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid WAL entry at line {line}: {reason}")]
    InvalidEntry { line: usize, reason: String },
}

impl Wal {
    /// Open or create a WAL file.
    ///
    /// If the file exists, reads it to determine the next sequence number.
    /// If the file doesn't exist, creates it and starts from seq=1.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let path = path.as_ref();

        // Determine starting sequence number by reading existing entries
        let starting_seq = if path.exists() {
            Self::find_last_seq(path)?.map_or(0, |s| s)
        } else {
            0
        };

        let file = OpenOptions::new().create(true).append(true).open(path)?;

        Ok(Self {
            file,
            seq: AtomicU64::new(starting_seq + 1),
        })
    }

    /// Find the last sequence number in an existing WAL file.
    fn find_last_seq(path: impl AsRef<Path>) -> Result<Option<u64>, WalError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut last_seq = None;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<WalEntry>(&line) {
                last_seq = Some(entry.seq);
            }
        }

        Ok(last_seq)
    }

    /// Get current timestamp in milliseconds.
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Append an event to the WAL.
    pub fn append(&mut self, event: WalEvent) -> Result<u64, WalError> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let entry = WalEntry {
            v: WAL_VERSION,
            seq,
            ts: Self::now_ms(),
            event,
        };

        let json = serde_json::to_string(&entry)?;
        writeln!(self.file, "{json}")?;
        self.file.flush()?;

        Ok(seq)
    }

    /// Log a published transaction.
    pub fn log_published(
        &mut self,
        channel_id: &[u8],
        tx_hash: &[u8],
        msg_id: &[u8],
        parent_msg_id: &[u8],
        tx_size: usize,
    ) -> Result<u64, WalError> {
        self.append(WalEvent::TxPublished {
            channel_id: hex::encode(channel_id),
            tx_hash: hex::encode(tx_hash),
            msg_id: hex::encode(msg_id),
            parent_msg_id: hex::encode(parent_msg_id),
            tx_size,
        })
    }

    /// Log a finalized transaction.
    pub fn log_finalized(
        &mut self,
        tx_hash: &[u8],
        l1_block_id: &[u8],
        l1_slot: u64,
    ) -> Result<u64, WalError> {
        self.append(WalEvent::TxFinalized {
            tx_hash: hex::encode(tx_hash),
            l1_block_id: hex::encode(l1_block_id),
            l1_slot,
        })
    }

    /// Sync the WAL to disk.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Get the next sequence number that will be used.
    pub fn next_seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }
}

/// WAL reader for replay.
pub struct WalReader {
    reader: BufReader<File>,
    line_number: usize,
}

impl WalReader {
    /// Open a WAL file for reading.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            line_number: 0,
        })
    }

    /// Read entries starting from a specific sequence number.
    pub fn read_from_seq(path: impl AsRef<Path>, from_seq: u64) -> Result<Vec<WalEntry>, WalError> {
        let reader = Self::open(path)?;
        reader
            .into_iter()
            .filter(|r| r.as_ref().map_or(true, |e| e.seq >= from_seq))
            .collect()
    }
}

impl Iterator for WalReader {
    type Item = Result<WalEntry, WalError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None, // EOF
                Ok(_) => {
                    self.line_number += 1;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_str(trimmed).map_err(|e| {
                        WalError::InvalidEntry {
                            line: self.line_number,
                            reason: e.to_string(),
                        }
                    }));
                }
                Err(e) => return Some(Err(WalError::Io(e))),
            }
        }
    }
}

/// Replay statistics.
#[derive(Debug, Default)]
pub struct ReplayStats {
    pub total_entries: usize,
    pub published_count: usize,
    pub finalized_count: usize,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
}

impl ReplayStats {
    /// Compute stats from a WAL file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, WalError> {
        let reader = WalReader::open(path)?;
        let mut stats = Self::default();

        for entry in reader {
            let entry = entry?;
            stats.total_entries += 1;

            match &entry.event {
                WalEvent::TxPublished { .. } => stats.published_count += 1,
                WalEvent::TxFinalized { .. } => stats.finalized_count += 1,
            }

            if stats.first_seq.is_none() {
                stats.first_seq = Some(entry.seq);
                stats.first_ts = Some(entry.ts);
            }
            stats.last_seq = Some(entry.seq);
            stats.last_ts = Some(entry.ts);
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_append_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        // Write some entries
        {
            let mut wal = Wal::open(&path).unwrap();
            wal.log_published(b"chan1", b"tx1", b"msg1", b"parent1", 100)
                .unwrap();
            wal.log_published(b"chan1", b"tx2", b"msg2", b"msg1", 150)
                .unwrap();
            wal.log_finalized(b"tx1", b"block1", 12345).unwrap();
        }

        // Read them back
        let reader = WalReader::open(&path).unwrap();
        let entries: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[2].seq, 3);

        match &entries[0].event {
            WalEvent::TxPublished { tx_hash, .. } => {
                assert_eq!(tx_hash, &hex::encode(b"tx1"));
            }
            _ => panic!("expected TxPublished"),
        }

        match &entries[2].event {
            WalEvent::TxFinalized { l1_slot, .. } => {
                assert_eq!(*l1_slot, 12345);
            }
            _ => panic!("expected TxFinalized"),
        }
    }

    #[test]
    fn test_reopen_continues_seq() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        // Write first batch
        {
            let mut wal = Wal::open(&path).unwrap();
            wal.log_published(b"chan", b"tx1", b"msg1", b"p", 100)
                .unwrap();
            wal.log_published(b"chan", b"tx2", b"msg2", b"msg1", 100)
                .unwrap();
            assert_eq!(wal.next_seq(), 3);
        }

        // Reopen and continue
        {
            let mut wal = Wal::open(&path).unwrap();
            assert_eq!(wal.next_seq(), 3);
            wal.log_published(b"chan", b"tx3", b"msg3", b"msg2", 100)
                .unwrap();
            assert_eq!(wal.next_seq(), 4);
        }

        // Verify all entries
        let reader = WalReader::open(&path).unwrap();
        let entries: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].seq, 3);
    }

    #[test]
    fn test_read_from_seq() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::open(&path).unwrap();
            for i in 0..5 {
                wal.log_published(
                    b"chan",
                    format!("tx{i}").as_bytes(),
                    format!("msg{i}").as_bytes(),
                    b"p",
                    100,
                )
                .unwrap();
            }
        }

        let entries = WalReader::read_from_seq(&path, 3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 3);
        assert_eq!(entries[1].seq, 4);
        assert_eq!(entries[2].seq, 5);
    }

    #[test]
    fn test_replay_stats() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::open(&path).unwrap();
            wal.log_published(b"c", b"tx1", b"m1", b"p", 100).unwrap();
            wal.log_published(b"c", b"tx2", b"m2", b"m1", 100).unwrap();
            wal.log_finalized(b"tx1", b"b1", 100).unwrap();
            wal.log_published(b"c", b"tx3", b"m3", b"m2", 100).unwrap();
            wal.log_finalized(b"tx2", b"b2", 200).unwrap();
        }

        let stats = ReplayStats::from_file(&path).unwrap();
        assert_eq!(stats.total_entries, 5);
        assert_eq!(stats.published_count, 3);
        assert_eq!(stats.finalized_count, 2);
        assert_eq!(stats.first_seq, Some(1));
        assert_eq!(stats.last_seq, Some(5));
    }

    #[test]
    fn test_json_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        {
            let mut wal = Wal::open(&path).unwrap();
            wal.log_published(b"\x01\x02", b"\xaa\xbb", b"\xcc\xdd", b"\xee\xff", 42)
                .unwrap();
        }

        // Read raw file and verify JSON structure
        let content = std::fs::read_to_string(&path).unwrap();
        let line = content.lines().next().unwrap();

        // Should be valid JSON with expected fields
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["seq"], 1);
        assert_eq!(parsed["event"], "tx_published");
        assert_eq!(parsed["channel_id"], "0102");
        assert_eq!(parsed["tx_hash"], "aabb");
        assert_eq!(parsed["tx_size"], 42);
    }

    // Config tests

    #[test]
    fn test_config_disabled_is_valid() {
        let config = WalConfig {
            enabled: false,
            dir: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_enabled_with_dir_is_valid() {
        let config = WalConfig {
            enabled: true,
            dir: Some(PathBuf::from("/tmp/wal")),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_enabled_without_dir_is_error() {
        let config = WalConfig {
            enabled: true,
            dir: None,
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, WalConfigError::MissingDir));
        assert_eq!(err.to_string(), "WAL enabled but 'dir' not specified");
    }

    #[test]
    fn test_config_open_disabled_returns_none() {
        let config = WalConfig {
            enabled: false,
            dir: None,
        };
        config.validate().unwrap();
        let wal = config.open().unwrap();
        assert!(wal.is_none());
    }

    #[test]
    fn test_config_open_enabled_returns_wal() {
        let dir = tempdir().unwrap();
        let config = WalConfig {
            enabled: true,
            dir: Some(dir.path().to_path_buf()),
        };
        config.validate().unwrap();
        let wal = config.open().unwrap();
        assert!(wal.is_some());

        // Verify file was created
        assert!(dir.path().join("sequencer.wal").exists());
    }

    #[test]
    fn test_config_open_creates_dir() {
        let dir = tempdir().unwrap();
        let wal_dir = dir.path().join("nested").join("wal");

        let config = WalConfig {
            enabled: true,
            dir: Some(wal_dir.clone()),
        };
        config.validate().unwrap();
        config.open().unwrap();

        // Verify nested dir was created
        assert!(wal_dir.join("sequencer.wal").exists());
    }
}

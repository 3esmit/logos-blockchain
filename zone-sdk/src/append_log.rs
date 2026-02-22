//! Append-only event log (journal) for sequencer event history.
//!
//! This module provides an append-only event log for audit, debugging, and
//! replay purposes. Uses NDJSON (newline-delimited JSON) format.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead as _, BufReader, BufWriter, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Current log format version.
const LOG_VERSION: u8 = 1;

/// Journal configuration (user-facing).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JournalConfig {
    /// Directory for journal files. Defaults to current directory.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

impl JournalConfig {
    /// Open the append log.
    pub fn open(&self) -> Result<AppendLog, AppendLogError> {
        let dir = self.dir.clone().unwrap_or_else(|| PathBuf::from("."));

        std::fs::create_dir_all(&dir)?;
        let path = dir.join("sequencer.journal");
        AppendLog::open(path)
    }
}

/// A single log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendLogEntry {
    /// Format version for forward compatibility.
    pub v: u8,
    /// Monotonic sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub ts: u64,
    /// The event payload.
    #[serde(flatten)]
    pub event: AppendLogEvent,
}

/// Log event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AppendLogEvent {
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

/// Append-only log writer.
pub struct AppendLog {
    writer: BufWriter<File>,
    seq: u64,
}

/// Error type for append log operations.
#[derive(Debug, thiserror::Error)]
pub enum AppendLogError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid log entry at line {line}: {reason}")]
    InvalidEntry { line: usize, reason: String },
}

impl AppendLog {
    /// Open or create a log file.
    ///
    /// If the file exists, reads it to determine the next sequence number.
    /// If the file doesn't exist, creates it and starts from seq=1.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppendLogError> {
        let path = path.as_ref();

        // Determine starting sequence number by reading existing entries
        let starting_seq = if path.exists() {
            Self::find_last_seq(path)?.unwrap_or(0)
        } else {
            0
        };

        let file = OpenOptions::new().create(true).append(true).open(path)?;

        Ok(Self {
            writer: BufWriter::new(file),
            seq: starting_seq + 1,
        })
    }

    /// Find the last sequence number in an existing log file.
    ///
    /// Corrupted or malformed entries are skipped with a warning. This allows
    /// recovery from partial writes (crash mid-write) without losing data.
    fn find_last_seq(path: impl AsRef<Path>) -> Result<Option<u64>, AppendLogError> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);
        let mut last_seq = None;
        let mut corrupted_count = 0;

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<AppendLogEntry>(&line) {
                Ok(entry) => {
                    last_seq = Some(entry.seq);
                }
                Err(e) => {
                    corrupted_count += 1;
                    tracing::warn!(
                        "journal corruption at line {}: {} (file: {})",
                        line_num + 1,
                        e,
                        path.as_ref().display()
                    );
                }
            }
        }

        if corrupted_count > 0 {
            // TODO: implement journal repair to clean up corrupted entries
            tracing::warn!(
                "journal has {} corrupted entries that were skipped",
                corrupted_count
            );
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

    /// Append an event to the log.
    ///
    /// Flushes data to the OS after each write for durability.
    pub fn append(&mut self, event: AppendLogEvent) -> Result<u64, AppendLogError> {
        let seq = self.seq;
        self.seq += 1;

        let entry = AppendLogEntry {
            v: LOG_VERSION,
            seq,
            ts: Self::now_ms(),
            event,
        };

        let json = serde_json::to_string(&entry)?;
        writeln!(self.writer, "{json}")?;
        self.writer.flush()?;

        Ok(seq)
    }

    /// Flush buffered data to the OS.
    pub fn flush(&mut self) -> Result<(), AppendLogError> {
        self.writer.flush()?;
        Ok(())
    }

    /// Log a published transaction.
    pub fn log_published(
        &mut self,
        channel_id: &[u8],
        tx_hash: &[u8],
        msg_id: &[u8],
        parent_msg_id: &[u8],
        tx_size: usize,
    ) -> Result<u64, AppendLogError> {
        self.append(AppendLogEvent::TxPublished {
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
    ) -> Result<u64, AppendLogError> {
        self.append(AppendLogEvent::TxFinalized {
            tx_hash: hex::encode(tx_hash),
            l1_block_id: hex::encode(l1_block_id),
            l1_slot,
        })
    }

    /// Sync the log to disk (flush buffer + fsync).
    pub fn sync(&mut self) -> Result<(), AppendLogError> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        Ok(())
    }

    /// Get the next sequence number that will be used.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.seq
    }
}

impl Drop for AppendLog {
    fn drop(&mut self) {
        // Best-effort flush on drop
        drop(self.writer.flush());
    }
}

/// Log reader for replay.
pub struct AppendLogReader {
    reader: BufReader<File>,
    line_number: usize,
}

impl AppendLogReader {
    /// Open a log file for reading.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppendLogError> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            line_number: 0,
        })
    }

    /// Return a streaming iterator starting from a specific sequence number.
    ///
    /// This is the preferred method for replaying log entries as it doesn't
    /// load everything into memory.
    pub fn from_seq(
        self,
        from_seq: u64,
    ) -> impl Iterator<Item = Result<AppendLogEntry, AppendLogError>> {
        self.filter(move |r| r.as_ref().map_or(true, |e| e.seq >= from_seq))
    }

    /// Read all entries starting from a specific sequence number into a Vec.
    ///
    /// Convenience method for tests. For production replay, use `from_seq()`
    /// which returns a streaming iterator.
    #[cfg(test)]
    pub fn read_from_seq(
        path: impl AsRef<Path>,
        from_seq: u64,
    ) -> Result<Vec<AppendLogEntry>, AppendLogError> {
        let reader = Self::open(path)?;
        reader.from_seq(from_seq).collect()
    }
}

impl Iterator for AppendLogReader {
    type Item = Result<AppendLogEntry, AppendLogError>;

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
                    match serde_json::from_str(trimmed) {
                        Ok(entry) => return Some(Ok(entry)),
                        Err(e) => {
                            // TODO: implement journal repair to clean up corrupted entries
                            tracing::warn!(
                                "journal corruption at line {}: {} - skipping entry",
                                self.line_number,
                                e
                            );
                        }
                    }
                }
                Err(e) => return Some(Err(AppendLogError::Io(e))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_append_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.journal");

        // Write some entries
        let mut log = AppendLog::open(&path).unwrap();
        log.log_published(b"chan1", b"tx1", b"msg1", b"parent1", 100)
            .unwrap();
        log.log_published(b"chan1", b"tx2", b"msg2", b"msg1", 150)
            .unwrap();
        log.log_finalized(b"tx1", b"block1", 12345).unwrap();
        drop(log);

        // Read them back
        let reader = AppendLogReader::open(&path).unwrap();
        let entries: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[2].seq, 3);

        match &entries[0].event {
            AppendLogEvent::TxPublished { tx_hash, .. } => {
                assert_eq!(tx_hash, &hex::encode(b"tx1"));
            }
            AppendLogEvent::TxFinalized { .. } => panic!("expected TxPublished"),
        }

        match &entries[2].event {
            AppendLogEvent::TxFinalized { l1_slot, .. } => {
                assert_eq!(*l1_slot, 12345);
            }
            AppendLogEvent::TxPublished { .. } => panic!("expected TxFinalized"),
        }
    }

    #[test]
    fn test_reopen_continues_seq() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.journal");

        // Write first batch
        let mut log = AppendLog::open(&path).unwrap();
        log.log_published(b"chan", b"tx1", b"msg1", b"p", 100)
            .unwrap();
        log.log_published(b"chan", b"tx2", b"msg2", b"msg1", 100)
            .unwrap();
        assert_eq!(log.next_seq(), 3);
        drop(log);

        // Reopen and continue
        let mut log = AppendLog::open(&path).unwrap();
        assert_eq!(log.next_seq(), 3);
        log.log_published(b"chan", b"tx3", b"msg3", b"msg2", 100)
            .unwrap();
        assert_eq!(log.next_seq(), 4);
        drop(log);

        // Verify all entries
        let reader = AppendLogReader::open(&path).unwrap();
        let entries: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].seq, 3);
    }

    #[test]
    fn test_read_from_seq() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.journal");

        {
            let mut log = AppendLog::open(&path).unwrap();
            for i in 0..5 {
                log.log_published(
                    b"chan",
                    format!("tx{i}").as_bytes(),
                    format!("msg{i}").as_bytes(),
                    b"p",
                    100,
                )
                .unwrap();
            }
        }

        let entries = AppendLogReader::read_from_seq(&path, 3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 3);
        assert_eq!(entries[1].seq, 4);
        assert_eq!(entries[2].seq, 5);
    }

    #[test]
    fn test_json_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.journal");

        let mut log = AppendLog::open(&path).unwrap();
        log.log_published(b"\x01\x02", b"\xaa\xbb", b"\xcc\xdd", b"\xee\xff", 42)
            .unwrap();
        drop(log);

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
    fn test_config_default_dir_uses_current_dir() {
        // Use a temp dir as working directory to avoid polluting the repo
        let temp = tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let config = JournalConfig { dir: None };
        let _log = config.open().unwrap();

        // Verify file was created in current dir
        assert!(temp.path().join("sequencer.journal").exists());

        // Restore original dir
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_config_custom_dir() {
        let dir = tempdir().unwrap();
        let config = JournalConfig {
            dir: Some(dir.path().to_path_buf()),
        };
        let _log = config.open().unwrap();

        // Verify file was created
        assert!(dir.path().join("sequencer.journal").exists());
    }

    #[test]
    fn test_config_creates_nested_dir() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("nested").join("journal");

        let config = JournalConfig {
            dir: Some(journal_dir.clone()),
        };
        config.open().unwrap();

        // Verify nested dir was created
        assert!(journal_dir.join("sequencer.journal").exists());
    }
}

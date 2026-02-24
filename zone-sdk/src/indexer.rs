use std::collections::HashSet;

use futures::{Stream, StreamExt as _, stream};
use lb_common_http_client::{BasicAuthCredentials, CommonHttpClient, ProcessedBlockEvent};
use lb_core::{
    header::HeaderId,
    mantle::ops::{
        Op,
        channel::{ChannelId, MsgId},
    },
};
use reqwest::Url;
use tracing::warn;

use crate::{BlockStatus, ZoneBlock, ZoneBlockWithStatus};

/// Indexer errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] lb_common_http_client::Error),
}

/// Cursor for resuming message streaming. Persist this with your checkpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Slot of the last processed block.
    pub slot: u64,
    /// Last message ID within the slot (for mid-slot resumption).
    pub last_id: Option<MsgId>,
}

/// Result of polling for messages.
pub struct PollResult {
    /// Messages found.
    pub messages: Vec<ZoneBlock>,
    /// Cursor to pass to the next `next_messages` call.
    pub cursor: Cursor,
}

/// Zone indexer — reads finalized zone messages from a channel.
pub struct ZoneIndexer {
    channel_id: ChannelId,
    node_url: Url,
    http_client: CommonHttpClient,
}

const BATCH_SIZE: u64 = 100;

/// Extract zone blocks from a processed block event (live stream).
///
/// All blocks from the live stream are marked as `Safe` since they are
/// above the LIB (Last Irreversible Block). Finalized status is only
/// assigned during backfill where we read from the finalized chain.
fn extract_zone_blocks(
    channel_id: ChannelId,
    event: &ProcessedBlockEvent,
) -> Vec<ZoneBlockWithStatus> {
    let block_id = event.block.header.id;
    let block_slot: u64 = event.block.header.slot.into();

    // Live stream blocks are always Safe (above LIB).
    // Finalized status is only set during backfill.
    let status = BlockStatus::Safe;

    let mut zone_blocks = Vec::new();

    for tx in &event.block.transactions {
        for op in &tx.mantle_tx.ops {
            if let Op::ChannelInscribe(inscribe) = op
                && inscribe.channel_id == channel_id
            {
                let msg_id = inscribe.id();
                zone_blocks.push(ZoneBlockWithStatus {
                    block: ZoneBlock {
                        id: msg_id,
                        data: inscribe.inscription.clone(),
                    },
                    status,
                    block_id,
                    cursor: Cursor {
                        slot: block_slot,
                        last_id: Some(msg_id),
                    },
                });
            }
        }
    }

    zone_blocks
}

impl ZoneIndexer {
    #[must_use]
    pub fn new(channel_id: ChannelId, node_url: Url, auth: Option<BasicAuthCredentials>) -> Self {
        Self {
            channel_id,
            node_url,
            http_client: CommonHttpClient::new(auth),
        }
    }

    /// Subscribe to live zone messages as they finalize (LIB stream only).
    ///
    /// This is a simpler API that only emits finalized messages. For the full
    /// Safe → Finalized lifecycle, use [`follow`] instead.
    pub async fn follow_lib(
        &self,
    ) -> Result<impl Stream<Item = ZoneBlock> + '_, lb_common_http_client::Error> {
        let lib_stream = self
            .http_client
            .get_lib_stream(self.node_url.clone())
            .await?;

        let channel_id = self.channel_id;
        let stream = lib_stream.filter_map(move |block_info| {
            let http_client = self.http_client.clone();
            let node_url = self.node_url.clone();
            let header_id = block_info.header_id;

            async move {
                let block = match http_client.get_block(node_url, header_id).await {
                    Ok(Some(block)) => block,
                    Ok(None) => {
                        warn!("LIB block {header_id} not found");
                        return None;
                    }
                    Err(e) => {
                        warn!("Failed to fetch LIB block {header_id}: {e}");
                        return None;
                    }
                };

                let zone_blocks: Vec<ZoneBlock> = block
                    .transactions()
                    .flat_map(|tx| &tx.mantle_tx.ops)
                    .filter_map(|op| match op {
                        Op::ChannelInscribe(inscribe) if inscribe.channel_id == channel_id => {
                            Some(ZoneBlock {
                                id: inscribe.id(),
                                data: inscribe.inscription.clone(),
                            })
                        }
                        _ => None,
                    })
                    .collect();

                Some(stream::iter(zone_blocks))
            }
        });

        Ok(stream.flatten())
    }

    /// Subscribe to zone messages with optional backfill from cursor.
    ///
    /// - `cursor = None`: Start fresh from now, no backfill, just live stream
    /// - `cursor = Some(...)`: Backfill from cursor to current LIB, then live
    ///   stream
    ///
    /// Each message includes a cursor that can be persisted for resumption.
    ///
    /// # Status lifecycle
    ///
    /// Messages follow a Safe → Finalized lifecycle:
    /// - When a block first arrives (above LIB): emitted as `Safe`
    /// - When LIB advances past that block: emitted again as `Finalized`
    ///
    /// The same message may be emitted multiple times:
    /// - Multiple `Safe` events during reorgs (message appears in different
    ///   blocks)
    /// - One `Finalized` event when LIB advances past the block
    ///
    /// Callers should track by `MsgId` to correlate these events.
    ///
    /// # Cursor semantics
    ///
    /// Each message includes a cursor representing its position. The cursor is
    /// meant for checkpointing - persist the cursor from the last processed
    /// message to resume from that point after restart.
    ///
    /// # Example
    /// ```ignore
    /// let cursor = load_checkpoint(); // or None to start from beginning
    /// let stream = indexer.follow(cursor).await?;
    /// while let Some(msg) = stream.next().await {
    ///     match msg.status {
    ///         BlockStatus::Safe => println!("New message (not yet finalized)"),
    ///         BlockStatus::Finalized => println!("Message is now permanent"),
    ///     }
    ///     save_checkpoint(msg.cursor);
    /// }
    /// ```
    pub async fn follow(
        &self,
        cursor: Option<Cursor>,
    ) -> Result<impl Stream<Item = ZoneBlockWithStatus> + '_, Error> {
        // Get current LIB info
        let info = self
            .http_client
            .consensus_info(self.node_url.clone())
            .await?;
        let current_lib = info.lib;
        let current_lib_slot = self
            .http_client
            .get_block(self.node_url.clone(), current_lib)
            .await?
            .map_or(0, |block| block.header().slot().into());

        // Subscribe to live stream first (so we don't miss blocks during backfill)
        let blocks_stream = Box::pin(
            self.http_client
                .get_blocks_stream(self.node_url.clone())
                .await?,
        );

        // Backfill only if cursor is provided (resuming from checkpoint).
        // If cursor is None, start fresh from now - no backfill.
        let (backfill, seen_blocks) = if let Some(c) = cursor {
            let backfill = self.backfill(Some(c), current_lib_slot).await?;
            let seen: HashSet<HeaderId> = backfill.iter().map(|m| m.block_id).collect();
            (backfill, seen)
        } else {
            (Vec::new(), HashSet::new())
        };

        let channel_id = self.channel_id;
        let http_client = self.http_client.clone();
        let node_url = self.node_url.clone();

        // Use unfold to maintain state (current_lib, current_lib_slot) across
        // iterations
        let live_stream = stream::unfold(
            (blocks_stream, current_lib, current_lib_slot, seen_blocks),
            move |(mut blocks_stream, mut lib_id, mut lib_slot, mut seen)| {
                let http_client = http_client.clone();
                let node_url = node_url.clone();

                async move {
                    let event = blocks_stream.next().await?;
                    let mut results = Vec::new();

                    // Check if LIB advanced - emit Finalized for newly finalized blocks
                    if event.lib != lib_id {
                        // Fetch new LIB block to get its slot
                        if let Ok(Some(lib_block)) =
                            http_client.get_block(node_url.clone(), event.lib).await
                        {
                            let new_lib_slot: u64 = lib_block.header().slot().into();

                            // Fetch and emit blocks between old_lib and new_lib as Finalized
                            if new_lib_slot > lib_slot
                                && let Ok(blocks) = http_client
                                    .get_blocks(node_url.clone(), lib_slot + 1, new_lib_slot)
                                    .await
                            {
                                for block in blocks {
                                    let block_id = block.header.id;
                                    let block_slot: u64 = block.header.slot.into();

                                    for tx in &block.transactions {
                                        for op in &tx.mantle_tx.ops {
                                            if let Op::ChannelInscribe(inscribe) = op
                                                && inscribe.channel_id == channel_id
                                            {
                                                let msg_id = inscribe.id();
                                                results.push(ZoneBlockWithStatus {
                                                    block: ZoneBlock {
                                                        id: msg_id,
                                                        data: inscribe.inscription.clone(),
                                                    },
                                                    status: BlockStatus::Finalized,
                                                    block_id,
                                                    cursor: Cursor {
                                                        slot: block_slot,
                                                        last_id: Some(msg_id),
                                                    },
                                                });
                                            }
                                        }
                                    }
                                }
                            }

                            lib_slot = new_lib_slot;
                        }
                        lib_id = event.lib;
                    }

                    // Emit current block as Safe (skip if already in backfill)
                    let block_id = event.block.header.id;
                    if seen.insert(block_id) {
                        results.extend(extract_zone_blocks(channel_id, &event));
                    }

                    Some((
                        stream::iter(results),
                        (blocks_stream, lib_id, lib_slot, seen),
                    ))
                }
            },
        )
        .flatten();

        // Chain backfill + live stream
        Ok(stream::iter(backfill).chain(live_stream))
    }

    /// Backfill finalized messages from cursor to target slot (LIB).
    ///
    /// All messages returned are marked `Finalized` since they are at or
    /// below the Last Irreversible Block.
    async fn backfill(
        &self,
        cursor: Option<Cursor>,
        target_slot: u64,
    ) -> Result<Vec<ZoneBlockWithStatus>, Error> {
        let mut result = Vec::new();

        let (mut current_slot, skip_until) = cursor.map_or((0, None), |c| {
            let start = if c.last_id.is_some() {
                c.slot
            } else {
                c.slot.saturating_add(1)
            };
            (start, c.last_id)
        });

        if current_slot > target_slot {
            return Ok(result);
        }

        let mut skip_until = skip_until;

        while current_slot <= target_slot {
            let end_slot = (current_slot + BATCH_SIZE - 1).min(target_slot);
            let blocks = self
                .http_client
                .get_blocks(self.node_url.clone(), current_slot, end_slot)
                .await?;

            for block in blocks {
                let block_slot: u64 = block.header.slot.into();
                let block_id = block.header.id;

                // Once we move past cursor slot, skipping is irrelevant
                if block_slot > cursor.map_or(0, |c| c.slot) {
                    skip_until = None;
                }

                for tx in &block.transactions {
                    for op in &tx.mantle_tx.ops {
                        if let Op::ChannelInscribe(inscribe) = op
                            && inscribe.channel_id == self.channel_id
                        {
                            let msg_id = inscribe.id();

                            // Skip up to (and including) last_id
                            if let Some(skip_id) = skip_until {
                                if msg_id == skip_id {
                                    skip_until = None;
                                }
                                continue;
                            }

                            result.push(ZoneBlockWithStatus {
                                block: ZoneBlock {
                                    id: msg_id,
                                    data: inscribe.inscription.clone(),
                                },
                                // Backfill reads from finalized chain (at or below LIB)
                                status: BlockStatus::Finalized,
                                block_id,
                                // Cursor for this specific message
                                cursor: Cursor {
                                    slot: block_slot,
                                    last_id: Some(msg_id),
                                },
                            });
                        }
                    }
                }
            }

            current_slot = end_slot + 1;
        }

        Ok(result)
    }

    /// Fetch the LIB slot.
    ///
    /// TODO(node-api): expose `lib_slot` in /cryptarchia/info so indexer
    /// doesn't need two calls (`consensus_info` + `get_block(lib)`).
    async fn lib_slot(&self) -> Result<u64, Error> {
        let info = self
            .http_client
            .consensus_info(self.node_url.clone())
            .await?;

        // Genesis block isn't stored as a regular block, so None here means slot 0.
        Ok(self
            .http_client
            .get_block(self.node_url.clone(), info.lib)
            .await?
            .map_or(0, |block| block.header().slot().into()))
    }

    /// Poll for the next batch of messages.
    ///
    /// Returns up to `limit` messages and a cursor for the next call.
    /// Pass `None` to start from the beginning, or pass the returned cursor
    /// to continue where you left off.
    pub async fn next_messages(
        &self,
        cursor: Option<Cursor>,
        limit: usize,
    ) -> Result<PollResult, Error> {
        let lib_slot = self.lib_slot().await?;

        let (cursor, mut current_slot) = cursor.map_or_else(
            || (Cursor::default(), 0),
            |c| {
                let start = if c.last_id.is_some() {
                    c.slot
                } else {
                    c.slot.saturating_add(1)
                };
                (c, start)
            },
        );

        if current_slot > lib_slot || limit == 0 {
            return Ok(PollResult {
                messages: Vec::new(),
                cursor: Cursor {
                    slot: lib_slot,
                    last_id: None,
                },
            });
        }

        let mut scan = ScanState::new(cursor, limit);

        while current_slot <= lib_slot {
            let end_slot = (current_slot + BATCH_SIZE - 1).min(lib_slot);
            let blocks = self
                .http_client
                .get_blocks(self.node_url.clone(), current_slot, end_slot)
                .await?;

            for block in blocks {
                let block_slot: u64 = block.header.slot.into();

                for tx in &block.transactions {
                    for op in &tx.mantle_tx.ops {
                        if let Op::ChannelInscribe(inscribe) = op
                            && inscribe.channel_id == self.channel_id
                            && let Some(done) =
                                scan.push_msg(block_slot, inscribe.id(), &inscribe.inscription)
                        {
                            return Ok(done);
                        }
                    }
                }
            }

            current_slot = end_slot + 1;
        }

        // Caught up to LIB.
        Ok(PollResult {
            messages: scan.out,
            cursor: Cursor {
                slot: lib_slot,
                last_id: None,
            },
        })
    }
}

/// Internal state machine for cursor-based message scanning.
struct ScanState {
    cursor_slot: u64,
    skip_until: Option<MsgId>,
    out: Vec<ZoneBlock>,
    limit: usize,
}

impl ScanState {
    const fn new(cursor: Cursor, limit: usize) -> Self {
        Self {
            cursor_slot: cursor.slot,
            skip_until: cursor.last_id,
            out: Vec::new(),
            limit,
        }
    }

    /// Process a message. Returns `Some(PollResult)` if limit reached.
    fn push_msg(&mut self, block_slot: u64, msg_id: MsgId, data: &[u8]) -> Option<PollResult> {
        // Once we move past cursor slot, skipping is irrelevant.
        debug_assert!(
            block_slot >= self.cursor_slot,
            "blocks must be scanned forward"
        );
        if block_slot > self.cursor_slot {
            self.skip_until = None;
        }

        // Skip up to (and including) last_id.
        if let Some(skip_id) = self.skip_until {
            if msg_id == skip_id {
                self.skip_until = None;
            }
            return None;
        }

        self.out.push(ZoneBlock {
            id: msg_id,
            data: data.to_vec(),
        });

        if self.out.len() >= self.limit {
            return Some(PollResult {
                messages: std::mem::take(&mut self.out),
                cursor: Cursor {
                    slot: block_slot,
                    last_id: Some(msg_id),
                },
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use lb_common_http_client::ApiHeader;
    use lb_core::{
        mantle::{
            MantleTx, SignedMantleTx, Transaction as _, ledger::Tx as LedgerTx,
            ops::channel::inscribe::InscriptionOp,
        },
        proofs::leader_proof::Groth16LeaderProof,
    };

    use super::*;

    fn msg_id(n: u8) -> MsgId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        MsgId::from(bytes)
    }

    fn header_id(n: u8) -> HeaderId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        HeaderId::from(bytes)
    }

    fn channel_id(n: u8) -> ChannelId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        ChannelId::from(bytes)
    }

    fn content_id(n: u8) -> lb_core::header::ContentId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        lb_core::header::ContentId::from(bytes)
    }

    fn make_inscribe_tx(channel: ChannelId, data: Vec<u8>) -> SignedMantleTx {
        // Create a deterministic signer for tests
        let secret_bytes = [42u8; 32];
        let signing_key =
            lb_key_management_system_service::keys::Ed25519Key::from_bytes(&secret_bytes);
        let signer = signing_key.public_key();

        let inscribe_op = InscriptionOp {
            channel_id: channel,
            inscription: data,
            parent: MsgId::root(),
            signer,
        };
        let ledger_tx = LedgerTx::new(vec![], vec![]);
        let mantle_tx = MantleTx {
            ops: vec![Op::ChannelInscribe(inscribe_op)],
            ledger_tx,
            storage_gas_price: 0,
            execution_gas_price: 0,
        };
        SignedMantleTx {
            ops_proofs: vec![],
            ledger_tx_proof: lb_key_management_system_service::keys::ZkKey::multi_sign(
                &[],
                mantle_tx.hash().as_ref(),
            )
            .expect("empty multi-sign"),
            mantle_tx,
        }
    }

    fn make_block_event(
        block_id: HeaderId,
        slot: u64,
        tip: HeaderId,
        lib: HeaderId,
        txs: Vec<SignedMantleTx>,
    ) -> ProcessedBlockEvent {
        use lb_common_http_client::ApiBlock;

        ProcessedBlockEvent {
            block: ApiBlock {
                header: ApiHeader {
                    id: block_id,
                    parent_block: header_id(0),
                    slot: slot.into(),
                    block_root: content_id(0),
                    proof_of_leadership: Groth16LeaderProof::genesis(),
                },
                transactions: txs,
            },
            tip,
            lib,
        }
    }

    // --- Tests for extract_zone_blocks ---

    #[test]
    fn extract_zone_blocks_filters_by_channel() {
        let target_channel = channel_id(1);
        let other_channel = channel_id(2);

        let tx1 = make_inscribe_tx(target_channel, vec![1, 2, 3]);
        let tx2 = make_inscribe_tx(other_channel, vec![4, 5, 6]);
        let tx3 = make_inscribe_tx(target_channel, vec![7, 8, 9]);

        let event = make_block_event(
            header_id(1),
            10,
            header_id(1),
            header_id(0),
            vec![tx1, tx2, tx3],
        );

        let blocks = extract_zone_blocks(target_channel, &event);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block.data, vec![1, 2, 3]);
        assert_eq!(blocks[1].block.data, vec![7, 8, 9]);
    }

    #[test]
    fn extract_zone_blocks_sets_safe_status_when_above_lib() {
        let channel = channel_id(1);
        let tx = make_inscribe_tx(channel, vec![1, 2, 3]);

        // Block is at tip (header_id(5)), lib is behind at header_id(3)
        let event = make_block_event(header_id(5), 50, header_id(5), header_id(3), vec![tx]);

        let blocks = extract_zone_blocks(channel, &event);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].status, BlockStatus::Safe);
        assert_eq!(blocks[0].block_id, header_id(5));
    }

    #[test]
    fn extract_zone_blocks_always_returns_safe_for_live_stream() {
        let channel = channel_id(1);
        let tx = make_inscribe_tx(channel, vec![1, 2, 3]);

        // Even when block_id == lib, live stream returns Safe
        // (Finalized is only set during backfill)
        let event = make_block_event(
            header_id(5),
            50,
            header_id(6),
            header_id(5), // lib == block_id
            vec![tx],
        );

        let blocks = extract_zone_blocks(channel, &event);

        assert_eq!(blocks.len(), 1);
        // Live stream always returns Safe - Finalized is only for backfill
        assert_eq!(blocks[0].status, BlockStatus::Safe);
    }

    #[test]
    fn extract_zone_blocks_includes_cursor() {
        let channel = channel_id(1);
        let tx = make_inscribe_tx(channel, vec![1, 2, 3]);

        let event = make_block_event(header_id(1), 42, header_id(1), header_id(0), vec![tx]);

        let blocks = extract_zone_blocks(channel, &event);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].cursor.slot, 42);
        assert!(blocks[0].cursor.last_id.is_some());
    }

    #[test]
    fn extract_zone_blocks_empty_when_no_matching_channel() {
        let target_channel = channel_id(1);
        let other_channel = channel_id(2);

        let tx = make_inscribe_tx(other_channel, vec![1, 2, 3]);

        let event = make_block_event(header_id(1), 10, header_id(1), header_id(0), vec![tx]);

        let blocks = extract_zone_blocks(target_channel, &event);
        assert!(blocks.is_empty());
    }

    // --- Tests for ScanState (cursor logic) ---

    #[test]
    fn resume_within_slot_skip() {
        // Cursor says: resume in slot 0 after msg_id(2)
        let cursor = Cursor {
            slot: 0,
            last_id: Some(msg_id(2)),
        };
        let mut scan = ScanState::new(cursor, 10);

        // Feed messages from slot 0
        assert!(scan.push_msg(0, msg_id(1), &[1]).is_none()); // skipped
        assert!(scan.push_msg(0, msg_id(2), &[2]).is_none()); // skipped (last_id itself)
        assert!(scan.push_msg(0, msg_id(3), &[3]).is_none()); // collected

        assert_eq!(scan.out.len(), 1);
        assert_eq!(scan.out[0].id, msg_id(3));
    }

    #[test]
    fn resume_next_slot() {
        // Cursor says: slot 0 is done, start at slot 1
        let cursor = Cursor {
            slot: 0,
            last_id: None,
        };
        let mut scan = ScanState::new(cursor, 10);

        // Feed messages from slot 1 (skip_until should be None, nothing to skip)
        assert!(scan.push_msg(1, msg_id(1), &[1]).is_none());
        assert!(scan.push_msg(1, msg_id(2), &[2]).is_none());

        assert_eq!(scan.out.len(), 2);
    }

    #[test]
    fn limit_hit_mid_block() {
        let cursor = Cursor::default();
        let mut scan = ScanState::new(cursor, 2);

        // Feed 3 messages, limit is 2
        assert!(scan.push_msg(0, msg_id(1), &[1]).is_none());
        let result = scan.push_msg(0, msg_id(2), &[2]);

        assert!(result.is_some());
        let poll = result.unwrap();
        assert_eq!(poll.messages.len(), 2);
        assert_eq!(poll.cursor.slot, 0);
        assert_eq!(poll.cursor.last_id, Some(msg_id(2)));
    }

    #[test]
    fn skip_clears_when_leaving_cursor_slot() {
        // Cursor points to non-existent msg in slot 0
        let cursor = Cursor {
            slot: 0,
            last_id: Some(msg_id(99)),
        };
        let mut scan = ScanState::new(cursor, 10);

        // All messages in slot 0 are skipped (looking for msg_id(99))
        assert!(scan.push_msg(0, msg_id(1), &[1]).is_none());
        assert!(scan.push_msg(0, msg_id(2), &[2]).is_none());
        assert_eq!(scan.out.len(), 0);

        // Moving to slot 1 clears skip_until
        assert!(scan.push_msg(1, msg_id(3), &[3]).is_none());
        assert_eq!(scan.out.len(), 1);
        assert_eq!(scan.out[0].id, msg_id(3));
    }
}

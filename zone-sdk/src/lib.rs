pub mod indexer;
pub mod sequencer;
pub mod state;

use lb_core::{header::HeaderId, mantle::ops::channel::MsgId};

/// A zone block — opaque data published to / read from a channel.
#[derive(Debug, Clone)]
pub struct ZoneBlock {
    /// The unique identifier of this inscription.
    pub id: MsgId,
    /// The opaque inscription data.
    pub data: Vec<u8>,
}

/// Finalization status of a zone block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStatus {
    /// On canonical chain between LIB and tip.
    Safe,
    /// At or below LIB, permanent.
    Finalized,
}

/// A zone block with its finalization status and cursor for resumption.
#[derive(Debug, Clone)]
pub struct ZoneBlockWithStatus {
    /// The zone block.
    pub block: ZoneBlock,
    /// The finalization status.
    pub status: BlockStatus,
    /// The block header ID containing this inscription.
    pub block_id: HeaderId,
    /// Cursor for resuming from this point.
    pub cursor: indexer::Cursor,
}

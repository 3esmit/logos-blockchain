use futures::{Stream, StreamExt as _};
use lb_common_http_client::Slot;
use lb_core::mantle::ops::channel::ChannelId;
use lb_log_targets::zone_sdk;
use tracing::warn;

use crate::{ZoneMessage, adapter};

const TARGET: &str = zone_sdk::INDEXER;

/// Errors returned while reading finalized channel messages.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] lb_common_http_client::Error),
}

/// Read-only view of finalized messages in a zone channel.
pub struct ZoneIndexer<Node> {
    channel_id: ChannelId,
    node: Node,
}

const BATCH_SIZE: Slot = Slot::new(100);

impl<Node> ZoneIndexer<Node>
where
    Node: adapter::Node + Clone + Sync,
{
    #[must_use]
    pub const fn new(channel_id: ChannelId, node: Node) -> Self {
        Self { channel_id, node }
    }

    /// Subscribe to finalized zone messages as they become immutable.
    pub async fn follow(&self) -> Result<impl Stream<Item = ZoneMessage> + '_, Error> {
        let lib_stream = self.node.lib_stream().await?;

        let channel_id = self.channel_id;
        let stream = lib_stream.filter_map(move |block_info| {
            let header_id = block_info.header_id;

            async move {
                let stream = match self
                    .node
                    .zone_messages_in_block(header_id, channel_id)
                    .await
                {
                    Ok(stream) => stream,
                    Err(error) => {
                        warn!(target: TARGET, "Failed to fetch LIB block {header_id}: {error}");
                        return None;
                    }
                };

                Some(stream)
            }
        });

        Ok(stream.flatten())
    }

    /// Stream finalized messages after `last_slot`, or from genesis when it is
    /// `None`.
    pub async fn next_messages(
        &self,
        last_slot: Option<Slot>,
    ) -> Result<impl Stream<Item = (ZoneMessage, Slot)> + '_, Error> {
        let lib_slot = self.node.consensus_info().await?.cryptarchia_info.lib_slot;
        let start_slot = last_slot.map_or_else(Slot::genesis, |slot| slot.strict_add(1.into()));

        #[expect(
            closure_returning_async_block,
            reason = "Signature expected by `unfold`"
        )]
        let stream = futures::stream::unfold(start_slot, move |current_slot| async move {
            if current_slot > lib_slot {
                return None;
            }

            let end_slot = (Slot::from(
                current_slot
                    .into_inner()
                    .saturating_add(BATCH_SIZE.into_inner())
                    .checked_sub(1)
                    .expect("slot shouldn't overflow"),
            ))
            .min(lib_slot);

            match self
                .node
                .zone_messages_in_blocks(current_slot, end_slot, self.channel_id)
                .await
            {
                Ok(messages) => Some((messages, end_slot.strict_add(1.into()))),
                Err(error) => {
                    warn!(
                        target: TARGET,
                        ?current_slot,
                        ?end_slot,
                        ?error,
                        "Failed to fetch zone messages from blocks",
                    );
                    None
                }
            }
        })
        .flatten();

        Ok(stream)
    }
}

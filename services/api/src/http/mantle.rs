use core::fmt::Debug;
use std::{collections::HashMap, fmt::Display, num::NonZeroUsize, ops::RangeInclusive};

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use lb_chain_broadcast_service::{BlockBroadcastMsg, BlockBroadcastService, BlockInfo};
use lb_chain_service::{
    CryptarchiaInfo, ProcessedBlockEvent, Slot,
    api::{CryptarchiaServiceApi, CryptarchiaServiceData},
    storage::{StorageAdapter as _, adapters::StorageAdapter},
};
use lb_core::{
    block::Block,
    events::Events,
    header::HeaderId,
    mantle::{
        SignedOps,
        channel::ChannelState,
        ledger::verification_mode::StandardMode,
        ops::channel::ChannelId,
        traits::Hashable,
        transactions::{hash::TxHash, states::Preverified},
    },
    sdp::{Declaration, DeclarationId},
};
use lb_log_targets::api;
use lb_storage_service::{
    StorageMsg, StorageService,
    api::{
        StorageApiRequest,
        chain::{StorageChainApi, requests::ChainApiRequest},
    },
};
use lb_tx_service::{
    MempoolMetrics, MempoolMsg, TxMempoolService, backend::Mempool,
    network::adapters::libp2p::Libp2pAdapter as MempoolNetworkAdapter,
    tx::service::openapi::Status,
};
use overwatch::services::AsServiceId;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::oneshot;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use crate::http::{
    consensus::{Cryptarchia, cryptarchia_info, cryptarchia_ledger_state},
    errors::BlockSlotRangeError,
};

const LOG_TARGET: &str = api::http::MANTLE;
/// Bound parent lookups used to verify an immutable range against the current
/// LIB. A corrupted index must fail closed without allowing an unbounded walk.
const IMMUTABLE_ANCESTRY_CHECK_LIMIT: usize = 16_384;

/// A block along with the current chain state (tip and LIB) at the time it was
/// processed. This allows clients to track the canonical chain without needing
/// to poll /cryptarchia/info.
pub struct BlockWithChainState<Tx> {
    /// The processed block.
    pub block: Block<Tx>,
    /// The current canonical tip after processing this block.
    pub tip: HeaderId,
    pub tip_slot: Slot,
    /// The current Last Irreversible Block after processing this block.
    pub lib: HeaderId,
    pub lib_slot: Slot,
}

pub type MempoolService<StorageAdapter, RuntimeServiceId> = TxMempoolService<
    MempoolNetworkAdapter<
        SignedOps<Preverified, StandardMode>,
        <SignedOps<Preverified, StandardMode> as Hashable>::Hash,
        RuntimeServiceId,
    >,
    Mempool<
        HeaderId,
        SignedOps<Preverified, StandardMode>,
        <SignedOps<Preverified, StandardMode> as Hashable>::Hash,
        StorageAdapter,
        RuntimeServiceId,
    >,
    StorageAdapter,
    RuntimeServiceId,
>;

pub async fn channel_state<RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    id: ChannelId,
) -> Result<Option<ChannelState>, super::DynError>
where
    RuntimeServiceId:
        Debug + Send + Sync + Display + 'static + AsServiceId<Cryptarchia<RuntimeServiceId>>,
{
    let ledger_state = cryptarchia_ledger_state(handle).await?;
    Ok(ledger_state
        .mantle_ledger()
        .channels()
        .channel_state(&id)
        .cloned())
}

pub async fn channel<RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    id: ChannelId,
) -> Result<ChannelState, super::DynError>
where
    RuntimeServiceId:
        Debug + Send + Sync + Display + 'static + AsServiceId<Cryptarchia<RuntimeServiceId>>,
{
    channel_state(handle, id)
        .await?
        .ok_or_else(|| "channel not found".into())
}

pub async fn mantle_mempool_metrics<StorageAdapter, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<MempoolMetrics, super::DynError>
where
    StorageAdapter: lb_tx_service::storage::MempoolStorageAdapter<
            RuntimeServiceId,
            Key = <SignedOps<Preverified, StandardMode> as Hashable>::Hash,
            Item = SignedOps<Preverified, StandardMode>,
        > + Clone
        + 'static,
    StorageAdapter::Error: Debug,
    RuntimeServiceId: Debug
        + Clone
        + Sync
        + Send
        + Display
        + 'static
        + AsServiceId<StorageService<StorageAdapter::Backend, RuntimeServiceId>>
        + AsServiceId<MempoolService<StorageAdapter, RuntimeServiceId>>,
{
    let relay = handle
        .relay::<MempoolService<StorageAdapter, RuntimeServiceId>>()
        .await?;
    let (sender, receiver) = oneshot::channel();
    relay
        .send(MempoolMsg::Metrics {
            reply_channel: sender,
        })
        .await
        .map_err(|e| e)?;

    receiver.await.map_err(|e| Box::new(e) as super::DynError)
}

pub async fn mantle_mempool_status<StorageAdapter, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    items: Vec<<SignedOps<Preverified, StandardMode> as Hashable>::Hash>,
) -> Result<Vec<Status>, super::DynError>
where
    StorageAdapter: lb_tx_service::storage::MempoolStorageAdapter<
            RuntimeServiceId,
            Key = <SignedOps<Preverified, StandardMode> as Hashable>::Hash,
            Item = SignedOps<Preverified, StandardMode>,
        > + Clone
        + 'static,
    StorageAdapter::Error: Debug,
    RuntimeServiceId: Debug
        + Clone
        + Sync
        + Send
        + Display
        + 'static
        + AsServiceId<StorageService<StorageAdapter::Backend, RuntimeServiceId>>
        + AsServiceId<MempoolService<StorageAdapter, RuntimeServiceId>>,
{
    let relay = handle
        .relay::<MempoolService<StorageAdapter, RuntimeServiceId>>()
        .await?;
    let (sender, receiver) = oneshot::channel();
    relay
        .send(MempoolMsg::Status {
            items,
            reply_channel: sender,
        })
        .await
        .map_err(|e| e)?;

    receiver.await.map_err(|e| Box::new(e) as super::DynError)
}

pub async fn lib_block_stream<RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<
    impl Stream<Item = Result<BlockInfo, crate::http::DynError>> + Send + Sync + use<RuntimeServiceId>,
    super::DynError,
>
where
    RuntimeServiceId: Debug + Sync + Display + AsServiceId<BlockBroadcastService<RuntimeServiceId>>,
{
    let relay = handle.relay().await?;
    let (sender, receiver) = oneshot::channel();
    relay
        .send(BlockBroadcastMsg::SubscribeToFinalizedBlocks {
            result_sender: sender,
        })
        .await
        .map_err(|e| e)?;

    let broadcast_receiver = receiver.await.map_err(|e| Box::new(e) as super::DynError)?;
    let stream = BroadcastStream::new(broadcast_receiver)
        .map(|result| result.map_err(|e| Box::new(e) as crate::http::DynError));

    Ok(stream)
}

pub async fn get_processed_blocks_event_stream<Transaction, Service, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<
    impl Stream<Item = Result<ProcessedBlockEvent, crate::http::DynError>>
    + Send
    + Sync
    + use<Transaction, Service, RuntimeServiceId>,
    super::DynError,
>
where
    Transaction: Send + 'static,
    Service: CryptarchiaServiceData<Tx = Transaction>,
    RuntimeServiceId: Debug + Sync + Display + AsServiceId<Service>,
{
    let new_blocks_receiver = CryptarchiaServiceApi::<Service>::from_overwatch_handle(handle)
        .await
        .subscribe_new_blocks()
        .await?;

    let processed_blocks_stream = BroadcastStream::new(new_blocks_receiver)
        .map(|item| item.map_err(|error| Box::new(error) as crate::http::DynError));

    Ok(processed_blocks_stream)
}

pub async fn get_new_blocks_stream<
    Transaction,
    StorageBackend,
    ConsensusService,
    RuntimeServiceId,
>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<
    impl Stream<Item = BlockWithChainState<Transaction>>
    + Send
    + use<Transaction, StorageBackend, ConsensusService, RuntimeServiceId>,
    super::DynError,
>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    ConsensusService: CryptarchiaServiceData<Tx = Transaction>,
    RuntimeServiceId: Debug
        + Sync
        + Display
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + AsServiceId<ConsensusService>
        + 'static,
{
    let processed_blocks_stream =
        get_processed_blocks_event_stream::<Transaction, ConsensusService, RuntimeServiceId>(
            handle,
        )
        .await?;

    let relay = handle
        .relay::<StorageService<StorageBackend, RuntimeServiceId>>()
        .await?;
    let storage_adapter =
        StorageAdapter::<StorageBackend, Transaction, RuntimeServiceId>::new(relay).await;

    let new_blocks_stream = processed_blocks_stream.filter_map(move |event| {
        let storage_adapter = storage_adapter.clone();
        async move {
            let event = event.ok()?;
            let block = storage_adapter.get_block(&event.block_id).await?;
            Some(BlockWithChainState {
                block,
                tip: event.tip,
                tip_slot: event.tip_slot,
                lib: event.lib,
                lib_slot: event.lib_slot,
            })
        }
    });

    Ok(new_blocks_stream)
}

async fn get_immutable_block_ids_in_slot_range<Backend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    slot_from: Slot,
    slot_to: Slot,
    limit: NonZeroUsize,
    descending: bool,
) -> Result<Vec<HeaderId>, super::DynError>
where
    Backend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    RuntimeServiceId:
        Debug + Sync + Display + AsServiceId<StorageService<Backend, RuntimeServiceId>>,
{
    let relay = handle.relay().await?;
    let (response_tx, response_rx) = oneshot::channel();
    let request = if descending {
        ChainApiRequest::ScanImmutableBlockIdsReverse {
            slot_range: RangeInclusive::new(slot_from, slot_to),
            limit,
            response_tx,
        }
    } else {
        ChainApiRequest::ScanImmutableBlockIds {
            slot_range: RangeInclusive::new(slot_from, slot_to),
            limit,
            response_tx,
        }
    };

    relay
        .send(StorageMsg::Api {
            request: StorageApiRequest::Chain(request),
        })
        .await
        .map_err(|error| error)?;

    response_rx
        .await
        .map_err(|error| Box::new(error) as super::DynError)
}

async fn load_blocks_with_chain_state_by_ids<Transaction, StorageBackend, RuntimeServiceId>(
    storage_adapter: &StorageAdapter<StorageBackend, Transaction, RuntimeServiceId>,
    header_ids: Vec<HeaderId>,
    chain_info: &CryptarchiaInfo,
    blocks_limit: usize,
) -> Result<Vec<BlockWithChainState<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>,
{
    let mut blocks = Vec::with_capacity(header_ids.len().min(blocks_limit));
    for header_id in header_ids {
        let Some(block) = storage_adapter.get_block(&header_id).await else {
            warn!(
                target: LOG_TARGET,
                "missing block body for indexed header {header_id}, skipping"
            );
            continue;
        };
        blocks.push(BlockWithChainState {
            block,
            tip: chain_info.tip,
            tip_slot: chain_info.slot,
            lib: chain_info.lib,
            lib_slot: chain_info.lib_slot,
        });
        if blocks.len() == blocks_limit {
            break;
        }
    }

    Ok(blocks)
}

/// Check that indexed immutable IDs occur in one canonical child-to-parent
/// path. Slot ordering alone does not prove ancestry after recovery or a
/// reorganisation, so stale index entries must fail closed.
fn validate_immutable_ids_on_path(
    indexed_ids: &[HeaderId],
    canonical_path: &[HeaderId],
    descending: bool,
) -> Result<(), String> {
    let expected_ids: Vec<HeaderId> = if descending {
        indexed_ids.to_vec()
    } else {
        indexed_ids.iter().rev().copied().collect()
    };

    let mut expected_index = 0;
    for id in canonical_path {
        if expected_ids.get(expected_index) == Some(id) {
            expected_index += 1;
            if expected_index == expected_ids.len() {
                return Ok(());
            }
        }
    }

    let missing_id = expected_ids
        .get(expected_index)
        .map_or_else(|| "<none>".to_owned(), |id| format!("{id:?}"));
    Err(format!(
        "immutable block index is not on one canonical chain; missing indexed header {missing_id}"
    ))
}

/// Verify immutable IDs against the advertised LIB before loading or
/// serializing their bodies. This prevents contradictory ancestry from being
/// returned when the index is stale or corrupted.
async fn validate_immutable_block_ids<Transaction, StorageBackend, RuntimeServiceId>(
    storage_adapter: &StorageAdapter<StorageBackend, Transaction, RuntimeServiceId>,
    indexed_ids: &[HeaderId],
    lib: HeaderId,
    descending: bool,
) -> Result<(), super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>,
{
    if indexed_ids.is_empty() {
        return Ok(());
    }

    let expected_ids: Vec<HeaderId> = if descending {
        indexed_ids.to_vec()
    } else {
        indexed_ids.iter().rev().copied().collect()
    };
    let mut expected_index = 0;
    let mut current = lib;
    let mut canonical_path = Vec::with_capacity(expected_ids.len().min(1024));

    for step in 0..=IMMUTABLE_ANCESTRY_CHECK_LIMIT {
        canonical_path.push(current);
        if expected_ids.get(expected_index) == Some(&current) {
            expected_index += 1;
            if expected_index == expected_ids.len() {
                return validate_immutable_ids_on_path(indexed_ids, &canonical_path, descending)
                    .map_err(Into::into);
            }
        }

        if step == IMMUTABLE_ANCESTRY_CHECK_LIMIT {
            break;
        }

        let Some(parent) = storage_adapter.get_block_parent(&current).await else {
            return Err(format!(
                "immutable block index is not anchored at LIB {lib:?}: missing parent for {current:?}"
            )
            .into());
        };
        if parent == current {
            return Err(format!(
                "immutable block index is not anchored at LIB {lib:?}: {current:?} is its own parent"
            )
            .into());
        }
        current = parent;
    }

    let missing_id = expected_ids
        .get(expected_index)
        .map_or_else(|| "<none>".to_owned(), |id| format!("{id:?}"));
    Err(format!(
        "immutable block index is not anchored at LIB {lib:?} within {IMMUTABLE_ANCESTRY_CHECK_LIMIT} parent links; missing indexed header {missing_id}"
    )
    .into())
}

fn validate_blocks_slot_range(
    slot_from: Slot,
    slot_to: Slot,
    immutable_only: bool,
    chain_info: &CryptarchiaInfo,
) -> Result<(), BlockSlotRangeError> {
    if slot_to < slot_from {
        return Err(BlockSlotRangeError::InvalidRange { slot_from, slot_to });
    }
    if immutable_only {
        if slot_to > chain_info.lib_slot {
            return Err(BlockSlotRangeError::SlotToExceedsLibSlot {
                slot_to,
                lib_slot: chain_info.lib_slot,
            });
        }
    } else if slot_to > chain_info.slot {
        return Err(BlockSlotRangeError::SlotToExceedsTipSlot {
            slot_to,
            tip_slot: chain_info.slot,
        });
    }

    Ok(())
}

async fn fetch_and_load_mutable_blocks<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    storage_adapter: &StorageAdapter<StorageBackend, Transaction, RuntimeServiceId>,
    chain_info: &CryptarchiaInfo,
    slot_from: Slot,
    slot_to: Slot,
    remaining: usize,
    descending: bool,
) -> Result<Vec<BlockWithChainState<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + AsServiceId<Cryptarchia<RuntimeServiceId>>,
{
    let limit = remaining;
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut blocks = Vec::with_capacity(limit.min(1024));
    let mut current_id = chain_info.tip;
    let gated_slot_from = slot_from.max(chain_info.lib_slot.strict_add(1.into()));
    if gated_slot_from > slot_to {
        return Ok(Vec::new());
    }
    let mut retried = false;

    loop {
        // This function only serves the mutable window. Once we hit LIB we are below
        // the requested mutable range and should stop without loading the body.
        if current_id == chain_info.lib {
            break;
        }

        let Some(block) = storage_adapter.get_block(&current_id).await else {
            if retried {
                return Err(format!(
                    "canonical chain inconsistency: missing block {current_id} while traversing \
                    mutable chain anchored at LIB {}",
                    chain_info.lib
                )
                .into());
            }

            // Retry once from the latest tip if the original tip is not yet available.
            // The original LIB remains the anchor for this request.
            let refreshed_info = cryptarchia_info::<RuntimeServiceId>(handle).await?;
            current_id = refreshed_info.cryptarchia_info.tip;
            retried = true;
            blocks.clear();
            continue;
        };

        let header = block.header();
        let slot = header.slot();
        let parent_id = header.parent_block();

        if slot < gated_slot_from {
            break;
        }

        if slot <= slot_to {
            blocks.push(BlockWithChainState {
                block,
                tip: chain_info.tip,
                tip_slot: chain_info.slot,
                lib: chain_info.lib,
                lib_slot: chain_info.lib_slot,
            });

            if descending && blocks.len() == limit {
                break;
            }
        }

        // Defensive guard against malformed/self-parenting headers.
        if parent_id == current_id {
            return Err(format!(
                "canonical chain inconsistency: block {current_id} at slot {slot:?} is its own\
                 parent before anchored LIB {}",
                chain_info.lib
            )
            .into());
        }
        current_id = parent_id;
    }

    if !descending {
        blocks.reverse();
        blocks.truncate(limit);
    }

    Ok(blocks)
}

async fn fetch_and_load_immutable_blocks<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    storage_adapter: &StorageAdapter<StorageBackend, Transaction, RuntimeServiceId>,
    chain_info: &CryptarchiaInfo,
    slot_from: Slot,
    immutable_slot_to: Slot,
    remaining: usize,
    descending: bool,
) -> Result<Vec<BlockWithChainState<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>,
{
    let limit = NonZeroUsize::new(remaining.saturating_mul(2).max(1))
        .expect("remaining is positive while fetching immutable blocks");
    let header_ids = get_immutable_block_ids_in_slot_range::<StorageBackend, RuntimeServiceId>(
        handle,
        slot_from,
        immutable_slot_to,
        limit,
        descending,
    )
    .await?;

    validate_immutable_block_ids(storage_adapter, &header_ids, chain_info.lib, descending).await?;

    load_blocks_with_chain_state_by_ids(storage_adapter, header_ids, chain_info, remaining).await
}

pub async fn get_blocks_in_slot_range_with_snapshot<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    slot_from: Slot,
    slot_to: Slot,
    descending: bool,
    blocks_limit: NonZeroUsize,
    immutable_only: bool,
    chain_info: &CryptarchiaInfo,
) -> Result<Vec<BlockWithChainState<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + AsServiceId<Cryptarchia<RuntimeServiceId>>,
{
    validate_blocks_slot_range(slot_from, slot_to, immutable_only, chain_info)
        .map_err(|e| Box::new(e) as super::DynError)?;

    let relay = handle
        .relay::<StorageService<StorageBackend, RuntimeServiceId>>()
        .await?;
    let storage_adapter =
        StorageAdapter::<StorageBackend, Transaction, RuntimeServiceId>::new(relay).await;

    let mut blocks = Vec::with_capacity(blocks_limit.get().min(1024));
    let mut remaining = blocks_limit.get();

    let immutable_slot_to = slot_to.min(chain_info.lib_slot);
    let has_immutable_range = slot_from <= immutable_slot_to;
    // Mutable window starts strictly above LIB; LIB itself is served via immutable
    // index.
    let mutable_slot_from = (chain_info.lib_slot.strict_add(1.into())).max(slot_from);
    let has_mutable_range = !immutable_only && mutable_slot_from <= slot_to;

    let fetch_mutable = |remaining: usize, descending: bool| {
        let storage_adapter = storage_adapter.clone();
        async move {
            fetch_and_load_mutable_blocks::<Transaction, StorageBackend, RuntimeServiceId>(
                handle,
                &storage_adapter,
                chain_info,
                mutable_slot_from,
                slot_to,
                remaining,
                descending,
            )
            .await
        }
    };

    let fetch_immutable = |remaining: usize, descending: bool| {
        let storage_adapter = storage_adapter.clone();
        async move {
            fetch_and_load_immutable_blocks::<Transaction, StorageBackend, RuntimeServiceId>(
                handle,
                &storage_adapter,
                chain_info,
                slot_from,
                immutable_slot_to,
                remaining,
                descending,
            )
            .await
        }
    };

    if descending {
        if remaining > 0 && has_mutable_range {
            let mutable_blocks = fetch_mutable(remaining, true).await?;
            remaining = remaining.saturating_sub(mutable_blocks.len());
            blocks.extend(mutable_blocks);
        }

        if remaining > 0 && has_immutable_range {
            let immutable_blocks = fetch_immutable(remaining, true).await?;
            blocks.extend(immutable_blocks);
        }
    } else {
        if remaining > 0 && has_immutable_range {
            let immutable_blocks = fetch_immutable(remaining, false).await?;
            remaining = remaining.saturating_sub(immutable_blocks.len());
            blocks.extend(immutable_blocks);
        }

        if remaining > 0 && has_mutable_range {
            let mutable_blocks = fetch_mutable(remaining, false).await?;
            blocks.extend(mutable_blocks);
        }
    }

    Ok(blocks)
}

/// Fetch immutable block header IDs in a slot range.
///
/// Kept as a compatibility helper for consumers that need to inspect headers
/// before fetching block bodies. The storage scan remains bounded by the
/// requested slot span and rejects reversed ranges.
pub async fn get_immutable_blocks_header_ids<Backend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    from_slot: usize,
    to_slot: usize,
) -> Result<Vec<HeaderId>, super::DynError>
where
    Backend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    RuntimeServiceId:
        Debug + Sync + Display + AsServiceId<StorageService<Backend, RuntimeServiceId>>,
{
    if to_slot < from_slot {
        return Err("to_slot must be greater or equal to from_slot".into());
    }
    let span = to_slot
        .checked_sub(from_slot)
        .and_then(|difference| difference.checked_add(1))
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| "immutable block range is too large".to_owned())?;
    get_immutable_block_ids_in_slot_range::<Backend, RuntimeServiceId>(
        handle,
        Slot::new(from_slot as u64),
        Slot::new(to_slot as u64),
        span,
        false,
    )
    .await
}

fn slot_range_limit(slot_from: Slot, slot_to: Slot) -> Option<NonZeroUsize> {
    slot_to
        .into_inner()
        .checked_sub(slot_from.into_inner())
        .and_then(|diff| diff.checked_add(1))
        .and_then(|diff| usize::try_from(diff).ok())
        .and_then(NonZeroUsize::new)
}

/// Fetch immutable blocks in range
///
/// # Parameters
///
/// - `handle`: A reference to the `OverwatchHandle` to interact with the
///   runtime and storage service.
/// - `from_slot`: A non-zero starting slot (inclusive) indicating the starting
///   point of the desired slot range.
/// - `to_slot`: A non-zero ending slot (inclusive) indicating the endpoint of
///   the desired slot range. If the range spans across the LIB block, only
///   blocks up to LIB will be returned.
///
/// # Returns
///
/// If successful, returns a `Vec` containing the immutable blocks for the
/// specified slot range. If any error occurs during processing, returns a boxed
/// `DynError`.
pub async fn get_immutable_blocks<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    from_slot: usize,
    to_slot: usize,
) -> Result<Vec<Block<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + AsServiceId<Cryptarchia<RuntimeServiceId>>
        + 'static,
{
    if to_slot < from_slot {
        return Err("to_slot must be greater or equal to from_slot".into());
    }

    let chain_info = cryptarchia_info::<RuntimeServiceId>(handle)
        .await?
        .cryptarchia_info;
    let slot_from = Slot::new(from_slot as u64);
    if slot_from > chain_info.lib_slot {
        return Ok(Vec::new());
    }

    let slot_to = Slot::new(to_slot as u64).min(chain_info.lib_slot);
    let blocks_limit = slot_range_limit(slot_from, slot_to)
        .ok_or_else(|| "legacy immutable block range is too large".to_owned())?;
    let blocks = get_blocks_in_slot_range_with_snapshot::<_, _, RuntimeServiceId>(
        handle,
        slot_from,
        slot_to,
        false,
        blocks_limit,
        true,
        &chain_info,
    )
    .await?;

    Ok(blocks.into_iter().map(|block| block.block).collect())
}

/// Fetch a single block by its header ID.
///
/// # Arguments
///
/// - `handle`: A reference to the `OverwatchHandle` to interact with the
///   runtime and storage service.
/// - `header_id`: The `HeaderId` of the block to fetch.
///
/// # Returns
///
/// If successful, returns `Some(Block<Transaction>)` if the block exists, or
/// `None` if no block with the given header ID was found. Returns a boxed
/// `DynError` if any error occurs during processing.
pub async fn get_block<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    header_id: HeaderId,
) -> Result<Option<Block<Transaction>>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Sync
        + Display
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + 'static,
{
    let relay = handle.relay().await?;
    let storage_adapter = StorageAdapter::<_, _, RuntimeServiceId>::new(relay).await;
    Ok(storage_adapter.get_block(&header_id).await)
}

/// Fetch transactions by their hashes.
///
/// # Arguments
///
/// - `handle`: A reference to the `OverwatchHandle` to interact with the
///   runtime and storage service.
/// - `tx_hashes`: The ordered [`TxHash`]es to fetch.
///
/// # Returns
///
/// If successful, returns a stream of matching [`Transaction`]s.
/// Returns a boxed `DynError` if any error occurs during processing.
pub async fn get_transactions<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    tx_hashes: Vec<TxHash>,
) -> Result<
    impl Stream<Item = Transaction> + use<Transaction, StorageBackend, RuntimeServiceId>,
    super::DynError,
>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Sync
        + Display
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + 'static,
{
    let relay = handle.relay().await?;
    let storage_adapter = StorageAdapter::<_, _, RuntimeServiceId>::new(relay).await;
    storage_adapter.get_transactions(tx_hashes).await
}

/// Fetch a single transaction by its hash.
///
/// # Arguments
///
/// - `handle`: A reference to the `OverwatchHandle` to interact with the
///   runtime and storage service.
/// - `tx_hash`: The [`TxHash`] of the transaction to fetch.
///
/// # Returns
///
/// - `Ok(Some(tx))`: Found transaction.
/// - `Ok(None)`: No transaction with the given hash was found.
/// - `Err(_)`: An error occurred during processing.
pub async fn get_transaction<Transaction, StorageBackend, RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
    tx_hash: TxHash,
) -> Result<Option<Transaction>, super::DynError>
where
    Transaction:
        Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static + Hashable<Hash = TxHash>,
    StorageBackend: lb_storage_service::backends::StorageBackend + Send + Sync + 'static,
    <StorageBackend as StorageChainApi>::Block:
        TryFrom<Block<Transaction>> + TryInto<Block<Transaction>>,
    <StorageBackend as StorageChainApi>::Tx: From<Bytes> + AsRef<[u8]>,
    <StorageBackend as StorageChainApi>::Events: TryFrom<Events> + TryInto<Events>,
    RuntimeServiceId: Debug
        + Sync
        + Display
        + AsServiceId<StorageService<StorageBackend, RuntimeServiceId>>
        + 'static,
{
    let mut stream =
        get_transactions::<Transaction, StorageBackend, RuntimeServiceId>(handle, vec![tx_hash])
            .await?;

    // Assume only one transaction is returned
    Ok(stream.next().await)
}

pub async fn get_sdp_declarations<RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<HashMap<DeclarationId, Declaration>, super::DynError>
where
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<Cryptarchia<RuntimeServiceId>>
        + 'static,
{
    let chain_api =
        CryptarchiaServiceApi::<Cryptarchia<RuntimeServiceId>>::from_overwatch_handle(handle).await;
    Ok(chain_api.get_sdp_declarations().await?)
}

pub async fn get_sdp_snapshot<RuntimeServiceId>(
    handle: &overwatch::overwatch::handle::OverwatchHandle<RuntimeServiceId>,
) -> Result<HashMap<DeclarationId, Declaration>, super::DynError>
where
    RuntimeServiceId: Debug
        + Send
        + Sync
        + Display
        + 'static
        + AsServiceId<Cryptarchia<RuntimeServiceId>>
        + 'static,
{
    let chain_api =
        CryptarchiaServiceApi::<Cryptarchia<RuntimeServiceId>>::from_overwatch_handle(handle).await;
    Ok(chain_api.get_sdp_snapshot().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_id(value: u8) -> HeaderId {
        HeaderId::from([value; 32])
    }

    #[test]
    fn immutable_index_accepts_sparse_ascending_path() {
        let result = validate_immutable_ids_on_path(
            &[header_id(1), header_id(3)],
            &[header_id(4), header_id(3), header_id(2), header_id(1)],
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn immutable_index_accepts_sparse_descending_path() {
        let result = validate_immutable_ids_on_path(
            &[header_id(3), header_id(1)],
            &[header_id(4), header_id(3), header_id(2), header_id(1)],
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn immutable_index_rejects_conflicting_branch() {
        let result = validate_immutable_ids_on_path(
            &[header_id(1), header_id(9)],
            &[header_id(4), header_id(3), header_id(2), header_id(1)],
            false,
        );
        assert!(
            result
                .expect_err("off-chain immutable ID must fail validation")
                .contains("missing indexed header")
        );
    }

    #[test]
    fn immutable_index_rejects_unanchored_single_block() {
        assert!(
            validate_immutable_ids_on_path(&[header_id(9)], &[header_id(4), header_id(3)], false,)
                .is_err()
        );
    }
}

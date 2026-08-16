use std::{
    ffi::{CString, c_char},
    io::{Error, ErrorKind, Write},
    num::NonZeroUsize,
};

use lb_api_service::http::{mantle, time};
use lb_chain_service::Slot;
use lb_core::mantle::{SignedMantleTx, transactions::states::Preverified};
use lb_node::{
    RocksBackend, RuntimeServiceId, api::serializers::blocks::ApiProcessedBlockEventOwned,
};
use serde::Serialize;

use crate::{
    LogosBlockchainNode, OperationStatus,
    api::cryptarchia::get_cryptarchia_info_sync,
    errors::OperationStatusCode,
    result::{FfiStatusResult, StatusResult},
    return_error_if_null_pointer, unwrap_or_return_error,
};

/// Matches the maximum page size accepted by the Inspector catalog reader.
const MAX_FINALIZED_BLOCKS_RANGE_LIMIT: usize = 100;
/// Matches the maximum direct range-response budget accepted by the Inspector.
const MAX_FINALIZED_BLOCKS_RANGE_BYTES: usize = 64 * 1024 * 1024;

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    limit: usize,
    limit_exceeded: bool,
}

impl BoundedJsonBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            limit_exceeded: false,
        }
    }
}

impl Write for BoundedJsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.limit_exceeded = true;
            return Err(Error::new(
                ErrorKind::WriteZero,
                "JSON response exceeds its configured byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn json_cstring<Value: Serialize>(
    value: &Value,
    label: &str,
    max_bytes: usize,
) -> StatusResult<CString> {
    let mut buffer = BoundedJsonBuffer::new(max_bytes);
    if let Err(error) = serde_json::to_writer(&mut buffer, value) {
        let code = if buffer.limit_exceeded {
            OperationStatusCode::ValidationError
        } else {
            OperationStatusCode::RuntimeError
        };
        return Err(OperationStatus::error(
            code,
            format!("Failed to serialize {label}: {error}"),
        ));
    }

    CString::new(buffer.bytes).map_err(|error| {
        OperationStatus::error(
            OperationStatusCode::RuntimeError,
            format!("Failed to create {label} C string: {error}"),
        )
    })
}

fn finalized_blocks_limit(
    from_slot: u64,
    to_slot: u64,
    blocks_limit: u64,
) -> StatusResult<NonZeroUsize> {
    if from_slot > to_slot {
        return Err(OperationStatus::error(
            OperationStatusCode::ValidationError,
            "from_slot must not be greater than to_slot.",
        ));
    }

    let blocks_limit = usize::try_from(blocks_limit).map_err(|_| {
        OperationStatus::error(
            OperationStatusCode::ValidationError,
            "blocks_limit overflow.",
        )
    })?;
    let blocks_limit = NonZeroUsize::new(blocks_limit).ok_or_else(|| {
        OperationStatus::error(
            OperationStatusCode::ValidationError,
            "blocks_limit must be greater than zero.",
        )
    })?;
    if blocks_limit.get() > MAX_FINALIZED_BLOCKS_RANGE_LIMIT {
        return Err(OperationStatus::error(
            OperationStatusCode::ValidationError,
            format!("blocks_limit exceeds {MAX_FINALIZED_BLOCKS_RANGE_LIMIT} finalized blocks."),
        ));
    }

    Ok(blocks_limit)
}

fn ensure_finalized_range_within_lib(to_slot: u64, lib_slot: Slot) -> StatusResult<()> {
    if Slot::new(to_slot) > lib_slot {
        return Err(OperationStatus::error(
            OperationStatusCode::ValidationError,
            format!(
                "to_slot {to_slot} exceeds captured LIB slot {}.",
                u64::from(lib_slot)
            ),
        ));
    }

    Ok(())
}

pub(crate) fn get_time_info_sync(node: &LogosBlockchainNode) -> StatusResult<CString> {
    let runtime_handle = node.get_runtime_handle();
    let time_info = runtime_handle
        .block_on(time::time_info::<lb_node::TimeService, RuntimeServiceId>(
            node.get_overwatch_handle(),
        ))
        .map_err(|error| {
            OperationStatus::error(
                OperationStatusCode::RelayError,
                format!("Failed to get time info: {error}"),
            )
        })?;

    json_cstring(&time_info, "time info", MAX_FINALIZED_BLOCKS_RANGE_BYTES)
}

pub(crate) fn get_finalized_blocks_range_sync(
    node: &LogosBlockchainNode,
    from_slot: u64,
    to_slot: u64,
    blocks_limit: u64,
) -> StatusResult<CString> {
    let blocks_limit = finalized_blocks_limit(from_slot, to_slot, blocks_limit)?;
    let chain_info = get_cryptarchia_info_sync(node)?;
    ensure_finalized_range_within_lib(to_slot, chain_info.cryptarchia_info.lib_slot)?;
    let runtime_handle = node.get_runtime_handle();
    let blocks = runtime_handle
        .block_on(mantle::get_blocks_in_slot_range_with_snapshot::<
            SignedMantleTx<Preverified>,
            RocksBackend,
            RuntimeServiceId,
        >(
            node.get_overwatch_handle(),
            Slot::new(from_slot),
            Slot::new(to_slot),
            false,
            blocks_limit,
            true,
            &chain_info.cryptarchia_info,
        ))
        .map_err(|error| {
            OperationStatus::error(
                OperationStatusCode::RelayError,
                format!("Failed to get finalized blocks range: {error}"),
            )
        })?;
    let events: Vec<ApiProcessedBlockEventOwned<Preverified>> = blocks
        .into_iter()
        .map(ApiProcessedBlockEventOwned::from)
        .collect();

    json_cstring(
        &events,
        "finalized blocks range",
        MAX_FINALIZED_BLOCKS_RANGE_BYTES,
    )
}

/// Result type for [`get_time_info`]. On success, `value` is a pointer to a
/// NUL-terminated JSON string matching `/time/info`.
pub type FfiGetTimeInfoResult = FfiStatusResult<*mut c_char>;

/// Reads the current time-service snapshot as JSON.
///
/// The result uses the existing C-string allocation contract and must be
/// released with [`free_cstring`](super::free_cstring).
///
/// # Safety
///
/// `node` must be a valid, non-null [`LogosBlockchainNode`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_time_info(node: *const LogosBlockchainNode) -> FfiGetTimeInfoResult {
    return_error_if_null_pointer!(node);
    // SAFETY: the public function contract requires a live, initialized node.
    let node = unsafe { &*node };
    let json_cstring = unwrap_or_return_error!(get_time_info_sync(node));
    FfiGetTimeInfoResult::ok(json_cstring.into_raw())
}

/// Result type for [`get_finalized_blocks_range`]. On success, `value` is a
/// pointer to a NUL-terminated JSON array of processed block events.
pub type FfiGetFinalizedBlocksRangeResult = FfiStatusResult<*mut c_char>;

/// Reads an ascending immutable block range from one finalized chain snapshot.
///
/// `blocks_limit` must be in `1..=100`; the function rejects a target above
/// the captured LIB rather than silently changing the requested range. Each
/// event has the same `{ block, tip, tip_slot, lib, lib_slot }` shape as the
/// direct blocks-range API. The returned string must be released with
/// [`free_cstring`](super::free_cstring).
///
/// # Safety
///
/// `node` must be a valid, non-null [`LogosBlockchainNode`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_finalized_blocks_range(
    node: *const LogosBlockchainNode,
    from_slot: u64,
    to_slot: u64,
    blocks_limit: u64,
) -> FfiGetFinalizedBlocksRangeResult {
    return_error_if_null_pointer!(node);
    // SAFETY: the public function contract requires a live, initialized node.
    let node = unsafe { &*node };
    let json_cstring = unwrap_or_return_error!(get_finalized_blocks_range_sync(
        node,
        from_slot,
        to_slot,
        blocks_limit,
    ));
    FfiGetFinalizedBlocksRangeResult::ok(json_cstring.into_raw())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use lb_api_service::http::mantle::BlockWithChainState;
    use lb_core::{
        block::{Block, BlockTransactions, UncleHeaders},
        header::HeaderId,
        proofs::leader_proof::Groth16LeaderProof,
    };
    use lb_key_management_system_keys::keys::Ed25519Key;

    use super::*;

    fn api_block(slot: u64, parent: HeaderId) -> Block<SignedMantleTx<Preverified>> {
        let signing_key = Ed25519Key::from_bytes(&[0; 32]);
        let mut proof = serde_json::to_value(Groth16LeaderProof::genesis())
            .expect("genesis leader proof should serialize");
        proof["leader_key"] = serde_json::to_value(signing_key.public_key())
            .expect("leader public key should serialize");
        let proof = serde_json::from_value(proof).expect("leader proof should serialize");

        Block::<SignedMantleTx<Preverified>>::create(
            parent,
            Slot::new(slot),
            UncleHeaders::empty(),
            proof,
            BlockTransactions::empty(),
            &signing_key,
        )
        .expect("test block should be constructible")
    }

    #[test]
    fn finalized_range_rejects_reversed_slots() {
        let result = finalized_blocks_limit(11, 10, 1);

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }

    #[test]
    fn finalized_range_rejects_zero_limit() {
        let result = finalized_blocks_limit(0, 10, 0);

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }

    #[test]
    fn finalized_range_rejects_a_page_larger_than_the_catalog_contract() {
        let result = finalized_blocks_limit(0, 100, 101);

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }

    #[test]
    fn finalized_range_accepts_the_catalog_page_limit() {
        let result = finalized_blocks_limit(0, 100, 100);

        assert!(matches!(result, Ok(limit) if limit.get() == 100));
    }

    #[test]
    fn finalized_range_rejects_a_target_beyond_the_captured_lib() {
        let result = ensure_finalized_range_within_lib(42, Slot::new(41));

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }

    #[test]
    fn bounded_json_serialization_rejects_an_oversized_result() {
        let result = json_cstring(&"abcdef", "test result", 5);

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }

    #[test]
    fn finalized_range_serialization_preserves_processed_event_contract() {
        let tip = HeaderId::from([9; 32]);
        let lib = HeaderId::from([8; 32]);
        let first_block = api_block(40, HeaderId::from([0; 32]));
        let second_block = api_block(41, first_block.header().id());
        let events = vec![
            ApiProcessedBlockEventOwned::from(BlockWithChainState {
                block: first_block,
                tip,
                tip_slot: Slot::new(42),
                lib,
                lib_slot: Slot::new(41),
            }),
            ApiProcessedBlockEventOwned::from(BlockWithChainState {
                block: second_block,
                tip,
                tip_slot: Slot::new(42),
                lib,
                lib_slot: Slot::new(41),
            }),
        ];

        let json = json_cstring(
            &events,
            "finalized blocks range",
            MAX_FINALIZED_BLOCKS_RANGE_BYTES,
        )
        .expect("processed events should serialize");
        let value: serde_json::Value =
            serde_json::from_str(json.to_str().expect("JSON must be UTF-8"))
                .expect("serialized events must be JSON");
        let events = value.as_array().expect("range result must be an array");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["block"]["header"]["slot"], 40);
        assert_eq!(events[1]["block"]["header"]["slot"], 41);
        for event in events {
            let keys = event
                .as_object()
                .expect("each range item must be an object")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                keys,
                BTreeSet::from(["block", "tip", "tip_slot", "lib", "lib_slot"])
            );
            assert!(event["block"]["header"]["id"].is_string());
            assert_eq!(event["tip"], serde_json::json!(tip));
            assert_eq!(event["tip_slot"], 42);
            assert_eq!(event["lib"], serde_json::json!(lib));
            assert_eq!(event["lib_slot"], 41);
        }
    }
}

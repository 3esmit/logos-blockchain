use std::ffi::{CString, c_char};

use lb_core::{
    header::HeaderId as CoreHeaderId,
    mantle::{SignedMantleTx, Transaction},
};
use lb_node::RuntimeServiceId;
use lb_tx_service::storage::adapters::RocksStorageAdapter;
use serde::Serialize;

use crate::{
    LogosBlockchainNode,
    api::cryptarchia::HeaderId,
    errors::{OperationStatus, OperationStatusCode},
    result::{FfiStatusResult, StatusResult},
    return_error_if_null_pointer, unwrap_or_return_error,
};

/// Result type for diagnostic reads. On success, `value` is an allocated
/// NUL-terminated C string containing JSON. Callers free it with
/// [`free_cstring`](super::free_cstring).
pub type FfiDiagnosticJsonResult = FfiStatusResult<*mut c_char>;

fn serialize_json<Value>(value: &Value, operation: &str) -> StatusResult<CString>
where
    Value: Serialize,
{
    let json = serde_json::to_string(value).map_err(|error| {
        OperationStatus::error(
            OperationStatusCode::RuntimeError,
            format!("Failed to serialize {operation}: {error}"),
        )
    })?;

    CString::new(json).map_err(|error| {
        OperationStatus::error(
            OperationStatusCode::RuntimeError,
            format!("Failed to create C string for {operation}: {error}"),
        )
    })
}

fn get_cryptarchia_headers_sync(
    node: &LogosBlockchainNode,
    from_descendant: Option<HeaderId>,
    to_ancestor: Option<HeaderId>,
) -> StatusResult<CString> {
    let runtime_handle = node.get_runtime_handle();
    let headers = runtime_handle
        .block_on(lb_api_service::http::consensus::cryptarchia_headers::<
            RuntimeServiceId,
        >(
            node.get_overwatch_handle(),
            from_descendant.map(CoreHeaderId::from),
            to_ancestor.map(CoreHeaderId::from),
        ))
        .map_err(|error| {
            OperationStatus::error(
                OperationStatusCode::RelayError,
                format!("Failed to get cryptarchia headers: {error}"),
            )
        })?;

    serialize_json(&headers, "cryptarchia headers")
}

fn get_network_info_sync(node: &LogosBlockchainNode) -> StatusResult<CString> {
    let runtime_handle = node.get_runtime_handle();
    let network_info = runtime_handle
        .block_on(
            lb_api_service::http::libp2p::libp2p_info::<RuntimeServiceId>(
                node.get_overwatch_handle(),
            ),
        )
        .map_err(|error| {
            OperationStatus::error(
                OperationStatusCode::RelayError,
                format!("Failed to get network information: {error}"),
            )
        })?;

    serialize_json(&network_info, "network information")
}

fn get_mantle_metrics_sync(node: &LogosBlockchainNode) -> StatusResult<CString> {
    let runtime_handle = node.get_runtime_handle();
    let metrics = runtime_handle
        .block_on(lb_api_service::http::mantle::mantle_mempool_metrics::<
            RocksStorageAdapter<SignedMantleTx, <SignedMantleTx as Transaction>::Hash>,
            RuntimeServiceId,
        >(node.get_overwatch_handle()))
        .map_err(|error| {
            OperationStatus::error(
                OperationStatusCode::RelayError,
                format!("Failed to get Mantle metrics: {error}"),
            )
        })?;

    serialize_json(&metrics, "Mantle metrics")
}

/// Gets bounded Cryptarchia header identifiers as a JSON array.
///
/// A null `from_descendant` or `to_ancestor` represents the corresponding
/// omitted direct-node query parameter. The result has the same data shape as
/// the node's Cryptarchia headers endpoint.
///
/// # Safety
///
/// `node` must point to a live [`LogosBlockchainNode`] allocated by this
/// library. When non-null, `from_descendant` and `to_ancestor` must each point
/// to a readable, initialized 32-byte [`HeaderId`] for the duration of this
/// call. The returned `value` must be freed with
/// [`free_cstring`](super::free_cstring).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_cryptarchia_headers(
    node: *const LogosBlockchainNode,
    from_descendant: *const HeaderId,
    to_ancestor: *const HeaderId,
) -> FfiDiagnosticJsonResult {
    return_error_if_null_pointer!(node);
    // SAFETY: the public function contract requires a live, aligned,
    // initialized LogosBlockchainNode whenever `node` is non-null.
    let node = unsafe { &*node };
    let from_descendant = if from_descendant.is_null() {
        None
    } else {
        // SAFETY: a non-null optional header pointer is required by the public
        // function contract to reference one initialized HeaderId.
        Some(unsafe { *from_descendant })
    };
    let to_ancestor = if to_ancestor.is_null() {
        None
    } else {
        // SAFETY: a non-null optional header pointer is required by the public
        // function contract to reference one initialized HeaderId.
        Some(unsafe { *to_ancestor })
    };
    let value = unwrap_or_return_error!(get_cryptarchia_headers_sync(
        node,
        from_descendant,
        to_ancestor,
    ));
    FfiDiagnosticJsonResult::ok(value.into_raw())
}

/// Gets Libp2p network information as JSON.
///
/// # Safety
///
/// `node` must point to a live [`LogosBlockchainNode`] allocated by this
/// library. The returned `value` must be freed with
/// [`free_cstring`](super::free_cstring).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_network_info(
    node: *const LogosBlockchainNode,
) -> FfiDiagnosticJsonResult {
    return_error_if_null_pointer!(node);
    // SAFETY: the public function contract requires a live, aligned,
    // initialized LogosBlockchainNode whenever `node` is non-null.
    let node = unsafe { &*node };
    let value = unwrap_or_return_error!(get_network_info_sync(node));
    FfiDiagnosticJsonResult::ok(value.into_raw())
}

/// Gets Mantle mempool metrics as JSON.
///
/// # Safety
///
/// `node` must point to a live [`LogosBlockchainNode`] allocated by this
/// library. The returned `value` must be freed with
/// [`free_cstring`](super::free_cstring).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_mantle_metrics(
    node: *const LogosBlockchainNode,
) -> FfiDiagnosticJsonResult {
    return_error_if_null_pointer!(node);
    // SAFETY: the public function contract requires a live, aligned,
    // initialized LogosBlockchainNode whenever `node` is non-null.
    let node = unsafe { &*node };
    let value = unwrap_or_return_error!(get_mantle_metrics_sync(node));
    FfiDiagnosticJsonResult::ok(value.into_raw())
}

#[cfg(test)]
mod tests {
    use super::serialize_json;

    #[test]
    fn diagnostic_json_is_a_c_string() {
        let value = serialize_json(&serde_json::json!({"peers": 2}), "test diagnostic")
            .expect("serializable JSON must be representable as a C string");
        assert_eq!(value.to_bytes_with_nul(), b"{\"peers\":2}\0");
    }
}

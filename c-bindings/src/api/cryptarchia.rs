use crate::{
    LogosBlockchainNode,
    api::free,
    errors::{OperationStatus, OperationStatusCode},
    result::{FfiStatusResult, StatusResult},
    return_error_if_null_pointer, unwrap_or_return_error,
};

#[repr(C)]
pub enum State {
    Bootstrapping = 0x0,
    Online = 0x1,
    NotStarted = 0x2,
}

impl From<lb_chain_service::ChainServiceMode> for State {
    fn from(value: lb_chain_service::ChainServiceMode) -> Self {
        match value {
            lb_chain_service::ChainServiceMode::AwaitingStart => Self::NotStarted,
            lb_chain_service::ChainServiceMode::Started(inner_state) => match inner_state {
                lb_chain_service::State::Bootstrapping => Self::Bootstrapping,
                lb_chain_service::State::Online => Self::Online,
            },
        }
    }
}

pub type Hash = [u8; 32];
pub type HeaderId = Hash;
pub type TxHash = Hash;
/// A note (UTXO) identifier, as 32 little-endian bytes. The FFI representation
/// of [`lb_core::mantle::NoteId`].
pub type NoteId = Hash;

/// Converts a raw pointer to a `TxHash` into a `lb_core::mantle::TxHash`.
///
/// # Parameters
///
/// - `tx_hash`: A raw pointer to a `TxHash` (32-byte array).
///
/// # Returns
///
/// - A `lb_core::mantle::TxHash` if successful, or an
///   `OperationStatusCode::ValidationError` if the conversion fails.
///
/// # Safety
///
/// This function is unsafe because it dereferences a raw pointer.
/// The caller must ensure that the pointer is valid and points to a properly
/// initialized `TxHash`.
pub(crate) unsafe fn into_tx_hash(tx_hash: *const TxHash) -> lb_core::mantle::TxHash {
    lb_core::mantle::TxHash::from(unsafe { *tx_hash })
}

#[repr(C)]
pub struct CryptarchiaInfo {
    pub lib: HeaderId,
    pub tip: HeaderId,
    pub slot: u64,
    pub height: u64,
    pub mode: State,
    // Appended to retain the offsets of existing fields. Callers that read
    // this field must use a matching C library that allocates it.
    pub genesis_id: HeaderId,
}

/// Version of the `CryptarchiaInfo` C ABI layout.
///
/// Consumers must check this before dereferencing a value returned by
/// [`get_cryptarchia_info`]. Increment it whenever that layout changes.
pub const CRYPTARCHIA_INFO_ABI_VERSION: u32 = 1;

/// Gets the version of the `CryptarchiaInfo` C ABI layout.
///
/// Consumers use this to reject incompatible libraries before reading a
/// `CryptarchiaInfo` allocation.
#[unsafe(no_mangle)]
pub extern "C" fn cryptarchia_info_abi_version() -> u32 {
    CRYPTARCHIA_INFO_ABI_VERSION
}

impl TryFrom<lb_chain_service::ChainServiceInfo> for CryptarchiaInfo {
    type Error = OperationStatus;

    fn try_from(value: lb_chain_service::ChainServiceInfo) -> Result<Self, Self::Error> {
        let genesis_id = value.cryptarchia_info.genesis_id.ok_or_else(|| {
            OperationStatus::error(
                OperationStatusCode::ValidationError,
                "Cryptarchia info omitted its genesis identity; use a matching node and C library version.",
            )
        })?;

        Ok(Self {
            lib: value.cryptarchia_info.lib.into(),
            tip: value.cryptarchia_info.tip.into(),
            slot: u64::from(value.cryptarchia_info.slot),
            height: value.cryptarchia_info.height,
            mode: State::from(value.mode),
            genesis_id: genesis_id.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_preserves_genesis_identity() {
        let genesis_id = lb_core::header::HeaderId::from([1; 32]);
        let info = lb_chain_service::ChainServiceInfo {
            cryptarchia_info: lb_chain_service::CryptarchiaInfo {
                genesis_id: Some(genesis_id),
                lib: lb_core::header::HeaderId::from([2; 32]),
                lib_slot: lb_chain_service::Slot::new(3),
                tip: lb_core::header::HeaderId::from([4; 32]),
                slot: lb_chain_service::Slot::new(5),
                height: 6,
            },
            mode: lb_chain_service::ChainServiceMode::Started(lb_chain_service::State::Online),
        };

        let ffi = CryptarchiaInfo::try_from(info).expect("genesis identity should be present");

        assert_eq!(ffi.genesis_id, [1; 32]);
    }

    #[test]
    fn cryptarchia_info_abi_version_matches_the_current_layout() {
        assert_eq!(cryptarchia_info_abi_version(), CRYPTARCHIA_INFO_ABI_VERSION);
    }

    #[test]
    fn conversion_rejects_missing_genesis_identity() {
        let info = lb_chain_service::ChainServiceInfo {
            cryptarchia_info: lb_chain_service::CryptarchiaInfo {
                genesis_id: None,
                lib: lb_core::header::HeaderId::from([2; 32]),
                lib_slot: lb_chain_service::Slot::new(3),
                tip: lb_core::header::HeaderId::from([4; 32]),
                slot: lb_chain_service::Slot::new(5),
                height: 6,
            },
            mode: lb_chain_service::ChainServiceMode::Started(lb_chain_service::State::Online),
        };

        let result = CryptarchiaInfo::try_from(info);

        assert!(matches!(
            result,
            Err(OperationStatus {
                code: OperationStatusCode::ValidationError,
                ..
            })
        ));
    }
}

/// Gets the current Cryptarchia info.
///
/// This is a synchronous wrapper around the asynchronous
/// [`cryptarchia_info`](lb_api_service::http::consensus::cryptarchia_info)
/// function.
///
/// # Arguments
///
/// - `node`: A [`LogosBlockchainNode`] instance.
///
/// # Returns
///
/// A `Result` containing the [`ChainServiceInfo`] on success, or an
/// [`OperationStatus`] error on failure.
pub(crate) fn get_cryptarchia_info_sync(
    node: &LogosBlockchainNode,
) -> StatusResult<lb_chain_service::ChainServiceInfo> {
    let runtime_handle = node.get_runtime_handle();

    let Ok(info) = runtime_handle.block_on(lb_api_service::http::consensus::cryptarchia_info(
        node.get_overwatch_handle(),
    )) else {
        return Err(OperationStatus::error(
            OperationStatusCode::RelayError,
            "Failed to get cryptarchia info.",
        ));
    };

    Ok(info)
}

pub type FfiCryptarchiaInfoResult = FfiStatusResult<*mut CryptarchiaInfo>;

/// Get the current Cryptarchia info.
///
/// # Arguments
///
/// - `node`: A non-null pointer to a [`LogosBlockchainNode`].
///
/// # Returns
///
/// A [`FfiCryptarchiaInfoResult`] containing a pointer to the allocated
/// [`CryptarchiaInfo`] struct on success, or an [`OperationStatus`] error on
/// failure.
///
/// # Safety
///
/// This function is unsafe because it dereferences raw pointers.
/// The caller must ensure that all pointers are non-null and point to valid
/// memory.
///
/// # Memory Management
///
/// This function allocates memory for the output [`CryptarchiaInfo`] struct.
/// The caller must free this memory using the [`free_cryptarchia_info`]
/// function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_cryptarchia_info(
    node: *const LogosBlockchainNode,
) -> FfiCryptarchiaInfoResult {
    return_error_if_null_pointer!(node);
    let node = unsafe { &*node };
    let service_info = unwrap_or_return_error!(get_cryptarchia_info_sync(node));
    let c_info = unwrap_or_return_error!(CryptarchiaInfo::try_from(service_info));

    FfiCryptarchiaInfoResult::from_value(c_info)
}

/// Frees the memory allocated for a [`CryptarchiaInfo`] struct.
///
/// # Arguments
///
/// - `pointer`: A pointer to the [`CryptarchiaInfo`] struct to be freed.
#[unsafe(no_mangle)]
pub extern "C" fn free_cryptarchia_info(pointer: *mut CryptarchiaInfo) -> OperationStatus {
    free::<CryptarchiaInfo>(pointer)
}

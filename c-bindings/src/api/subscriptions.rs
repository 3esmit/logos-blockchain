use std::ffi::c_char;

use lb_api_service::http::storage::StorageAdapter as _;
use lb_chain_service::api::CryptarchiaServiceApi;
use lb_core::{
    block::{Block as CoreBlock, BlockTransactions},
    mantle::{
        traits::{Hashable, StorageSize, hashable},
        transactions::{hash::TxHash, states::Unverified},
    },
};
use lb_node::{
    ApiStorageAdapter, RuntimeServiceId, SignedMantleTx, StorageService,
    generic_services::CryptarchiaService,
};
use serde::Serialize;

use crate::{
    LogosBlockchainNode, OperationStatus,
    api::types::block::Block,
    callbacks::{BoxedCallback, CCallback, into_boxed_callback},
    errors::OperationStatusCode,
    logging, return_error_if_null_pointer,
};

#[derive(Serialize)]
#[serde(rename = "SignedMantleTx")]
#[derive(Clone)]
pub struct TxWithId {
    id: TxHash,
    #[serde(flatten)]
    tx: SignedMantleTx<Unverified>,
}

impl TxWithId {
    pub(crate) fn new(tx: SignedMantleTx<Unverified>) -> Self {
        let id = tx.hash();
        Self { id, tx }
    }
}

impl Hashable for TxWithId {
    const HASHER: hashable::Hasher<Self> = |tx| tx.id;
    type Hash = <SignedMantleTx<Unverified> as Hashable>::Hash;

    fn as_signing(&self) -> Vec<u8> {
        self.tx.as_signing()
    }
}

impl StorageSize for TxWithId {
    fn storage_size(&self) -> usize {
        self.tx.storage_size()
    }
}

#[must_use]
pub fn subscribe_to_new_blocks_sync(
    node: &LogosBlockchainNode,
    mut callback_per_block: BoxedCallback<*const c_char>,
) -> OperationStatus {
    let runtime_handler = node.get_runtime_handle();
    let overwatch = node.get_overwatch_handle();
    runtime_handler.block_on(async move {
        let Ok(relay) = overwatch
            .relay::<CryptarchiaService<RuntimeServiceId>>()
            .await
        else {
            return OperationStatus::error(
                OperationStatusCode::RelayError,
                "Failed to get relay to CryptarchiaService.",
            );
        };
        let Ok(storage_relay) = overwatch.relay::<StorageService>().await else {
            return OperationStatus::error(
                OperationStatusCode::RelayError,
                "Failed to get relay to StorageService.",
            );
        };
        let api =
            CryptarchiaServiceApi::<CryptarchiaService<RuntimeServiceId>, RuntimeServiceId>::new(
                relay,
            );
        match api.subscribe_new_blocks().await {
            Ok(mut block_stream) => {
                runtime_handler.spawn(async move {
                    while let Ok(event) = block_stream.recv().await {
                        let relay = storage_relay.clone();
                        let res: Result<Option<CoreBlock<SignedMantleTx<Unverified>>>, _> =
                            ApiStorageAdapter::<RuntimeServiceId>::get_block(relay, event.block_id)
                                .await;
                        if let Ok(Some(block)) = res {
                            let header = block.header().clone();
                            let signature = *block.signature();
                            let txs_with_id: Vec<TxWithId> = block
                                .into_transactions()
                                .into_iter()
                                .map(TxWithId::new)
                                .collect();
                            let block: CoreBlock<TxWithId> = CoreBlock::reconstruct(
                                header,
                                BlockTransactions::try_from(txs_with_id)
                                    .expect("Block should always build from valid block"),
                                signature,
                            )
                            .expect("Block should always build from valid block");
                            callback_per_block(Block::from(block).as_ptr());
                        } else {
                            logging::error!(
                                "subscribe_to_new_blocks_sync",
                                "Failed to get block {:?} from storage",
                                event.block_id
                            );
                        }
                    }
                    logging::warning!(
                        "subscribe_to_new_blocks_sync",
                        "Block stream closed, subscription to new blocks ended."
                    );
                });
                OperationStatus::OK
            }
            Err(e) => OperationStatus::error(
                OperationStatusCode::ServiceError,
                format!("Failed to subscribe to blocks: {e}"),
            ),
        }
    })
}

/// Subscribes to new blocks on the blockchain and calls the provided callback
/// for each new block.
///
/// # Arguments
///
/// - `node`: A non-null pointer to a running [`LogosBlockchainNode`] instance.
/// - `callback_per_block`: A callback function that will be called with a
///   pointer to a C string containing the JSON representation of each new
///   block. The callback is declared as unsafe extern "C" and must be
///   thread-safe.
///
/// # Returns
///
/// An [`OperationStatus`] indicating success or failure.
///
/// # Safety
///
/// This function is unsafe because it dereferences raw pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn subscribe_to_new_blocks(
    node: *const LogosBlockchainNode,
    callback_per_block: CCallback<*const c_char>,
) -> OperationStatus {
    return_error_if_null_pointer!(node);
    let node = unsafe { &*node };
    let callback_per_block = into_boxed_callback(callback_per_block);
    subscribe_to_new_blocks_sync(node, callback_per_block)
}

#[cfg(test)]
mod tests {
    use lb_core::mantle::{traits::Hashable as _, transactions::states::Unverified};

    use super::{SignedMantleTx, TxHash, TxWithId};

    #[test]
    fn transaction_with_id_serializes_the_hash_accepted_by_get_transaction() {
        let transaction = serde_json::from_value::<SignedMantleTx<Unverified>>(serde_json::json!({
            "mantle_tx": { "ops": [] },
            "ops_proofs": []
        }))
        .expect("empty transaction should deserialize");
        let expected_hash = transaction.hash();
        let original =
            serde_json::to_value(&transaction).expect("signed transaction should serialize");
        let transaction_with_id = TxWithId::new(transaction);
        let serialized = serde_json::to_value(&transaction_with_id)
            .expect("transaction with id should serialize");
        let emitted_id = serde_json::from_value::<TxHash>(serialized["id"].clone())
            .expect("emitted id should deserialize as a transaction hash");

        assert!(serialized["id"].as_str().is_some_and(|id| id.len() == 64));
        assert_eq!(emitted_id, expected_hash);
        assert_eq!(transaction_with_id.hash(), expected_hash);
        assert_eq!(serialized["mantle_tx"], original["mantle_tx"]);
        assert_eq!(serialized["ops_proofs"], original["ops_proofs"]);
    }

    #[test]
    fn transaction_with_id_hash_returns_the_stored_id() {
        let transaction = serde_json::from_value::<SignedMantleTx<Unverified>>(serde_json::json!({
            "mantle_tx": { "ops": [] },
            "ops_proofs": []
        }))
        .expect("empty transaction should deserialize");
        let stored_id = TxHash::default();
        let transaction_with_id = TxWithId {
            id: stored_id,
            tx: transaction,
        };

        assert_eq!(transaction_with_id.hash(), stored_id);
    }
}

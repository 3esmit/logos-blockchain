use lb_core::mantle::{
    MantleTx, OpProof, SignedMantleTx, TxHash,
    transactions::{Ops, states::VerificationState},
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(remote = "MantleTx")]
pub struct ApiTransactionSerializer {
    #[serde(getter = "<MantleTx as lb_core::mantle::Transaction>::hash")]
    hash: TxHash,
    #[serde(getter = "MantleTx::ops")]
    ops: Ops,
}

#[derive(Serialize)]
pub struct ApiSignedTransaction<'tx> {
    #[serde(with = "ApiTransactionSerializer")]
    mantle_tx: &'tx MantleTx,
    ops_proofs: &'tx Vec<OpProof>,
}

impl<'tx, State: VerificationState> From<&'tx SignedMantleTx<State>> for ApiSignedTransaction<'tx> {
    fn from(value: &'tx SignedMantleTx<State>) -> Self {
        Self {
            mantle_tx: value.mantle_tx(),
            ops_proofs: value.ops_proofs(),
        }
    }
}

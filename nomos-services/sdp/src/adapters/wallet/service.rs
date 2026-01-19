use std::convert::Infallible;

use key_management_system_keys::keys::ZkPublicKey;
use nomos_core::{
    header::HeaderId,
    mantle::{Op, SignedMantleTx, tx_builder::MantleTxBuilder},
    sdp::{ActiveMessage, DeclarationMessage, WithdrawMessage},
};
use nomos_wallet::api::{WalletApi, WalletServiceData};

use crate::adapters::wallet::SdpWalletAdapter;

pub struct ServiceWalletAdapter<Wallet, RuntimeServiceId>
where
    Wallet: WalletServiceData,
{
    wallet_api: WalletApi<Wallet, RuntimeServiceId>,
}

impl<Wallet, RuntimeServiceId> SdpWalletAdapter for ServiceWalletAdapter<Wallet, RuntimeServiceId>
where
    Wallet: WalletServiceData,
{
    type Error = Infallible;
    type WalletApi = WalletApi<Wallet, RuntimeServiceId>;

    fn new(wallet_api: Self::WalletApi) -> Self {
        Self { wallet_api }
    }
    fn declare_tx(
        &self,
        tip: HeaderId,
        change_pk: ZkPublicKey,
        funding_pks: Vec<ZkPublicKey>,
        tx_builder: MantleTxBuilder,
        declaration: Box<DeclarationMessage>,
    ) -> Result<SignedMantleTx, Self::Error> {
        let _mantle_tx = tx_builder.push_op(Op::SDPDeclare(*declaration)).build();
        //self.wallet_api.fund_and_sign_tx()
        todo!()
    }

    fn withdraw_tx(
        &self,
        tip: HeaderId,
        change_pk: ZkPublicKey,
        funding_pks: Vec<ZkPublicKey>,
        tx_builder: MantleTxBuilder,
        withdrawn_message: WithdrawMessage,
    ) -> Result<SignedMantleTx, Self::Error> {
        todo!()
    }

    fn active_tx(
        &self,
        tip: HeaderId,
        change_pk: ZkPublicKey,
        funding_pks: Vec<ZkPublicKey>,
        tx_builder: MantleTxBuilder,
        active_message: ActiveMessage,
    ) -> Result<SignedMantleTx, Self::Error> {
        todo!()
    }
}

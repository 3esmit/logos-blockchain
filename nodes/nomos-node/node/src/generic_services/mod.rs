use chain_leader::CryptarchiaLeader;
use chain_network::network::adapters::libp2p::LibP2pAdapter;
use chain_service::CryptarchiaConsensus;
use key_management_system_service::backend::preload::PreloadKMSBackend;
use nomos_core::{
    header::HeaderId,
    mantle::{SignedMantleTx, Transaction, TxHash},
};
use nomos_sdp::adapters::mempool::sdp::SdpMempoolNetworkAdapter;
use nomos_storage::backends::rocksdb::RocksBackend;
use nomos_time::backends::NtpTimeBackend;
use tx_service::{backend::pool::Mempool, storage::adapters::rocksdb::RocksStorageAdapter};

use crate::MB16;

pub mod blend;

pub type TxMempoolService<RuntimeServiceId> = tx_service::TxMempoolService<
    tx_service::network::adapters::libp2p::Libp2pAdapter<
        SignedMantleTx,
        <SignedMantleTx as Transaction>::Hash,
        RuntimeServiceId,
    >,
    Mempool<
        HeaderId,
        SignedMantleTx,
        TxHash,
        RocksStorageAdapter<SignedMantleTx, <SignedMantleTx as Transaction>::Hash>,
        RuntimeServiceId,
    >,
    RocksStorageAdapter<SignedMantleTx, <SignedMantleTx as Transaction>::Hash>,
    RuntimeServiceId,
>;

pub type TimeService<RuntimeServiceId> = nomos_time::TimeService<NtpTimeBackend, RuntimeServiceId>;

pub type MempoolAdapter<RuntimeServiceId> = tx_service::network::adapters::libp2p::Libp2pAdapter<
    SignedMantleTx,
    <SignedMantleTx as Transaction>::Hash,
    RuntimeServiceId,
>;

pub type MempoolBackend<RuntimeServiceId> = Mempool<
    HeaderId,
    SignedMantleTx,
    <SignedMantleTx as Transaction>::Hash,
    RocksStorageAdapter<SignedMantleTx, <SignedMantleTx as Transaction>::Hash>,
    RuntimeServiceId,
>;

pub type CryptarchiaService<RuntimeServiceId> =
    CryptarchiaConsensus<SignedMantleTx, RocksBackend, NtpTimeBackend, RuntimeServiceId>;

pub type ChainNetworkService<RuntimeServiceId> = chain_network::ChainNetwork<
    CryptarchiaService<RuntimeServiceId>,
    LibP2pAdapter<SignedMantleTx, RuntimeServiceId>,
    MempoolBackend<RuntimeServiceId>,
    MempoolAdapter<RuntimeServiceId>,
    NtpTimeBackend,
    RuntimeServiceId,
>;

pub type KeyManagementService<RuntimeServiceId> =
    key_management_system_service::KMSService<PreloadKMSBackend, RuntimeServiceId>;

pub type WalletService<Cryptarchia, RuntimeServiceId> = nomos_wallet::WalletService<
    KeyManagementService<RuntimeServiceId>,
    Cryptarchia,
    SignedMantleTx,
    RocksBackend,
    RuntimeServiceId,
>;

pub type CryptarchiaLeaderService<Cryptarchia, Wallet, RuntimeServiceId> = CryptarchiaLeader<
    blend::BlendService<RuntimeServiceId>,
    MempoolBackend<RuntimeServiceId>,
    MempoolAdapter<RuntimeServiceId>,
    nomos_core::mantle::select::FillSize<MB16, SignedMantleTx>,
    NtpTimeBackend,
    Cryptarchia,
    Wallet,
    RuntimeServiceId,
>;

pub type SdpMempoolAdapterGeneric<RuntimeServiceId> = SdpMempoolNetworkAdapter<
    tx_service::network::adapters::libp2p::Libp2pAdapter<SignedMantleTx, TxHash, RuntimeServiceId>,
    Mempool<
        HeaderId,
        SignedMantleTx,
        TxHash,
        RocksStorageAdapter<SignedMantleTx, <SignedMantleTx as Transaction>::Hash>,
        RuntimeServiceId,
    >,
    RuntimeServiceId,
>;

pub type SdpService<RuntimeServiceId> =
    nomos_sdp::SdpService<SdpMempoolAdapterGeneric<RuntimeServiceId>, RuntimeServiceId>;

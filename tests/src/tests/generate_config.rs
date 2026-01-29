//! Utility to generate a valid single-node configuration file.
//!
//! Run with:
//! ```
//! cargo test -p logos-blockchain-tests generate_single_node_config -- --ignored --nocapture
//! ```
//!
//! This will generate a config file at `nodes/node/config-one-node.yaml`.

use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;

use lb_api_service::ApiServiceSettings;
use lb_core::mantle::Value;
use lb_http_api_common::settings::AxumBackendSettings;
use lb_key_management_system_service::keys::secured_key::SecuredKey as _;
use lb_node::{
    RocksBackendSettings, UserConfig,
    config::mempool::serde::Config as MempoolConfig,
};
use lb_sdp_service::SdpSettings;
use lb_wallet_service::WalletServiceSettings;

use logos_blockchain_tests::topology::configs::{GeneralConfig, create_general_configs};

/// Fixed ports for single-node config (to make config stable)
const HTTP_PORT: u16 = 8080;
const TESTING_HTTP_PORT: u16 = 50897;

fn create_user_config(config: GeneralConfig) -> UserConfig {
    let testing_http_address = format!("127.0.0.1:{TESTING_HTTP_PORT}")
        .parse()
        .unwrap();

    let http_address = format!("127.0.0.1:{HTTP_PORT}")
        .parse()
        .unwrap();

    UserConfig {
        network: config.network_config,
        blend: config.blend_config.0,
        time: config.time_config,
        cryptarchia: config.consensus_config.user_config().clone(),
        mempool: MempoolConfig {
            recovery_path: "./recovery/mempool.json".into(),
        },
        tracing: config.tracing_config.tracing_settings,
        http: ApiServiceSettings {
            backend_settings: AxumBackendSettings {
                address: http_address,
                rate_limit_per_second: 10000,
                rate_limit_burst: 10000,
                max_concurrent_requests: 1000,
                ..Default::default()
            },
        },
        storage: RocksBackendSettings {
            db_path: "./db".into(),
            read_only: false,
            column_family: Some("blocks".into()),
        },
        sdp: SdpSettings {
            declaration: None,
            wallet_config: lb_sdp_service::wallet::SdpWalletConfig {
                max_tx_fee: Value::MAX,
                funding_pk: config.consensus_config.funding_sk.as_public_key(),
            },
        },
        wallet: WalletServiceSettings {
            known_keys: HashSet::from_iter([
                config.consensus_config.user_config().leader.pk,
                config.consensus_config.funding_sk.as_public_key(),
            ]),
        },
        key_management: config.kms_config,
        testing_http: ApiServiceSettings {
            backend_settings: AxumBackendSettings {
                address: testing_http_address,
                rate_limit_per_second: 10000,
                rate_limit_burst: 10000,
                max_concurrent_requests: 1000,
                ..Default::default()
            },
        },
    }
}

#[tokio::test]
#[ignore = "Run manually to generate config file"]
async fn generate_single_node_config() {
    // Generate config for 1 node
    let configs = create_general_configs(1);
    let config = configs.into_iter().next().unwrap();

    // Print key info before generating
    println!("\nGenerated Key Information:");
    println!(
        "  Leader PK: {:?}",
        config.consensus_config.user_config().leader.pk
    );
    println!(
        "  Blend Non-Ephemeral Key ID: {}",
        config.blend_config.0.non_ephemeral_signing_key_id
    );
    println!(
        "  Blend ZK Key ID: {}",
        config.blend_config.0.core.zk.secret_key_kms_id
    );
    println!(
        "  SDP Funding PK: {:?}",
        config.consensus_config.funding_sk.as_public_key()
    );
    println!("\nKMS Keys:");
    for (key_id, _) in &config.kms_config.keys {
        println!("  {}", key_id);
    }

    // Create user config
    let user_config = create_user_config(config);

    // Write to file
    let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nodes/node/config-one-node.yaml");

    println!("\nWriting config to: {}", output_path.display());

    let file = File::create(&output_path).expect("Failed to create config file");
    serde_yaml::to_writer(file, &user_config).expect("Failed to write config");

    println!("Config generated successfully!");
}

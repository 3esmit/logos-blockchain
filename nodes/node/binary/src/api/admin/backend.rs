use std::{
    fmt::{Debug, Display},
    io,
    net::SocketAddr,
};

use axum::{
    Router,
    http::{
        HeaderValue,
        header::{CONTENT_TYPE, USER_AGENT},
    },
    routing,
};
use http::StatusCode;
use lb_api_service::Backend;
use lb_core::{
    header::HeaderId,
    mantle::{SignedMantleTx, Transaction},
};
use lb_http_api_common::paths;
use lb_tx_service::{TxMempoolService, backend::Mempool};
use overwatch::{overwatch::handle::OverwatchHandle, services::AsServiceId};
use tokio::net::TcpListener;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level as TracingLevel;

use crate::{
    TracingService, WalletService,
    api::{admin::handlers::reload_tracing_filter, backend::AxumBackendSettings, handlers::wallet},
};

type MempoolStorageAdapter = lb_tx_service::storage::adapters::rocksdb::RocksStorageAdapter<
    SignedMantleTx,
    <SignedMantleTx as Transaction>::Hash,
>;
type AdminMempoolService<RuntimeServiceId> = TxMempoolService<
    lb_tx_service::network::adapters::libp2p::Libp2pAdapter<
        SignedMantleTx,
        <SignedMantleTx as Transaction>::Hash,
        RuntimeServiceId,
    >,
    Mempool<
        HeaderId,
        SignedMantleTx,
        <SignedMantleTx as Transaction>::Hash,
        MempoolStorageAdapter,
        RuntimeServiceId,
    >,
    MempoolStorageAdapter,
    RuntimeServiceId,
>;

pub struct AdminAxumBackend {
    settings: AxumBackendSettings,
}

#[async_trait::async_trait]
impl<RuntimeServiceId> Backend<RuntimeServiceId> for AdminAxumBackend
where
    RuntimeServiceId: Debug
        + Sync
        + Send
        + Display
        + Clone
        + 'static
        + AsServiceId<TracingService>
        + AsServiceId<WalletService>
        + AsServiceId<AdminMempoolService<RuntimeServiceId>>,
{
    type Error = io::Error;
    type Settings = AxumBackendSettings;

    async fn new(settings: Self::Settings) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(Self { settings })
    }

    async fn serve(self, handle: OverwatchHandle<RuntimeServiceId>) -> Result<(), Self::Error> {
        let app = build_router::<RuntimeServiceId>(handle, &self.settings)?;
        let listener = bind_listener(self.settings.address).await?;

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    }
}

fn build_router<RuntimeServiceId>(
    handle: OverwatchHandle<RuntimeServiceId>,
    settings: &AxumBackendSettings,
) -> Result<Router, io::Error>
where
    RuntimeServiceId: Debug
        + Sync
        + Send
        + Display
        + Clone
        + 'static
        + AsServiceId<TracingService>
        + AsServiceId<WalletService>
        + AsServiceId<AdminMempoolService<RuntimeServiceId>>,
{
    let router = Router::new();

    let router = router.route(
        paths::admin::TRACING_FILTER,
        routing::put(reload_tracing_filter::<RuntimeServiceId>),
    );

    let router = router
        .route(
            paths::wallet::BALANCE,
            routing::get(wallet::get_balance::<WalletService, _>),
        )
        .route(
            paths::wallet::TRANSACTIONS_TRANSFER_FUNDS,
            routing::post(
                wallet::post_transactions_transfer_funds::<WalletService, MempoolStorageAdapter, _>,
            ),
        )
        .route(
            paths::wallet::SIGN_TX_ED25519,
            routing::post(wallet::sign_tx_ed25519::<WalletService, MempoolStorageAdapter, _>),
        )
        .route(
            paths::wallet::SIGN_TX_ZK,
            routing::post(wallet::sign_tx_zk::<WalletService, MempoolStorageAdapter, _>),
        );

    let router = router
        .with_state(handle)
        .layer(axum::extract::DefaultBodyLimit::max(settings.max_body_size))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            settings.timeout,
        ))
        .layer(RequestBodyLimitLayer::new(settings.max_body_size))
        .layer(ConcurrencyLimitLayer::new(settings.max_concurrent_requests))
        .layer(
            TraceLayer::new_for_http()
                .on_request(DefaultOnRequest::new().level(TracingLevel::TRACE))
                .on_response(DefaultOnResponse::new().level(TracingLevel::TRACE)),
        )
        .layer(build_cors_layer(&settings.cors_origins)?);

    Ok(router)
}

fn build_cors_layer(origins: &[String]) -> Result<CorsLayer, io::Error> {
    let mut layer = CorsLayer::new();

    if origins.is_empty() {
        layer = layer.allow_origin(Any);
    }

    for origin in origins {
        let header = origin.as_str().parse::<HeaderValue>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid admin CORS origin `{origin}`: {error}"),
            )
        })?;

        layer = layer.allow_origin(header);
    }

    Ok(layer
        .allow_headers(vec![CONTENT_TYPE, USER_AGENT])
        .allow_methods(Any))
}

async fn bind_listener(address: SocketAddr) -> Result<TcpListener, io::Error> {
    TcpListener::bind(address).await.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Failed to bind to address {address}: {error}"),
        )
    })
}

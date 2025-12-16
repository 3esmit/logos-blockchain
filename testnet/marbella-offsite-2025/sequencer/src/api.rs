use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::sequencer::{Sequencer, TransferRequest};

pub type AppState = Arc<Sequencer>;

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BalanceResponse {
    pub account: String,
    pub balance: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountEntry {
    pub account: String,
    pub balance: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountsResponse {
    pub accounts: Vec<AccountEntry>,
}

/// POST /transfer
/// Request body: { "from": "alice", "to": "bob", "amount": 100 }
async fn transfer(
    State(sequencer): State<AppState>,
    Json(request): Json<TransferRequest>,
) -> impl IntoResponse {
    info!(
        "Received transfer request: {} -> {} ({})",
        request.from, request.to, request.amount
    );

    match sequencer.process_transfer(request).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!("Transfer failed: {e}");
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// GET /balance/{account}
async fn get_balance(
    State(sequencer): State<AppState>,
    axum::extract::Path(account): axum::extract::Path<String>,
) -> impl IntoResponse {
    info!("Received balance request for: {}", account);

    match sequencer.get_balance(&account).await {
        Ok(balance) => (
            StatusCode::OK,
            Json(BalanceResponse { account, balance }),
        )
            .into_response(),
        Err(e) => {
            error!("Get balance failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// GET /accounts
/// Returns all accounts and their balances
async fn list_accounts(State(sequencer): State<AppState>) -> impl IntoResponse {
    info!("Received list accounts request");

    match sequencer.list_accounts().await {
        Ok(accounts) => {
            let accounts = accounts
                .into_iter()
                .map(|(account, balance)| AccountEntry { account, balance })
                .collect();
            (StatusCode::OK, Json(AccountsResponse { accounts })).into_response()
        }
        Err(e) => {
            error!("List accounts failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// GET /health
/// Health check endpoint
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

pub fn create_router(sequencer: Arc<Sequencer>) -> axum::Router {
    axum::Router::new()
        .route("/transfer", post(transfer))
        .route("/balance/:account", get(get_balance))
        .route("/accounts", get(list_accounts))
        .route("/health", get(health))
        .with_state(sequencer)
}

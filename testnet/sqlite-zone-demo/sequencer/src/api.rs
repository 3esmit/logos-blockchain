use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use demo_sqlite_sequencer::db::Dish;
use reqwest::{Method, header};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error};

use demo_sqlite_sequencer::db::Menu;

use crate::sequencer::Sequencer;

#[derive(Clone)]
pub struct AppState {
    pub menu: Arc<Menu>,
    pub sequencer: Arc<Sequencer>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Insert request struct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertRequest {
    pub name: String,
    pub data: String,
}

/// Response after successful insert
/*#[derive(Debug, Serialize, Deserialize)]
pub struct InsertResponse {
    pub dish: Dish,
}*/

/*fn friendly_error(err: &Error) -> String {
    match err {
        SequencerError::Timeout => "Query timed out waiting for confirmation".to_owned(),
        _ => "Internal server error".to_owned(),
    }
}*/

#[derive(Debug, Serialize, Deserialize)]
pub struct SelectResponse {
    #[serde(default)]
    pub dishes: Vec<Dish>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SelectQuery {
    #[serde(default)]
    pub query: String,
}

/// POST /insert
/// Request body: { "name": "Steak", "data": "A medium-rare ribsteak" }
#[axum::debug_handler]
async fn insert(
    State(appstate): State<AppState>,
    Json(request): Json<InsertRequest>,
) -> impl IntoResponse {
    debug!(
        "API /insert {}: {}",
        request.name, request.data
    );

    match appstate.menu.insert(&request.name, &request.data).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!("Insert failed: {e}");
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

/// GET /select
/// Request body: { "query": "SELECT * FROM menu" }
#[axum::debug_handler]
async fn get_select(
    State(appstate): State<AppState>,
    Json(request): Json<SelectQuery>,
) -> impl IntoResponse {
    debug!("API /select {}", request.query);

    match appstate.menu.select(request.query).await {
        Ok(dishes) => (StatusCode::OK, Json(SelectResponse{dishes})).into_response(),
        Err(e) => {
            error!("Select failed: {e}");
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

/// GET /health
/// Health check endpoint
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

pub fn create_router(menu: Arc<Menu>, sequencer: Arc<Sequencer>) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let state = AppState{menu, sequencer};

    axum::Router::new()
        .route("/insert", post(insert))
        .route("/select", get(get_select))
        .route("/health", get(health))
        .with_state(state)
        .layer(cors)
}

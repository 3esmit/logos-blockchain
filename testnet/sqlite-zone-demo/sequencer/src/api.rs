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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SqlResponse {
    #[serde(default)]
    pub dishes: Vec<Dish>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQuery {
    #[serde(default)]
    pub query: String,
}

/// POST /query
/// Request body: { "query": "SELECT * FROM menu" }
#[axum::debug_handler]
async fn query(
    State(appstate): State<AppState>,
    Json(request): Json<SqlQuery>,
) -> impl IntoResponse {
    debug!("API /query {}", request.query);

    match appstate.menu.query(request.query).await {
        Ok(Some(dishes)) => (StatusCode::OK, Json(SqlResponse{dishes})).into_response(),
        Ok(None) => (StatusCode::OK).into_response(),
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

pub fn create_router(menu: Arc<Menu>) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let state = AppState{menu};

    axum::Router::new()
        .route("/query", post(query))
        .route("/health", get(health))
        .with_state(state)
        .layer(cors)
}

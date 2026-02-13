use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use crate::db::MenuReadOnly;
use demo_sqlite_sequencer::db::Dish;

use reqwest::{Method, header};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<MenuReadOnly>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQuery {
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SqlResponse {
    #[serde(default)]
    pub dishes: Vec<Dish>,
}

#[axum::debug_handler]
async fn query(
    State(state): State<AppState>,
    Json(request): Json<SqlQuery>,
) -> impl IntoResponse {
    debug!("API /query {}", request.query);

    let trimmed = request.query.trim();
    if trimmed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "empty query"})),
        )
            .into_response();
    }

    if !trimmed
        .split_whitespace()
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case("SELECT"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "only SELECT queries are allowed"})),
        )
            .into_response();
    }
    
    match state.db.lock().await.query(request.query).await {
        Ok(dishes) => (StatusCode::OK, Json(SqlResponse{dishes})).into_response(),
        Err(e) => {
            error!("Query failed: {e}");
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

pub fn create_router(db: Arc<Mutex<MenuReadOnly>>) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let state = AppState { db };

    axum::Router::new()
        .route("/query", get(query))
        .route("/health", get(health))
        .with_state(state)
        .layer(cors)
}

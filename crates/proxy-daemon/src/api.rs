use axum::{
    extract::Path,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use proxy_core::pool::ProxyPool;
use proxy_core::router::Router as ProxyRouter;
use serde::Serialize;
use std::sync::Arc;

pub fn build_router(pool: Arc<ProxyPool>, _router: Arc<ProxyRouter>) -> Router {
    let state = AppState { pool };

    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/proxies", get(list_proxies))
        .route("/api/v1/proxies/{id}/switch", post(switch_proxy))
        .route("/api/v1/rotate", post(rotate_proxy))
        .with_state(state)
}

#[derive(Clone)]
struct AppState {
    pool: Arc<ProxyPool>,
}

#[derive(Serialize)]
struct StatusResponse {
    active_proxy: Option<proxy_core::proxy::ProxyInfo>,
    pool_size: usize,
}

async fn get_status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<StatusResponse>, StatusCode> {
    let active = state.pool.active().await;
    let all = state.pool.all().await;
    Ok(Json(StatusResponse {
        active_proxy: active,
        pool_size: all.len(),
    }))
}

async fn list_proxies(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<Vec<proxy_core::proxy::ProxyInfo>>, StatusCode> {
    let proxies = state.pool.all().await;
    Ok(Json(proxies))
}

async fn switch_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<proxy_core::proxy::ProxyInfo>, StatusCode> {
    state
        .pool
        .set_active(&id)
        .await
        .ok_or(StatusCode::NOT_FOUND)
        .map(Json)
}

async fn rotate_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<proxy_core::proxy::ProxyInfo>, StatusCode> {
    state
        .pool
        .rotate()
        .await
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
        .map(Json)
}

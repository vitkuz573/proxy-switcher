use axum::{
    extract::{Path, Request},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use proxy_core::pool::ProxyPool;
use proxy_core::router::Router as ProxyRouter;
use serde::Serialize;
use std::sync::Arc;

pub fn build_router(pool: Arc<ProxyPool>, router: Arc<ProxyRouter>) -> Router {
    let state = AppState { pool, router };

    let api = Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/proxies", get(list_proxies))
        .route("/api/v1/proxies/{id}/switch", post(switch_proxy))
        .route("/api/v1/rotate", post(rotate_proxy))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/dns", get(get_dns))
        .with_state(state);

    Router::new()
        .nest("/", api)
        .fallback(ui_handler)
}

async fn ui_handler(req: Request) -> Response {
    let ui_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let path = req.uri().path().trim_start_matches('/');
    let file_path = if path.is_empty() {
        ui_path.join("index.html")
    } else {
        ui_path.join(path)
    };

    if file_path.starts_with(&ui_path) && file_path.is_file() {
        let data = match tokio::fs::read(&file_path).await {
            Ok(d) => d,
            Err(_) => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
        };
        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let content_type = match ext {
            "css" => "text/css",
            "js" | "mjs" => "application/javascript",
            "html" => "text/html",
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "ico" => "image/x-icon",
            "json" => "application/json",
            _ => "application/octet-stream",
        };
        return Response::builder()
            .header("Content-Type", content_type)
            .body(axum::body::Body::from(data))
            .unwrap();
    }

    // SPA fallback: always serve index.html
    match tokio::fs::read_to_string(ui_path.join("index.html")).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

#[derive(Clone)]
struct AppState {
    pool: Arc<ProxyPool>,
    router: Arc<ProxyRouter>,
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

#[derive(Serialize)]
struct StatsResponse {
    tcp_connections: usize,
    udp_flows: usize,
    dns_cache_size: usize,
    healthy_count: usize,
}

async fn get_stats(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<StatsResponse> {
    let tcp = state.router.active_tcp_conns().await;
    let udp = state.router.active_udp_flows().await;
    let dns = state.router.dns_cache_entries().await.len();
    let healthy = state.pool.healthy_count().await;

    Json(StatsResponse {
        tcp_connections: tcp,
        udp_flows: udp,
        dns_cache_size: dns,
        healthy_count: healthy,
    })
}

#[derive(Serialize)]
struct DnsEntry {
    ip: String,
    hostname: String,
}

async fn get_dns(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<Vec<DnsEntry>> {
    let entries = state.router.dns_cache_entries().await;
    Json(
        entries
            .into_iter()
            .map(|(ip, hostname)| DnsEntry {
                ip: ip.to_string(),
                hostname,
            })
            .collect(),
    )
}

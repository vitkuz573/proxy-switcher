use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use tracing::{error, info};
use chrono::{DateTime, Utc};
use proxy_core::health::HealthChecker;
use proxy_core::pool::ProxyPool;
use proxy_core::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use proxy_core::router::Router as ProxyRouter;
use proxy_core::scraper::Scraper;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ScrapeState {
    pub running: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub proxies_found: usize,
    pub healthy_count: usize,
    pub checking_progress: Option<(usize, usize)>, // (checked, total)
    pub errors: Vec<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<ProxyPool>,
    pub router: Arc<ProxyRouter>,
    pub scraper: Arc<Scraper>,
    pub health: Arc<HealthChecker>,
    pub scrape_state: Arc<RwLock<ScrapeState>>,
    pub sources: Arc<RwLock<Vec<String>>>,
    pub ui_dir: std::path::PathBuf,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/proxies", get(list_proxies).post(add_proxy))
        .route("/api/v1/proxies/{id}", delete(delete_proxy))
        .route("/api/v1/switch", post(switch_proxy))
        .route("/api/v1/rotate", post(rotate_proxy))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/dns", get(get_dns))
        .route("/api/v1/scrape", post(trigger_scrape))
        .route("/api/v1/scrape/status", get(scrape_status))
        .route("/api/v1/sources", get(list_sources).post(add_source))
        .route("/api/v1/sources/{id}", delete(delete_source))
        .fallback(ui_handler)
        .with_state(state)
}

async fn ui_handler(State(state): State<AppState>, req: Request) -> Response {
    let ui_path = &state.ui_dir;
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

    match tokio::fs::read_to_string(ui_path.join("index.html")).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

// ── Status ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    active_proxy: Option<ProxyInfo>,
    pool_size: usize,
}

async fn get_status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<StatusResponse> {
    let active = state.pool.active().await;
    let all = state.pool.all().await;
    Json(StatusResponse {
        active_proxy: active,
        pool_size: all.len(),
    })
}

// ── Proxies CRUD ──────────────────────────────────────────────────────

async fn list_proxies(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<Vec<ProxyInfo>> {
    Json(state.pool.all().await)
}

#[derive(Deserialize)]
struct AddProxyInput {
    host: String,
    port: u16,
    protocol: Option<String>,
    country: Option<String>,
}

async fn add_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(input): axum::extract::Json<AddProxyInput>,
) -> StatusCode {
    let protocol = match input.protocol.as_deref() {
        Some("socks5") => ProxyProtocol::Socks5,
        Some("socks4") => ProxyProtocol::Socks4,
        Some("https") => ProxyProtocol::Https,
        _ => ProxyProtocol::Http,
    };

    let proxy = ProxyInfo {
        id: format!("{}:{}", input.host, input.port),
        host: input.host,
        port: input.port,
        protocol,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: input.country,
        last_checked: None,
        score: 0.0,
    };

    state.pool.add(proxy).await;
    StatusCode::CREATED
}

async fn delete_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(id): Path<String>,
) -> StatusCode {
    if state.pool.remove(&id).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
struct SwitchQuery {
    id: String,
}

async fn switch_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
    Query(q): Query<SwitchQuery>,
) -> Result<Json<ProxyInfo>, StatusCode> {
    state
        .pool
        .set_active(&q.id)
        .await
        .ok_or(StatusCode::NOT_FOUND)
        .map(Json)
}

async fn rotate_proxy(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<ProxyInfo>, StatusCode> {
    state
        .pool
        .rotate()
        .await
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
        .map(Json)
}

// ── Stats ─────────────────────────────────────────────────────────────

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

// ── DNS ───────────────────────────────────────────────────────────────

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

// ── Scrape ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ScrapeStatusResponse {
    running: bool,
    last_run: Option<DateTime<Utc>>,
    proxies_found: usize,
    healthy_count: usize,
    checking_progress: Option<(usize, usize)>,
    errors: Vec<String>,
}

async fn trigger_scrape(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> StatusCode {
    let running = state.scrape_state.read().await.running;
    if running {
        return StatusCode::CONFLICT;
    }

    let pool = state.pool.clone();
    let scraper = state.scraper.clone();
    let health = state.health.clone();
    let scrape_state = state.scrape_state.clone();

    tokio::spawn(async move {
        let mut s = scrape_state.write().await;
        s.running = true;
        s.errors.clear();
        drop(s);

        match scraper.scrape_all().await {
            Ok(p) => {
                let count = p.len();
                info!("Scrape: collected {count} proxies, running health check...");

                let results = health.check_batch(&p).await;
                let healthy_count = results.iter().filter(|r| r.alive).count();
                pool.apply_health_results(results).await;

                let mut s = scrape_state.write().await;
                s.running = false;
                s.last_run = Some(Utc::now());
                s.proxies_found = count;
                s.healthy_count = healthy_count;
            }
            Err(e) => {
                let msg = format!("{e}");
                error!("Scrape failed: {msg}");
                let mut s = scrape_state.write().await;
                s.running = false;
                s.errors.push(msg);
            }
        };
    });

    StatusCode::ACCEPTED
}

async fn scrape_status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<ScrapeStatusResponse> {
    let s = state.scrape_state.read().await;
    Json(ScrapeStatusResponse {
        running: s.running,
        last_run: s.last_run,
        proxies_found: s.proxies_found,
        healthy_count: s.healthy_count,
        checking_progress: s.checking_progress,
        errors: s.errors.clone(),
    })
}

// ── Sources CRUD ───────────────────────────────────────────────────────

async fn list_sources(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<Vec<String>> {
    Json(state.sources.read().await.clone())
}

#[derive(Deserialize)]
struct AddSourceInput {
    url: String,
}

async fn add_source(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(input): axum::extract::Json<AddSourceInput>,
) -> StatusCode {
    if input.url.trim().is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    let mut sources = state.sources.write().await;
    if sources.contains(&input.url) {
        return StatusCode::CONFLICT;
    }
    sources.push(input.url.trim().to_string());
    StatusCode::CREATED
}

async fn delete_source(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(url): Path<String>,
) -> StatusCode {
    let mut sources = state.sources.write().await;
    if let Some(pos) = sources.iter().position(|s| s == &url) {
        sources.remove(pos);
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

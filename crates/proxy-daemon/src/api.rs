use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use proxy_core::health::HealthChecker;
use proxy_core::pool::{EvictionPolicy, ProxyPool};
use proxy_core::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use proxy_core::router::Router as ProxyRouter;
use proxy_core::scraper::Scraper;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

/// Stable identifier for a scrape source.
///
/// The URL is data: it contains slashes and can change. Using it as a path
/// parameter both breaks routing and makes identity mutable.
fn source_id(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    format!("src_{}", hex(&digest[..8]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct ScrapeState {
    pub running: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub proxies_found: usize,
    pub healthy_count: usize,
    /// True while a health-check batch is in flight.
    pub check_running: bool,
    /// Size of the batch currently being checked.
    pub check_total: usize,
    /// Live counter of finished probes, updated by the batch itself.
    pub checked: Arc<AtomicUsize>,
    pub errors: Vec<String>,
}

impl ScrapeState {
    pub fn new(checked: Arc<AtomicUsize>) -> Self {
        Self {
            running: false,
            last_run: None,
            proxies_found: 0,
            healthy_count: 0,
            check_running: false,
            check_total: 0,
            checked,
            errors: Vec::new(),
        }
    }

    /// Live (checked, total) progress, or None when no batch is running.
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.check_running {
            Some((self.checked.load(Ordering::Relaxed), self.check_total))
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<ProxyPool>,
    pub router: Arc<ProxyRouter>,
    pub scraper: Arc<Scraper>,
    pub health: Arc<HealthChecker>,
    pub scrape_state: Arc<RwLock<ScrapeState>>,
    pub sources: Arc<RwLock<Vec<String>>>,
    /// Live health-check progress counter, shared with the check loops.
    pub check_progress: Arc<AtomicUsize>,
    /// Guards against overlapping full-pool health-check batches.
    pub check_in_flight: Arc<std::sync::atomic::AtomicBool>,
    /// When a proxy may be evicted, shared by every path that applies results.
    pub eviction_policy: EvictionPolicy,
}

/* ── Wire types ────────────────────────────────────────────────────────── */

#[derive(Serialize)]
struct Source {
    id: String,
    url: String,
}

#[derive(Serialize)]
struct Summary {
    pool_size: usize,
    healthy_count: usize,
    active_proxy: Option<ProxyInfo>,
}

#[derive(Serialize)]
struct ScrapeRun {
    id: String,
    state: &'static str,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    proxies_found: usize,
    healthy_count: usize,
    health_check: Option<HealthCheckProgress>,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct HealthCheckProgress {
    checked: usize,
    total: usize,
}

#[derive(Deserialize)]
struct PutActiveProxy {
    proxy_id: String,
}

#[derive(Deserialize)]
struct AddProxyInput {
    host: String,
    port: u16,
    protocol: Option<String>,
    country: Option<String>,
}

#[derive(Deserialize)]
struct AddSourceInput {
    url: String,
}

#[derive(Deserialize, Default)]
struct ListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    sort: Option<String>,
    search: Option<String>,
}

/// RFC 9457 problem document.
///
/// A dedicated error type rather than a bare `Response`: `Result<T, Response>`
/// is a large error variant that clippy (rightly) rejects, and handlers read
/// better returning `Result<T, ApiError>`.
pub struct ApiError {
    status: StatusCode,
    detail: String,
}

impl ApiError {
    fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }

    fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, detail)
    }

    fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let title = match self.status {
            StatusCode::NOT_FOUND => "Not found",
            StatusCode::CONFLICT => "Conflict",
            StatusCode::BAD_REQUEST => "Bad request",
            _ => "Error",
        };

        (
            self.status,
            Json(serde_json::json!({
                "type": format!("https://proxy-switcher.dev/problems/{}", self.status.as_u16()),
                "title": title,
                "status": self.status.as_u16(),
                "detail": self.detail,
            })),
        )
            .into_response()
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/summary", get(get_summary))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/dns", get(get_dns))
        // Proxies
        .route("/api/v1/proxies", get(list_proxies).post(create_proxy))
        .route("/api/v1/proxies/{id}", get(get_proxy).delete(delete_proxy))
        // Active proxy singleton. Deliberately not nested under /proxies, so
        // the literal path can never be shadowed by /proxies/{id}.
        .route(
            "/api/v1/active-proxy",
            get(get_active).put(put_active).delete(clear_active),
        )
        // Nested under the singleton rather than using a `:verb` segment: in
        // the router a colon introduces a path parameter, so `active-proxy:rotate`
        // would silently match any suffix instead of one exact path.
        .route("/api/v1/active-proxy/rotate", post(rotate))
        // Sources
        .route("/api/v1/sources", get(list_sources).post(create_source))
        .route("/api/v1/sources/{id}", delete(delete_source))
        // Scrape runs
        .route("/api/v1/scrape-runs", post(create_scrape_run))
        .route("/api/v1/scrape-runs/current", get(get_scrape_run))
        // No fallback: this is an internal API, not a web server. A catch-all
        // turned every mismatched path into a 200 with a web page.
        .with_state(state)
}

/* ── Summary and telemetry ─────────────────────────────────────────────── */

async fn get_summary(State(state): State<AppState>) -> Json<Summary> {
    Json(Summary {
        pool_size: state.pool.len().await,
        healthy_count: state.pool.healthy_count().await,
        active_proxy: state.pool.active().await,
    })
}

async fn get_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "tcp_connections": 0,
        "udp_flows": 0,
        "dns_cache_size": state.router.dns_cache_entries().await.len(),
        "healthy_count": state.pool.healthy_count().await,
        "pool_size": state.pool.len().await,
    }))
}

#[derive(Serialize)]
struct DnsEntry {
    ip: String,
    hostname: String,
}

async fn get_dns(State(state): State<AppState>) -> Json<Vec<DnsEntry>> {
    Json(
        state
            .router
            .dns_cache_entries()
            .await
            .into_iter()
            .map(|(ip, hostname)| DnsEntry {
                ip: ip.to_string(),
                hostname,
            })
            .collect(),
    )
}

/* ── Proxies ───────────────────────────────────────────────────────────── */

async fn list_proxies(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<Vec<ProxyInfo>> {
    let mut proxies = state.pool.all().await;

    if let Some(needle) = q.search.as_ref().filter(|s| !s.is_empty()) {
        let needle = needle.to_lowercase();
        proxies.retain(|p| {
            p.id.to_lowercase().contains(&needle)
                || (p
                    .country
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&needle))
        });
    }

    match q.sort.as_deref() {
        Some("latency") => proxies.sort_by(|a, b| {
            a.latency_ms
                .unwrap_or(u64::MAX)
                .cmp(&b.latency_ms.unwrap_or(u64::MAX))
        }),
        Some("host") => proxies.sort_by(|a, b| a.id.cmp(&b.id)),
        _ => proxies.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(usize::MAX).min(1000);
    Json(proxies.into_iter().skip(offset).take(limit).collect())
}

async fn create_proxy(State(state): State<AppState>, Json(input): Json<AddProxyInput>) -> Response {
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
        last_success: None,
        first_seen: chrono::Utc::now(),
        consecutive_failures: 0,
        score: 0.0,
    };

    let id = proxy.id.clone();
    state.pool.add(proxy.clone()).await;

    // Returning the record keeps the canonical id in one place: callers never
    // reconstruct `host:port` to reference what they just created.
    let location = format!("/api/v1/proxies/{id}");
    (
        StatusCode::CREATED,
        [(axum::http::header::LOCATION, location)],
        Json(proxy),
    )
        .into_response()
}

async fn get_proxy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProxyInfo>, ApiError> {
    state
        .pool
        .all()
        .await
        .into_iter()
        .find(|p| p.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("No proxy with id {id}")))
}

async fn delete_proxy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.pool.remove(&id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("No proxy with id {id}")))
    }
}

/* ── Active proxy ──────────────────────────────────────────────────────── */

async fn get_active(State(state): State<AppState>) -> Result<Json<Summary>, ApiError> {
    // Resolve the selection before reading the counters: a 404 here means
    // "nothing selected", which is a different state from "no proxies".
    let active = state
        .pool
        .active()
        .await
        .ok_or_else(|| ApiError::not_found("No proxy is currently selected"))?;

    Ok(Json(Summary {
        pool_size: state.pool.len().await,
        healthy_count: state.pool.healthy_count().await,
        active_proxy: Some(active),
    }))
}

async fn put_active(
    State(state): State<AppState>,
    Query(q): Query<PutActiveProxy>,
) -> Result<Json<ProxyInfo>, ApiError> {
    state
        .pool
        .set_active(&q.proxy_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("No proxy with id {}", q.proxy_id)))
}

async fn clear_active(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.pool.clear_active().await;
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate(State(state): State<AppState>) -> Result<Json<ProxyInfo>, ApiError> {
    state
        .pool
        .rotate()
        .await
        .map(Json)
        .ok_or_else(|| ApiError::conflict("The pool is empty"))
}

/* ── Sources ───────────────────────────────────────────────────────────── */

async fn list_sources(State(state): State<AppState>) -> Json<Vec<Source>> {
    Json(
        state
            .sources
            .read()
            .await
            .iter()
            .map(|url| Source {
                id: source_id(url),
                url: url.clone(),
            })
            .collect(),
    )
}

async fn create_source(
    State(state): State<AppState>,
    Json(input): Json<AddSourceInput>,
) -> Response {
    let url = input.url.trim().to_string();
    if url.is_empty() {
        return ApiError::bad_request("url must not be empty").into_response();
    }

    let id = source_id(&url);
    let mut sources = state.sources.write().await;

    if sources.iter().any(|u| source_id(u) == id) {
        return ApiError::conflict("That source is already configured").into_response();
    }
    sources.push(url.clone());

    drop(sources);
    info!(%id, "scrape source added");

    let location = format!("/api/v1/sources/{id}");
    (
        StatusCode::CREATED,
        [(axum::http::header::LOCATION, location)],
        Json(Source { id, url }),
    )
        .into_response()
}

async fn delete_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut sources = state.sources.write().await;
    let before = sources.len();
    sources.retain(|u| source_id(u) != id);

    if sources.len() == before {
        return Err(ApiError::not_found(format!("No source with id {id}")));
    }

    info!(%id, "scrape source removed");
    Ok(StatusCode::NO_CONTENT)
}

/* ── Scrape runs ───────────────────────────────────────────────────────── */

async fn create_scrape_run(State(state): State<AppState>) -> Response {
    let running = state.scrape_state.read().await.running;
    if running {
        return ApiError::conflict("A scrape cycle is already running").into_response();
    }

    let pool = state.pool.clone();
    let scraper = state.scraper.clone();
    let health = state.health.clone();
    let scrape_state = state.scrape_state.clone();
    let check_progress = state.check_progress.clone();
    let check_in_flight = state.check_in_flight.clone();
    let policy = state.eviction_policy;

    tokio::spawn(async move {
        let mut s = scrape_state.write().await;
        s.running = true;
        s.errors.clear();
        drop(s);

        let discovered = match scraper.scrape_all().await {
            Ok(p) => p,
            Err(e) => {
                error!(%e, "scrape failed");
                let mut s = scrape_state.write().await;
                s.running = false;
                s.errors.push(format!("{e}"));
                return;
            }
        };

        let count = discovered.len();
        pool.merge(discovered).await;

        let pending = pool.unverified().await;
        {
            let mut s = scrape_state.write().await;
            s.running = false;
            s.last_run = Some(Utc::now());
            s.proxies_found = pool.len().await;
        }

        if pending.is_empty()
            || check_in_flight
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_err()
        {
            let healthy = pool.healthy_count().await;
            let mut s = scrape_state.write().await;
            s.healthy_count = healthy;
            return;
        }

        {
            let mut s = scrape_state.write().await;
            s.check_running = true;
            s.check_total = pending.len();
        }

        let evicted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = evicted.clone();
        let apply_pool = pool.clone();

        let counts = health
            .check_batch(&pending, check_progress, move |result| {
                let pool = apply_pool.clone();
                let counter = counter.clone();
                async move {
                    if pool.apply_health_result(&result, policy).await {
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
            .await;
        check_in_flight.store(false, std::sync::atomic::Ordering::SeqCst);

        // Resolve before the macro: an `.await` inside tracing arguments would
        // keep its temporary alive across the await and make the future !Send.
        let healthy = pool.healthy_count().await;

        info!(
            found = count,
            working = counts.working,
            failed = counts.failed,
            inconclusive = counts.inconclusive,
            evicted = evicted.load(std::sync::atomic::Ordering::Relaxed),
            healthy,
            "scrape run complete"
        );

        let mut s = scrape_state.write().await;
        s.check_running = false;
        s.healthy_count = healthy;
    });

    let location = "/api/v1/scrape-runs/current";
    (
        StatusCode::CREATED,
        [(axum::http::header::LOCATION, location.to_string())],
    )
        .into_response()
}

async fn get_scrape_run(State(state): State<AppState>) -> Json<ScrapeRun> {
    let s = state.scrape_state.read().await;

    let (state_name, progress): (&'static str, Option<(usize, usize)>) = if s.running {
        ("running", None)
    } else if s.check_running {
        ("checking", s.progress())
    } else {
        ("idle", None)
    };

    Json(ScrapeRun {
        id: "run_current".to_string(),
        state: state_name,
        started_at: s.last_run,
        finished_at: s.last_run,
        proxies_found: s.proxies_found,
        healthy_count: s.healthy_count,
        health_check: progress.map(|(checked, total)| HealthCheckProgress { checked, total }),
        errors: s.errors.clone(),
    })
}

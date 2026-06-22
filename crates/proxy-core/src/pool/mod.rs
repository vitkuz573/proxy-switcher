use crate::health::HealthResult;
use crate::proxy::ProxyInfo;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct ProxyPool {
    inner: Arc<RwLock<PoolInner>>,
}

#[derive(Default)]
struct PoolInner {
    proxies: Vec<ProxyInfo>,
    active_index: Option<usize>,
}

impl ProxyPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn update(&self, proxies: Vec<ProxyInfo>) {
        let mut inner = self.inner.write().await;
        inner.proxies = proxies;
        tracing::info!("Pool updated with {} proxies", inner.proxies.len());
    }

    /// Replace pool with only healthy proxies, scored by latency
    pub async fn apply_health_results(&self, results: Vec<HealthResult>) {
        let mut healthy = Vec::new();

        for r in &results {
            if !r.alive {
                continue;
            }

            let score = if r.latency_ms == 0 {
                0.0
            } else {
                // Score 100 at 100ms, drops to ~50 at 1s, ~10 at 5s
                (1000.0 / r.latency_ms as f64).min(100.0)
            };

            healthy.push(ProxyInfo {
                id: r.proxy_id.clone(),
                host: String::new(), // will be filled from storage
                port: 0,
                protocol: crate::proxy::ProxyProtocol::Http,
                anonymity: crate::proxy::Anonymity::Unknown,
                latency_ms: Some(r.latency_ms),
                country: None,
                last_checked: Some(chrono::Utc::now()),
                score,
            });
        }

        // Sort by score descending
        healthy.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        let mut inner = self.inner.write().await;

        // Merge health data into existing proxy records
        for h in &healthy {
            if let Some(existing) = inner.proxies.iter_mut().find(|p| p.id == h.id) {
                existing.latency_ms = h.latency_ms;
                existing.score = h.score;
                existing.last_checked = h.last_checked;
            } else {
                inner.proxies.push(h.clone());
            }
        }

        // Remove dead proxies
        let dead_ids: Vec<String> = results.iter().filter(|r| !r.alive).map(|r| r.proxy_id.clone()).collect();
        inner.proxies.retain(|p| !dead_ids.contains(&p.id));

        // Adjust active_index if needed
        if let Some(idx) = inner.active_index {
            if idx >= inner.proxies.len() {
                inner.active_index = if inner.proxies.is_empty() { None } else { Some(0) };
            }
        }

        tracing::info!(
            "Health check: {}/{} alive, pool has {} proxies",
            healthy.len(),
            results.len(),
            inner.proxies.len()
        );
    }

    pub async fn set_active(&self, id: &str) -> Option<ProxyInfo> {
        let inner = &mut *self.inner.write().await;
        let idx = inner.proxies.iter().position(|p| p.id == id)?;
        inner.active_index = Some(idx);
        Some(inner.proxies[idx].clone())
    }

    pub async fn active(&self) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        inner.active_index.and_then(|i| inner.proxies.get(i).cloned())
    }

    pub async fn all(&self) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner.proxies.clone()
    }

    pub async fn healthy_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner.proxies.iter().filter(|p| p.score > 0.0).count()
    }

    pub async fn rotate(&self) -> Option<ProxyInfo> {
        let inner = &mut *self.inner.write().await;
        if inner.proxies.is_empty() {
            return None;
        }
        let next = inner.active_index.map_or(0, |i| (i + 1) % inner.proxies.len());
        inner.active_index = Some(next);
        Some(inner.proxies[next].clone())
    }
}

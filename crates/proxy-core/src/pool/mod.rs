use crate::health::{HealthResult, Verdict};
use crate::proxy::ProxyInfo;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// How many consecutive failed probes a proxy survives before eviction, and
/// how long a proxy that has never worked is kept around while unverified.
#[derive(Debug, Clone, Copy)]
pub struct EvictionPolicy {
    pub max_consecutive_failures: u32,
    pub unverified_ttl_secs: u64,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self {
            // A public proxy blips constantly. Evicting on the first failed
            // probe discarded roughly half of a working pool every minute.
            max_consecutive_failures: 3,
            unverified_ttl_secs: 3600,
        }
    }
}

#[derive(Default)]
pub struct ProxyPool {
    inner: Arc<RwLock<PoolInner>>,
}

#[derive(Default)]
struct PoolInner {
    proxies: Vec<ProxyInfo>,
    index: HashMap<String, usize>,
    active_index: Option<usize>,
}

/// Why a batch removed proxies, for logging. Pool contents change constantly
/// and an unexplained drop is impossible to diagnose.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchOutcome {
    pub checked: usize,
    pub working: usize,
    pub failed: usize,
    /// Probes that timed out. Recorded for visibility, but they do not change
    /// any proxy's state.
    pub inconclusive: usize,
    pub evicted: usize,
}

impl ProxyPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold freshly scraped proxies into the pool.
    ///
    /// Replaces nothing: proxies already known keep their latency, score and
    /// failure streak. Only genuinely new ids are added. Replacing the whole
    /// pool on every scrape threw away everything the health checker had
    /// learned and made the healthy count swing back to zero each cycle.
    pub async fn merge(&self, discovered: Vec<ProxyInfo>) -> usize {
        let mut inner = self.inner.write().await;
        let mut added = 0usize;

        for proxy in discovered {
            match inner.index.get(&proxy.id) {
                Some(&i) => {
                    // Refresh descriptive fields a source may have corrected,
                    // but never the health state.
                    let existing = &mut inner.proxies[i];
                    existing.anonymity = proxy.anonymity;
                    if existing.country.is_none() {
                        existing.country = proxy.country;
                    }
                }
                None => {
                    let slot = inner.proxies.len();
                    inner.index.insert(proxy.id.clone(), slot);
                    inner.proxies.push(proxy.discovered());
                    added += 1;
                }
            }
        }

        tracing::info!(
            added,
            total = inner.proxies.len(),
            "pool merged from scrape"
        );
        added
    }

    /// Proxies that have never been proven to work. These are what a scrape
    /// adds, so this is the set worth probing after one.
    pub async fn unverified(&self) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.last_success.is_none())
            .cloned()
            .collect()
    }

    /// Proxies whose last probe succeeded.
    pub async fn working(&self) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.consecutive_failures == 0 && p.last_success.is_some())
            .cloned()
            .collect()
    }

    /// Fold probe results back into the pool.
    ///
    /// A failed probe increments a streak; the proxy is evicted only once it
    /// fails `max_consecutive_failures` times in a row, or when an unverified
    /// proxy has sat in the pool past its TTL without ever working. Transient
    /// network blips therefore cost a proxy nothing but one failed attempt.
    pub async fn apply_health_results(
        &self,
        results: Vec<HealthResult>,
        policy: EvictionPolicy,
    ) -> BatchOutcome {
        let now = chrono::Utc::now();
        let mut outcome = BatchOutcome {
            checked: results.len(),
            ..Default::default()
        };

        let mut failures: HashMap<&str, u32> = HashMap::new();
        let mut working: HashMap<&str, u64> = HashMap::new();
        let mut inconclusive: Vec<&str> = Vec::new();

        for r in &results {
            match r.verdict {
                Verdict::Working => {
                    outcome.working += 1;
                    working.insert(r.proxy_id.as_str(), r.latency_ms);
                }
                Verdict::Failed => {
                    outcome.failed += 1;
                    failures.insert(r.proxy_id.as_str(), 1);
                }
                Verdict::Inconclusive => {
                    outcome.inconclusive += 1;
                    inconclusive.push(r.proxy_id.as_str());
                }
            }
        }

        let mut inner = self.inner.write().await;

        // Success: refresh measurements and clear the streak.
        for (id, latency_ms) in &working {
            if let Some(&i) = inner.index.get(*id) {
                let proxy = &mut inner.proxies[i];
                proxy.latency_ms = Some(*latency_ms);
                proxy.score = score_for(*latency_ms);
                proxy.last_checked = Some(now);
                proxy.last_success = Some(now);
                proxy.consecutive_failures = 0;
            }
        }

        // Inconclusive: the probe proved nothing, so leave the streak, the
        // score and `last_success` exactly as they were.
        for id in inconclusive {
            if let Some(&i) = inner.index.get(id) {
                inner.proxies[i].last_checked = Some(now);
            }
        }

        // Failure: record it, evict only once the policy says so.
        let mut evict: HashSet<String> = HashSet::new();

        for id in failures.keys() {
            let Some(&i) = inner.index.get(*id) else {
                continue;
            };

            let proxy = &mut inner.proxies[i];
            proxy.last_checked = Some(now);
            proxy.consecutive_failures = proxy.consecutive_failures.saturating_add(1);

            // Decay rather than reset, so the ranking degrades gradually
            // instead of dropping out of contention on one bad probe.
            proxy.score *= 0.5;

            let never_worked = proxy.last_success.is_none();
            let unverified_age = now
                .signed_duration_since(
                    proxy
                        .last_checked
                        .unwrap_or(proxy.last_success.unwrap_or(now)),
                )
                .num_seconds()
                .unsigned_abs();

            let exhausted = proxy.consecutive_failures >= policy.max_consecutive_failures;
            let stale_unverified =
                never_worked && unverified_age >= policy.unverified_ttl_secs as i64 as u64;

            if exhausted || stale_unverified {
                evict.insert((*id).to_string());
            }
        }

        if !evict.is_empty() {
            outcome.evicted = evict.len();
            self.retain(&mut inner, |p| !evict.contains(&p.id));
        }

        tracing::info!(
            checked = outcome.checked,
            working = outcome.working,
            failed = outcome.failed,
            inconclusive = outcome.inconclusive,
            evicted = outcome.evicted,
            pool = inner.proxies.len(),
            "health check applied"
        );

        outcome
    }

    /// Drop proxies whose id is not in `keep`, rebuilding the id index and
    /// repairing the active selection in one pass.
    fn retain<F>(&self, inner: &mut PoolInner, predicate: F)
    where
        F: Fn(&ProxyInfo) -> bool,
    {
        // Resolve the active proxy by identity before filtering: after removal
        // its old index no longer refers to it, and the previous code silently
        // selected a different proxy or nothing at all.
        let active_id = inner
            .active_index
            .and_then(|i| inner.proxies.get(i).map(|p| p.id.clone()));

        let mut kept = Vec::with_capacity(inner.proxies.len());
        for proxy in std::mem::take(&mut inner.proxies) {
            if predicate(&proxy) {
                kept.push(proxy);
            }
        }

        let mut index = HashMap::with_capacity(kept.len());
        for (i, proxy) in kept.iter().enumerate() {
            index.insert(proxy.id.clone(), i);
        }

        inner.proxies = kept;
        inner.index = index;
        inner.active_index = active_id.and_then(|id| inner.index.get(&id).copied());
    }

    pub async fn set_active(&self, id: &str) -> Option<ProxyInfo> {
        let inner = &mut *self.inner.write().await;
        let idx = *inner.index.get(id)?;
        inner.active_index = Some(idx);
        Some(inner.proxies[idx].clone())
    }

    pub async fn active(&self) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .active_index
            .and_then(|i| inner.proxies.get(i).cloned())
    }

    pub async fn all(&self) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner.proxies.clone()
    }

    /// Proxies whose most recent probe succeeded. This is the number the
    /// dashboard reports as "healthy".
    pub async fn healthy_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.last_success.is_some() && p.consecutive_failures == 0)
            .count()
    }

    /// Highest scoring currently-working proxy, used for auto-rotation.
    pub async fn best(&self) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.last_success.is_some() && p.consecutive_failures == 0)
            .max_by(|a, b| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// Drop the current selection, leaving the pool untouched.
    pub async fn clear_active(&self) {
        self.inner.write().await.active_index = None;
    }

    pub async fn rotate(&self) -> Option<ProxyInfo> {
        let inner = &mut *self.inner.write().await;
        if inner.proxies.is_empty() {
            return None;
        }
        let next = inner
            .active_index
            .map_or(0, |i| (i + 1) % inner.proxies.len());
        inner.active_index = Some(next);
        Some(inner.proxies[next].clone())
    }

    pub async fn add(&self, proxy: ProxyInfo) {
        let mut inner = self.inner.write().await;
        match inner.index.get(&proxy.id) {
            Some(&i) => {
                // A manual add replaces the record outright: the operator is
                // asserting this endpoint, so stale health state must not block
                // it from being re-probed.
                inner.proxies[i] = proxy.discovered();
            }
            None => {
                let slot = inner.proxies.len();
                inner.index.insert(proxy.id.clone(), slot);
                inner.proxies.push(proxy.discovered());
            }
        }
    }

    pub async fn remove(&self, id: &str) -> bool {
        let mut inner = self.inner.write().await;
        if inner.index.contains_key(id) {
            self.retain(&mut inner, |p| p.id != id);
            true
        } else {
            false
        }
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.proxies.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// Score 100 at 100ms, ~50 at 1s, ~10 at 5s.
fn score_for(latency_ms: u64) -> f64 {
    if latency_ms == 0 {
        0.0
    } else {
        (1000.0 / latency_ms as f64).min(100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(id: &str) -> ProxyInfo {
        ProxyInfo {
            id: id.to_string(),
            host: "198.51.100.1".into(),
            port: 8080,
            protocol: crate::proxy::ProxyProtocol::Http,
            anonymity: crate::proxy::Anonymity::Unknown,
            latency_ms: None,
            country: None,
            last_checked: None,
            last_success: None,
            consecutive_failures: 0,
            score: 0.0,
        }
    }

    fn result(id: &str, alive: bool, latency: u64) -> HealthResult {
        if alive {
            HealthResult::working(id, latency)
        } else {
            HealthResult::failed(id, "refused")
        }
    }

    fn timeout(id: &str) -> HealthResult {
        HealthResult::inconclusive(id, "read timeout")
    }

    #[tokio::test]
    async fn a_single_failure_does_not_evict() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", true, 100), result("b", false, 0)], policy)
            .await;

        assert_eq!(pool.len().await, 2, "one failure must not remove a proxy");
        assert_eq!(pool.healthy_count().await, 1);
    }

    #[tokio::test]
    async fn repeated_failures_eventually_evict() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        for _ in 0..policy.max_consecutive_failures {
            pool.apply_health_results(vec![result("a", true, 100), result("b", false, 0)], policy)
                .await;
        }

        assert_eq!(pool.len().await, 1);
        assert_eq!(pool.all().await[0].id, "a");
    }

    #[tokio::test]
    async fn success_clears_the_failure_streak() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", false, 0)], policy)
            .await;
        pool.apply_health_results(vec![result("a", false, 0)], policy)
            .await;
        pool.apply_health_results(vec![result("a", true, 120)], policy)
            .await;
        pool.apply_health_results(vec![result("a", false, 0)], policy)
            .await;

        assert_eq!(pool.len().await, 1, "streak was reset by the success");
        assert_eq!(pool.healthy_count().await, 0, "last probe failed");
    }

    #[tokio::test]
    async fn merge_preserves_health_state_of_known_proxies() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", true, 90)], policy)
            .await;
        assert_eq!(pool.healthy_count().await, 1);

        // A later scrape sees the same proxy again.
        pool.merge(vec![proxy("a")]).await;

        assert_eq!(pool.len().await, 1);
        assert_eq!(
            pool.healthy_count().await,
            1,
            "a re-scrape must not reset known-good state"
        );
    }

    #[tokio::test]
    async fn inconclusive_probes_leave_state_untouched() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", true, 100)], policy)
            .await;
        assert_eq!(pool.healthy_count().await, 1);

        // "a" times out, "b" times out. Neither is evidence of a dead proxy.
        let outcome = pool
            .apply_health_results(vec![timeout("a"), timeout("b")], policy)
            .await;

        assert_eq!(outcome.inconclusive, 2);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.evicted, 0);
        assert_eq!(pool.len().await, 2, "timeouts must not evict");
        assert_eq!(
            pool.healthy_count().await,
            1,
            "a timeout must not demote a working proxy"
        );
    }

    #[tokio::test]
    async fn unverified_lists_only_unproven_proxies() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b"), proxy("c")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", true, 80)], policy)
            .await;

        let unverified = pool.unverified().await;
        assert_eq!(unverified.len(), 2);
        assert!(unverified.iter().all(|p| p.id != "a"));
    }

    #[tokio::test]
    async fn eviction_keeps_the_active_proxy_selected() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b"), proxy("c")]).await;
        pool.set_active("c").await;

        let policy = EvictionPolicy {
            max_consecutive_failures: 1,
            unverified_ttl_secs: 3600,
        };
        pool.apply_health_results(
            vec![
                result("a", false, 0),
                result("b", false, 0),
                result("c", true, 50),
            ],
            policy,
        )
        .await;

        let active = pool.active().await.expect("active proxy must survive");
        assert_eq!(active.id, "c", "eviction must not shift the selection");
    }

    #[tokio::test]
    async fn rotate_walks_working_proxies() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(vec![result("a", true, 100), result("b", true, 900)], policy)
            .await;

        assert_eq!(pool.best().await.map(|p| p.id), Some("a".to_string()));

        pool.set_active("a").await;
        assert_eq!(
            pool.rotate().await.map(|p| p.id),
            Some("b".to_string()),
            "rotate must advance to the next proxy"
        );
    }
}

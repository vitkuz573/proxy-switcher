pub mod statistics;

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
    /// A proxy confirmed working but slower than this is not selected.
    ///
    /// The score is derived from the measured latency, so the floor is a
    /// statement about how slow is too slow rather than an arbitrary number.
    pub min_score: f64,
    /// When each proxy should be looked at again.
    ///
    /// The pool needs it for the failures the traffic itself discovers: a
    /// proxy that could not carry a connection is looked at again after the
    /// same wait a failed probe would get.
    pub probe_schedule: ProbeSchedule,
    inner: Arc<RwLock<PoolInner>>,
}

#[derive(Default)]
struct PoolInner {
    proxies: Vec<ProxyInfo>,
    index: HashMap<String, usize>,
    active_index: Option<usize>,
    /// Proxies the eviction policy has condemned but not yet removed.
    ///
    /// Removal rebuilds the whole index, so doing it per condemned proxy turns
    /// a batch with thousands of evictions into thousands of full rebuilds.
    /// They are collected here and applied in one pass at the end of the batch.
    pending_eviction: HashSet<String>,
}

/// When each proxy should be probed again.
///
/// The pool is scheduled per proxy, not per cycle. Re-probing the whole
/// unverified set on every cycle made the batch size equal the pool size, and
/// the pool grew with every scrape — so the time spent checking grew with it
/// too. With a backoff, a proxy that has just failed is looked at again
/// shortly, and one that has been dead for a week is looked at a few times a
/// day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSchedule {
    /// How often a proxy known to work is re-checked.
    pub working_interval_secs: u64,
    /// Wait after each consecutive failure, in seconds. The last entry applies
    /// to every further failure.
    pub failed_backoff_secs: Vec<u64>,
    /// Wait before retrying a proxy whose probe was inconclusive. Shorter than
    /// the failure backoff: an inconclusive probe says nothing about the
    /// proxy, so it deserves an early second look.
    pub inconclusive_backoff_secs: u64,
}

impl Default for ProbeSchedule {
    fn default() -> Self {
        Self {
            working_interval_secs: 600,
            failed_backoff_secs: vec![60, 300, 900, 3600, 10_800, 21_600],
            inconclusive_backoff_secs: 300,
        }
    }
}

impl ProbeSchedule {
    /// Wait before probing a proxy that has failed this many times in a row.
    pub fn failed_backoff(&self, failures: u32) -> u64 {
        let index = failures.saturating_sub(1) as usize;
        self.failed_backoff_secs
            .get(index)
            .copied()
            .or_else(|| self.failed_backoff_secs.last().copied())
            .unwrap_or(3600)
    }
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

    /// Set the score floor.
    pub fn with_min_score(mut self, min_score: f64) -> Self {
        self.min_score = min_score;
        self
    }

    /// Set when each proxy should be probed again.
    pub fn with_probe_schedule(mut self, schedule: ProbeSchedule) -> Self {
        self.probe_schedule = schedule;
        self
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

        // The room is made once. Growing a vector of two hundred thousand
        // records by doubling copies it again and again, and each copy is held
        // alongside the original until it finishes.
        let current = inner.proxies.len();
        let needed = current + discovered.len();
        if needed > inner.proxies.capacity() {
            inner.proxies.reserve(needed - current);
        }

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

    /// Keep the pool within its configured capacity.
    ///
    /// The capacity was a configuration field that nothing enforced, so the
    /// pool grew without bound. The working set is kept and the least useful
    /// proxies are dropped: unverified ones first, then the slowest.
    pub async fn enforce_capacity(&self, max_proxies: usize) -> usize {
        let mut inner = self.inner.write().await;

        if inner.proxies.len() <= max_proxies {
            return 0;
        }

        let overflow = inner.proxies.len() - max_proxies;

        // Rank by usefulness: confirmed first, then by latency, then by score.
        let mut order: Vec<usize> = (0..inner.proxies.len()).collect();
        order.sort_by(|&a, &b| {
            let pa = &inner.proxies[a];
            let pb = &inner.proxies[b];
            pb.last_success
                .cmp(&pa.last_success)
                .then_with(|| pa.latency_ms.cmp(&pb.latency_ms))
                .then_with(|| {
                    pb.score
                        .partial_cmp(&pa.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        let doomed: HashSet<String> = order
            .into_iter()
            .take(overflow)
            .map(|i| inner.proxies[i].id.clone())
            .collect();

        let removed = doomed.len();
        self.retain(&mut inner, |p| !doomed.contains(&p.id));
        removed
    }

    /// Work out the country of every proxy that has an exit address.
    ///
    /// The exit address is what the traffic leaves from, so the country
    /// follows from it. The list's own claim is kept beside it.
    pub async fn geolocate(&self, database: &crate::geo::GeoDatabase) -> usize {
        let mut inner = self.inner.write().await;
        let mut located = 0usize;

        for proxy in inner.proxies.iter_mut() {
            let Some(address) = proxy.exit_ip else {
                continue;
            };
            let Some(code) = database.lookup(address) else {
                continue;
            };

            if proxy.country.as_deref() != Some(code) {
                located += 1;
            }
            proxy.country = Some(code.into());
        }

        located
    }

    /// The country of a proxy, worked out from its exit address.
    ///
    /// Used right after a probe, when the exit address is known but the
    /// country is not.
    pub async fn geolocate_one(
        &self,
        proxy_id: &str,
        database: &crate::geo::GeoDatabase,
    ) -> Option<String> {
        let mut inner = self.inner.write().await;
        let index = *inner.index.get(proxy_id)?;
        let proxy = &mut inner.proxies[index];

        let address = proxy.exit_ip?;
        let code = database.lookup(address)?.to_string();

        proxy.country = Some(code.clone().into());
        Some(code)
    }

    /// Remove every proxy condemned since the last sweep, in one pass.
    ///
    /// Eviction is deferred to here because removal rebuilds the id index: a
    /// sweep per condemned proxy would mean thousands of full rebuilds in one
    /// batch, each of them holding the write lock.
    pub async fn evict_pending(&self) -> usize {
        let mut inner = self.inner.write().await;

        if inner.pending_eviction.is_empty() {
            return 0;
        }

        let condemned = std::mem::take(&mut inner.pending_eviction);
        let count = condemned.len();
        self.retain(&mut inner, |p| !condemned.contains(&p.id));
        count
    }

    /// Insert records exactly as they are, preserving their health state.
    ///
    /// `merge` is for discovery: a freshly scraped proxy genuinely has no known
    /// latency, score or verification, so it is inserted through
    /// `ProxyInfo::discovered`. Restoring from storage is the opposite case —
    /// the record already carries what the health checker learned, and running
    /// it through `discovered()` would make every proxy look unverified again
    /// and send the whole pool back through a full probe cycle on boot.
    pub async fn restore(&self, saved: Vec<ProxyInfo>) -> usize {
        let mut inner = self.inner.write().await;
        let mut added = 0usize;

        for proxy in saved {
            if inner.index.contains_key(&proxy.id) {
                continue;
            }
            let slot = inner.proxies.len();
            inner.index.insert(proxy.id.clone(), slot);
            inner.proxies.push(proxy);
            added += 1;
        }

        tracing::info!(
            added,
            total = inner.proxies.len(),
            "pool restored from storage"
        );
        added
    }

    /// Up to `max` proxies that are due for a probe right now.
    ///
    /// The set is taken in rounds rather than all at once. A pool of two
    /// hundred thousand proxies cloned in one go is two hundred thousand
    /// records held twice over, and the copy is pure overhead: the pool is
    /// already the only place the records live.
    ///
    /// The caller asks for another round when it has finished the last one,
    /// so the memory a check needs is a round rather than the pool.
    pub async fn due_batch(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        max: usize,
    ) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.next_probe_at <= now)
            .take(max)
            .cloned()
            .collect()
    }

    /// How many proxies are due for a probe right now.
    pub async fn due_count(&self, now: chrono::DateTime<chrono::Utc>) -> usize {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| p.next_probe_at <= now)
            .count()
    }

    /// When the next proxy comes due.
    ///
    /// Reported so the operator can see when the pool will be busy again
    /// instead of wondering why a cycle produced nothing.
    pub async fn next_due_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let inner = self.inner.read().await;
        inner.proxies.iter().map(|p| p.next_probe_at).min()
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

    /// The proxies that can carry a tunnel.
    ///
    /// A proxy that only forwards plain HTTP works, and is kept, but it cannot
    /// carry an arbitrary TCP stream, so it is not a candidate for routing.
    ///
    /// `min_score` is a floor on usefulness: a proxy that is confirmed
    /// working but slower than the floor is not worth selecting.
    pub async fn tunnel_capable(&self, min_score: f64) -> Vec<ProxyInfo> {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| {
                p.consecutive_failures == 0
                    && p.last_success.is_some()
                    && p.supports_tunnel
                    && p.score >= min_score
            })
            .cloned()
            .collect()
    }

    /// Fold probe results back into the pool.
    ///
    /// A failed probe increments a streak; the proxy is evicted only once it
    /// fails `max_consecutive_failures` times in a row, or when an unverified
    /// proxy has sat in the pool past its TTL without ever working. Transient
    /// network blips therefore cost a proxy nothing but one failed attempt.
    /// Apply one probe result as soon as it arrives.
    ///
    /// Streaming this per result is what makes the pool usable during a long
    /// batch: applied only at the end, ten thousand proxies left the pool
    /// reporting zero working proxies for the entire run.
    ///
    /// Returns true when the proxy was evicted.
    pub async fn apply_health_result(
        &self,
        result: &HealthResult,
        policy: EvictionPolicy,
        schedule: &ProbeSchedule,
    ) -> bool {
        let now = chrono::Utc::now();
        let mut inner = self.inner.write().await;

        let Some(&i) = inner.index.get(result.proxy_id.as_str()) else {
            return false;
        };

        match result.verdict {
            Verdict::Working => {
                let proxy = &mut inner.proxies[i];
                proxy.latency_ms = Some(result.latency_ms);
                proxy.score = score_for(result.latency_ms);
                proxy.last_checked = Some(now);
                proxy.last_success = Some(now);
                proxy.consecutive_failures = 0;
                // The address the traffic leaves from. Many proxy entries share
                // one exit address, and seeing it makes that visible.
                if let Some(exit_ip) = result.exit_ip {
                    proxy.exit_ip = Some(exit_ip);
                }
                // Whether the proxy can tunnel is a measured fact, not an
                // assumption: a proxy that only forwards plain HTTP is no use
                // for an arbitrary TCP stream.
                proxy.supports_tunnel = result.tunneled;
                // What the proxy revealed about the client, measured the same
                // way: the headers the target received.
                if let Some(anonymity) = result.anonymity {
                    proxy.anonymity = anonymity;
                }
                // A working proxy is re-checked on the normal interval.
                proxy.next_probe_at =
                    now + chrono::Duration::seconds(schedule.working_interval_secs as i64);
                false
            }

            // The probe proved nothing, so leave the streak, the score and
            // `last_success` exactly as they were. It is retried sooner than a
            // real failure, because nothing was actually proven against it.
            Verdict::Inconclusive => {
                let proxy = &mut inner.proxies[i];
                proxy.last_checked = Some(now);
                proxy.next_probe_at =
                    now + chrono::Duration::seconds(schedule.inconclusive_backoff_secs as i64);
                false
            }

            Verdict::Failed => {
                let proxy = &mut inner.proxies[i];
                proxy.last_checked = Some(now);
                proxy.consecutive_failures = proxy.consecutive_failures.saturating_add(1);

                // Decay rather than reset, so the ranking degrades gradually
                // instead of dropping out of contention on one bad probe.
                proxy.score *= 0.5;

                let never_worked = proxy.last_success.is_none();
                // Measured from when the proxy entered the pool. Measuring from
                // `last_checked` reset the age on every failed probe, so this
                // branch never fired and dead proxies accumulated forever.
                let unverified_age = now
                    .signed_duration_since(proxy.first_seen)
                    .num_seconds()
                    .unsigned_abs();

                let exhausted = proxy.consecutive_failures >= policy.max_consecutive_failures;
                let stale_unverified =
                    never_worked && unverified_age >= policy.unverified_ttl_secs as i64 as u64;

                // Back off before deciding: a proxy that has just failed is
                // not due again for a while, so the next cycle skips it.
                proxy.next_probe_at = now
                    + chrono::Duration::seconds(
                        schedule.failed_backoff(proxy.consecutive_failures) as i64,
                    );

                if exhausted || stale_unverified {
                    let id = proxy.id.clone();
                    inner.pending_eviction.insert(id);
                    return true;
                }
                false
            }
        }
    }

    /// Apply a whole batch, one result at a time.
    ///
    /// Kept as a convenience for callers that already hold the results; it
    /// delegates to `apply_health_result` so there is a single implementation
    /// of the eviction policy.
    pub async fn apply_health_results(
        &self,
        results: Vec<HealthResult>,
        policy: EvictionPolicy,
        schedule: &ProbeSchedule,
    ) -> BatchOutcome {
        let mut outcome = BatchOutcome {
            checked: results.len(),
            ..Default::default()
        };

        for result in &results {
            match result.verdict {
                Verdict::Working => outcome.working += 1,
                Verdict::Failed => outcome.failed += 1,
                Verdict::Inconclusive => outcome.inconclusive += 1,
            }
            if self.apply_health_result(result, policy, schedule).await {
                outcome.evicted += 1;
            }
        }

        tracing::info!(
            checked = outcome.checked,
            working = outcome.working,
            failed = outcome.failed,
            inconclusive = outcome.inconclusive,
            evicted = outcome.evicted,
            pool = self.len().await,
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

        // The records are kept in place: taking them out and pushing them
        // back would allocate a new array and copy every one of them.
        let mut index = HashMap::with_capacity(inner.proxies.len());
        let mut position = 0usize;

        inner.proxies.retain_mut(|proxy| {
            let keep = predicate(proxy);
            if keep {
                index.insert(proxy.id.clone(), position);
                position += 1;
            }
            keep
        });

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
    /// The proxies that are confirmed working and can carry a tunnel.
    pub async fn healthy_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner
            .proxies
            .iter()
            .filter(|p| {
                p.last_success.is_some() && p.consecutive_failures == 0 && p.supports_tunnel
            })
            .count()
    }

    /// The proxies confirmed working, whatever their score.
    ///
    /// The count an operator wants to see: a proxy that works but is slow is
    /// still a working proxy, it is just not worth selecting.
    pub async fn working_count(&self) -> usize {
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
            .filter(|p| {
                p.last_success.is_some()
                    && p.consecutive_failures == 0
                    && p.supports_tunnel
                    && p.score >= self.min_score
            })
            .max_by(|a, b| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// Record a failure that the traffic itself discovered.
    ///
    /// A proxy that cannot carry a connection is no use, whatever the last
    /// probe said. The failure is recorded the same way a failed probe is
    /// recorded, so the streak, the score and the backoff all move together,
    /// and the proxy stops being a candidate.
    pub async fn report_failure(&self, id: &str) {
        let now = chrono::Utc::now();
        let mut inner = self.inner.write().await;

        let Some(&i) = inner.index.get(id) else {
            return;
        };

        let proxy = &mut inner.proxies[i];
        proxy.last_checked = Some(now);
        proxy.consecutive_failures = proxy.consecutive_failures.saturating_add(1);
        proxy.score *= 0.5;
        proxy.next_probe_at = now
            + chrono::Duration::seconds(
                self.probe_schedule
                    .failed_backoff(proxy.consecutive_failures) as i64,
            );
    }

    /// The best working proxy that is not in the given set.
    ///
    /// The selection is made without touching the active proxy: a caller that
    /// hands proxies out to its own clients has its own rotation, and the
    /// proxy the device routes through is a separate decision.
    pub async fn best_avoiding(
        &self,
        avoid: &std::collections::HashSet<String>,
    ) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        let now = chrono::Utc::now();

        inner
            .proxies
            .iter()
            .filter(|p| {
                !avoid.contains(&p.id)
                    && !avoid.contains(&format!("host:{}", p.host))
                    && p.last_success.is_some()
                    && p.consecutive_failures == 0
                    && p.supports_tunnel
                    && p.score >= self.min_score
            })
            .max_by(|a, b| {
                effective_score(a, now)
                    .partial_cmp(&effective_score(b, now))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// The best working proxy that the policy allows.
    ///
    /// The policy decides which countries the exit address may be in and
    /// which protocols may be used, so the pool does not hand out a proxy the
    /// policy would refuse.
    pub async fn best_matching(
        &self,
        avoid: &std::collections::HashSet<String>,
        avoid_exits: &std::collections::HashSet<std::net::IpAddr>,
        policy: &crate::credentials::ProxyPolicy,
    ) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        let now = chrono::Utc::now();

        inner
            .proxies
            .iter()
            .filter(|p| {
                !avoid.contains(&p.id)
                    && !avoid.contains(&format!("host:{}", p.host))
                    && !p.exit_ip.is_some_and(|exit| avoid_exits.contains(&exit))
                    && p.last_success.is_some()
                    && p.consecutive_failures == 0
                    && p.supports_tunnel
                    && p.score >= self.min_score
                    && policy.allows(p)
            })
            .max_by(|a, b| {
                effective_score(a, now)
                    .partial_cmp(&effective_score(b, now))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// The proxy with this id, if the pool still has it.
    pub async fn get(&self, id: &str) -> Option<ProxyInfo> {
        let inner = self.inner.read().await;
        let index = *inner.index.get(id)?;
        inner.proxies.get(index).cloned()
    }

    /// Make the best working proxy the active one.
    ///
    /// The proxies that just failed are left out: a proxy that could not
    /// carry the connection is not the one to try next.
    pub async fn activate_best_avoiding(
        &self,
        avoid: &std::collections::HashSet<String>,
    ) -> Option<ProxyInfo> {
        let mut inner = self.inner.write().await;

        let now = chrono::Utc::now();

        let candidate = inner
            .proxies
            .iter()
            .filter(|p| {
                !avoid.contains(&p.id)
                    && !avoid.contains(&format!("host:{}", p.host))
                    && p.last_success.is_some()
                    && p.consecutive_failures == 0
                    && p.supports_tunnel
                    && p.score >= self.min_score
            })
            .max_by(|a, b| {
                effective_score(a, now)
                    .partial_cmp(&effective_score(b, now))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()?;

        let index = inner.index.get(&candidate.id).copied()?;
        inner.active_index = Some(index);

        Some(candidate)
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
/// The score of a proxy, aged by how long ago it was checked.
///
/// A public proxy dies within minutes of working, so a score that says only
/// "this one was fast when it was checked" sends the engine back to a proxy
/// that was verified half an hour ago. The age of the check is part of the
/// evidence: a fresh confirmation is worth more than an old one, and the
/// difference grows the longer the confirmation is.
fn effective_score(proxy: &ProxyInfo, now: chrono::DateTime<chrono::Utc>) -> f64 {
    // The age is measured from the last success, not from the last check: a
    // probe that proved nothing updates the check and leaves the score alone,
    // so an age measured from the check would keep an hours-old result
    // looking fresh.
    let Some(succeeded) = proxy.last_success else {
        return 0.0;
    };

    let age = now.signed_duration_since(succeeded).num_seconds().max(0) as f64;

    // Half the weight after ten minutes, a quarter after twenty, and so on.
    let freshness = 0.5f64.powf(age / 600.0);

    proxy.score * freshness
}

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
            country_reported: None,
            last_checked: None,
            last_success: None,
            first_seen: chrono::Utc::now(),
            next_probe_at: chrono::Utc::now(),
            exit_ip: None,
            password: None,
            cipher: None,
            supports_tunnel: true,
            consecutive_failures: 0,
            score: 0.0,
        }
    }

    fn result(id: &str, alive: bool, latency: u64) -> HealthResult {
        if alive {
            HealthResult::working(id, latency, None, true)
        } else {
            HealthResult::failed(id, "refused")
        }
    }

    fn timeout(id: &str) -> HealthResult {
        HealthResult::inconclusive(id, "read timeout")
    }

    /// A proxy that worked, with a given score and age.
    fn working(id: &str, score: f64, age_secs: i64) -> ProxyInfo {
        let mut proxy = proxy(id);
        proxy.score = score;
        proxy.last_success = Some(chrono::Utc::now() - chrono::Duration::seconds(age_secs));
        proxy.last_checked = proxy.last_success;
        proxy
    }

    #[tokio::test]
    async fn a_fresh_confirmation_outranks_an_old_one() {
        // Both proxies scored the same when they were checked, but one was
        // checked half an hour ago. The fresh one is the better bet: a public
        // proxy that was fast then is very likely dead now.
        let pool = ProxyPool::new();
        pool.restore(vec![working("old", 3.0, 1800), working("fresh", 3.0, 0)])
            .await;

        let next = pool
            .activate_best_avoiding(&std::collections::HashSet::new())
            .await
            .expect("a working proxy");

        assert_eq!(next.id, "fresh");
    }

    #[tokio::test]
    async fn a_fresh_proxy_outranks_a_stale_one_with_a_better_score() {
        // The stale proxy had the better score when it was measured. It is
        // four hours old, and a public proxy that was fast then is very
        // likely dead now.
        let pool = ProxyPool::new();
        pool.restore(vec![
            working("stale", 3.0, 4 * 3600),
            working("fresh", 0.5, 60),
        ])
        .await;

        let next = pool
            .best_avoiding(&std::collections::HashSet::new())
            .await
            .expect("a working proxy");

        assert_eq!(next.id, "fresh");
    }

    #[tokio::test]
    async fn a_session_is_held_apart_by_its_address() {
        // Two entries of the pool are the same server under different names,
        // and they leave from the same address. A session that is held apart
        // by the entry alone would still share the address.
        let pool = ProxyPool::new();

        let mut first = working("a", 3.0, 0);
        first.exit_ip = Some("198.51.100.7".parse().unwrap());
        let mut second = working("b", 2.5, 0);
        second.exit_ip = Some("198.51.100.7".parse().unwrap());
        let mut third = working("c", 2.0, 0);
        third.exit_ip = Some("198.51.100.8".parse().unwrap());

        pool.restore(vec![first, second, third]).await;

        let avoid: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
        let exits: std::collections::HashSet<std::net::IpAddr> =
            ["198.51.100.7".parse::<std::net::IpAddr>().unwrap()]
                .into_iter()
                .collect();

        let next = pool
            .best_matching(&avoid, &exits, &crate::credentials::ProxyPolicy::default())
            .await
            .expect("a working proxy");

        assert_eq!(next.id, "c", "the proxy with the same address is left out");
    }

    #[tokio::test]
    async fn a_failed_proxy_is_stepped_aside() {
        let pool = ProxyPool::new();
        pool.restore(vec![working("a", 3.0, 0), working("b", 2.0, 0)])
            .await;

        let avoid: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
        let next = pool
            .activate_best_avoiding(&avoid)
            .await
            .expect("a working proxy");

        assert_eq!(next.id, "b");
        assert_eq!(pool.active().await.map(|p| p.id), Some("b".to_string()));
    }

    #[tokio::test]
    async fn a_failure_the_traffic_found_is_recorded() {
        let pool = ProxyPool::new();
        pool.restore(vec![working("a", 3.0, 0)]).await;

        pool.report_failure("a").await;

        let proxy = &pool.all().await[0];
        assert_eq!(proxy.consecutive_failures, 1);
        assert!(proxy.score < 3.0, "the score must decay");
    }

    #[tokio::test]
    async fn a_single_failure_does_not_evict() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(
            vec![result("a", true, 100), result("b", false, 0)],
            policy,
            &ProbeSchedule::default(),
        )
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
            pool.apply_health_results(
                vec![result("a", true, 100), result("b", false, 0)],
                policy,
                &ProbeSchedule::default(),
            )
            .await;
            // Removal is deferred to a single sweep, so the streak is
            // accumulated across batches exactly as it is in production.
            pool.evict_pending().await;
        }

        assert_eq!(pool.len().await, 1);
        assert_eq!(pool.all().await[0].id, "a");
    }

    #[tokio::test]
    async fn success_clears_the_failure_streak() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(
            vec![result("a", false, 0)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;
        pool.apply_health_results(
            vec![result("a", false, 0)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;
        pool.apply_health_results(
            vec![result("a", true, 120)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;
        pool.apply_health_results(
            vec![result("a", false, 0)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;

        assert_eq!(pool.len().await, 1, "streak was reset by the success");
        assert_eq!(pool.healthy_count().await, 0, "last probe failed");
    }

    #[tokio::test]
    async fn merge_preserves_health_state_of_known_proxies() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a")]).await;

        let policy = EvictionPolicy::default();
        pool.apply_health_results(
            vec![result("a", true, 90)],
            policy,
            &ProbeSchedule::default(),
        )
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
        pool.apply_health_results(
            vec![result("a", true, 100)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;
        assert_eq!(pool.healthy_count().await, 1);

        // "a" times out, "b" times out. Neither is evidence of a dead proxy.
        let outcome = pool
            .apply_health_results(
                vec![timeout("a"), timeout("b")],
                policy,
                &ProbeSchedule::default(),
            )
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
        pool.apply_health_results(
            vec![result("a", true, 80)],
            policy,
            &ProbeSchedule::default(),
        )
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
            &ProbeSchedule::default(),
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
        pool.apply_health_results(
            vec![result("a", true, 100), result("b", true, 900)],
            policy,
            &ProbeSchedule::default(),
        )
        .await;

        assert_eq!(pool.best().await.map(|p| p.id), Some("a".to_string()));

        pool.set_active("a").await;
        assert_eq!(
            pool.rotate().await.map(|p| p.id),
            Some("b".to_string()),
            "rotate must advance to the next proxy"
        );
    }

    #[tokio::test]
    async fn restore_keeps_verification_and_merge_does_not() {
        use crate::proxy::{Anonymity, ProxyProtocol};

        let pool = ProxyPool::new();
        let now = chrono::Utc::now();

        let saved = |id: &str, confirmed: bool| ProxyInfo {
            id: id.to_string(),
            host: "198.51.100.1".to_string(),
            port: 8080,
            protocol: ProxyProtocol::Socks5,
            anonymity: Anonymity::Elite,
            latency_ms: Some(90),
            country: None,
            country_reported: None,
            last_checked: Some(now),
            last_success: confirmed.then_some(now),
            first_seen: chrono::Utc::now(),
            next_probe_at: chrono::Utc::now(),
            exit_ip: None,
            supports_tunnel: true,
            password: None,
            cipher: None,
            consecutive_failures: 0,
            score: 0.9,
        };

        // Restoring must keep the verification, or the whole pool is re-probed.
        pool.restore(vec![saved("a:1", true), saved("b:2", false)])
            .await;
        assert_eq!(
            pool.healthy_count().await,
            1,
            "confirmed proxy must stay working"
        );
        assert_eq!(
            pool.unverified().await.len(),
            1,
            "only the unverified one needs probing"
        );

        // A scrape uses `discovered`, so a known proxy keeps its state.
        pool.merge(vec![saved("a:1", true)]).await;
        assert_eq!(
            pool.healthy_count().await,
            1,
            "a scrape must not reset health"
        );

        let fresh = ProxyInfo {
            id: "c:3".to_string(),
            host: "198.51.100.3".to_string(),
            port: 3128,
            protocol: ProxyProtocol::Http,
            anonymity: Anonymity::Unknown,
            latency_ms: Some(500),
            country: None,
            country_reported: None,
            last_checked: Some(now),
            last_success: Some(now),
            first_seen: chrono::Utc::now(),
            next_probe_at: chrono::Utc::now(),
            exit_ip: None,
            supports_tunnel: true,
            password: None,
            cipher: None,
            consecutive_failures: 0,
            score: 0.5,
        };
        pool.merge(vec![fresh]).await;
        assert_eq!(
            pool.healthy_count().await,
            1,
            "a newly scraped proxy is unverified until it is probed"
        );
    }

    #[tokio::test]
    async fn an_unverified_proxy_is_evicted_once_its_ttl_expires() {
        use crate::proxy::{Anonymity, ProxyProtocol};

        let pool = ProxyPool::new();
        let policy = EvictionPolicy {
            max_consecutive_failures: 3,
            unverified_ttl_secs: 3600,
        };

        let mut stale = ProxyInfo {
            id: "old:1".to_string(),
            host: "198.51.100.1".to_string(),
            port: 8080,
            protocol: ProxyProtocol::Http,
            anonymity: Anonymity::Unknown,
            latency_ms: None,
            country: None,
            country_reported: None,
            last_checked: None,
            last_success: None,
            first_seen: chrono::Utc::now() - chrono::Duration::seconds(7200),
            next_probe_at: chrono::Utc::now(),
            exit_ip: None,
            supports_tunnel: true,
            password: None,
            cipher: None,
            consecutive_failures: 0,
            score: 0.0,
        };

        // `restore` inserts the record verbatim, which is how a pool reloaded
        // from storage arrives. `add` would reset `first_seen`, because a
        // manual add asserts the endpoint afresh.
        pool.restore(vec![stale.clone()]).await;

        let result = HealthResult {
            proxy_id: stale.id.clone(),
            verdict: Verdict::Failed,
            latency_ms: 0,
            exit_ip: None,
            tunneled: true,
            anonymity: None,
            error: None,
        };

        // Measured from `first_seen`, not from `last_checked`: a proxy that has
        // been failing for two hours must not be kept alive by its own last
        // failure resetting the clock.
        assert!(
            pool.apply_health_result(&result, policy, &ProbeSchedule::default())
                .await,
            "a never-working proxy past its TTL must be condemned"
        );
        // Removal is deferred to one sweep at the end of the batch.
        assert_eq!(pool.evict_pending().await, 1);
        assert_eq!(pool.len().await, 0);

        // The same verdict on a fresh proxy must not evict it.
        stale.id = "new:2".to_string();
        stale.first_seen = chrono::Utc::now();
        pool.add(stale.clone()).await;
        let result = HealthResult {
            proxy_id: stale.id.clone(),
            verdict: Verdict::Failed,
            latency_ms: 0,
            exit_ip: None,
            tunneled: true,
            anonymity: None,
            error: None,
        };
        assert!(
            !pool
                .apply_health_result(&result, policy, &ProbeSchedule::default())
                .await
        );
        assert_eq!(pool.evict_pending().await, 0);
        assert_eq!(pool.len().await, 1, "a single failure never evicts");
    }

    #[tokio::test]
    async fn a_failed_proxy_backs_off_and_stops_being_due() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy {
            max_consecutive_failures: 10,
            unverified_ttl_secs: 86_400,
        };
        let schedule = ProbeSchedule {
            working_interval_secs: 600,
            failed_backoff_secs: vec![300, 900, 3600],
            inconclusive_backoff_secs: 120,
        };

        let now = chrono::Utc::now();
        assert_eq!(
            pool.due_batch(now, usize::MAX).await.len(),
            2,
            "new proxies are due"
        );

        // "a" fails: it must be scheduled into the future, not re-probed on
        // the next cycle.
        pool.apply_health_result(&result("a", false, 0), policy, &schedule)
            .await;

        let due = pool.due_batch(now, usize::MAX).await;
        assert_eq!(due.len(), 1, "a failed proxy must not be due again at once");
        assert_eq!(due[0].id, "b");

        // It comes back after the backoff, not before.
        let shortly = now + chrono::Duration::seconds(60);
        assert_eq!(pool.due_batch(shortly, usize::MAX).await.len(), 1);
        let after = now + chrono::Duration::seconds(400);
        assert_eq!(pool.due_batch(after, usize::MAX).await.len(), 2);
    }

    #[tokio::test]
    async fn a_working_proxy_is_rechecked_on_the_working_interval() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a")]).await;

        let policy = EvictionPolicy::default();
        let schedule = ProbeSchedule {
            working_interval_secs: 600,
            failed_backoff_secs: vec![300],
            inconclusive_backoff_secs: 120,
        };

        let now = chrono::Utc::now();
        pool.apply_health_result(&result("a", true, 50), policy, &schedule)
            .await;

        assert_eq!(pool.due_batch(now, usize::MAX).await.len(), 0);
        assert_eq!(
            pool.due_batch(now + chrono::Duration::seconds(599), usize::MAX)
                .await
                .len(),
            0
        );
        assert_eq!(
            pool.due_batch(now + chrono::Duration::seconds(601), usize::MAX)
                .await
                .len(),
            1,
            "a working proxy is due again after its interval"
        );
    }

    #[tokio::test]
    async fn an_inconclusive_proxy_is_retried_sooner_than_a_failed_one() {
        let pool = ProxyPool::new();
        pool.merge(vec![proxy("a"), proxy("b")]).await;

        let policy = EvictionPolicy::default();
        let schedule = ProbeSchedule {
            working_interval_secs: 600,
            failed_backoff_secs: vec![3600],
            inconclusive_backoff_secs: 120,
        };

        let now = chrono::Utc::now();
        let inconclusive = HealthResult {
            proxy_id: "a".to_string(),
            verdict: Verdict::Inconclusive,
            latency_ms: 0,
            exit_ip: None,
            tunneled: true,
            anonymity: None,
            error: None,
        };
        pool.apply_health_result(&inconclusive, policy, &schedule)
            .await;
        pool.apply_health_result(&result("b", false, 0), policy, &schedule)
            .await;

        // The inconclusive one is due long before the failed one.
        let later = now + chrono::Duration::seconds(200);
        let due = pool.due_batch(later, usize::MAX).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "a");
    }
}

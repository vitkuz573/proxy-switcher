use crate::forwarder::Forwarder;
use crate::proxy::ProxyInfo;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

pub struct HealthChecker {
    concurrency: Arc<Semaphore>,
    target_url: String,
    /// Budget for the proxy handshake.
    ///
    /// Kept separate from the exchange budget because they measure different
    /// things. A proxy that cannot complete a handshake is dead, and a dead
    /// proxy is the common case — most of the batch time was spent waiting
    /// out the full timeout on endpoints that will never answer. Spending the
    /// exchange budget on them made every batch several times longer than it
    /// needed to be.
    connect_timeout: Duration,
    /// Budget for writing the request and reading the response.
    timeout: Duration,
}

/// Outcome of a single probe.
///
/// `Inconclusive` exists because a timeout says nothing about the proxy: the
/// host may simply have been busy. Counting those as failures made the healthy
/// total swing by half on every re-check, even though the eviction policy
/// tolerates three real failures in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The proxy carried a request and returned a real HTTP response.
    Working,
    /// Conclusive evidence the endpoint is not a usable proxy.
    Failed,
    /// The probe timed out or the network gave up. The proxy's state is unknown.
    Inconclusive,
}

#[derive(Debug)]
pub struct HealthResult {
    pub proxy_id: String,
    pub verdict: Verdict,
    pub latency_ms: u64,
    pub error: Option<String>,
}

impl HealthResult {
    pub fn working(proxy_id: &str, latency_ms: u64) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Working,
            latency_ms,
            error: None,
        }
    }

    pub fn failed(proxy_id: &str, reason: impl Into<String>) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Failed,
            latency_ms: 0,
            error: Some(reason.into()),
        }
    }

    pub fn inconclusive(proxy_id: &str, reason: impl Into<String>) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Inconclusive,
            latency_ms: 0,
            error: Some(reason.into()),
        }
    }

    pub fn is_working(&self) -> bool {
        self.verdict == Verdict::Working
    }
}

/// Split a target URL into (host, port, path).
///
/// The path must be stripped from the host: for `http://httpbin.org/ip` a naive
/// split leaves the host as `httpbin.org/ip`, which never resolves, so every
/// proxy is reported dead.
fn parse_target(url: &str) -> (String, u16, String) {
    let rest = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{}", p)),
        None => (rest, "/".to_string()),
    };

    let path = path
        .split(['?', '#'])
        .next()
        .filter(|p| !p.is_empty())
        .unwrap_or("/")
        .to_string();

    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_string(), p.parse().unwrap_or(80))
        }
        _ => (authority.to_string(), 80),
    };

    // Strip IPv6 brackets, e.g. `[::1]:8080`
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();

    (host, port, path)
}

/// Probe one proxy by tunnelling to the target host through it and requiring a
/// real HTTP response.
///
/// The status code is deliberately *not* part of the verdict. Thousands of
/// probes arrive at one shared target, and that target rate-limits. Treating a
/// `429` or `503` as a dead proxy silently deleted working proxies whenever the
/// target throttled us. What proves a proxy works is the ability to carry a
/// request to the target and bring a valid HTTP response back; who then decided
/// to answer 503 is the target's business, not the proxy's.
#[allow(clippy::too_many_arguments)]
async fn check_one(
    proxy: &ProxyInfo,
    host: &str,
    port: u16,
    path: &str,
    connect_timeout: Duration,
    timeout: Duration,
) -> HealthResult {
    let start = Instant::now();

    let mut conn =
        match tokio::time::timeout(connect_timeout, Forwarder::connect_to(proxy, host, port)).await
        {
            Ok(Ok(conn)) => conn,
            // Refused, reset, DNS failure: the endpoint is not usable.
            Ok(Err(e)) => return HealthResult::failed(&proxy.id, format!("connect error: {e}")),
            // Nothing came back in time — the proxy's state is unknown.
            Err(_) => return HealthResult::inconclusive(&proxy.id, "connect timeout"),
        };

    let authority = if port == 80 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\n\
         User-Agent: proxy-switcher-health/1.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );

    match tokio::time::timeout(timeout, conn.write_all(request.as_bytes())).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return HealthResult::failed(&proxy.id, format!("write error: {e}")),
        Err(_) => return HealthResult::inconclusive(&proxy.id, "write timeout"),
    }

    let mut buf = Vec::new();
    match tokio::time::timeout(timeout, conn.read_to_end(&mut buf)).await {
        Ok(Ok(_)) => {
            let response = String::from_utf8_lossy(&buf);
            let status = response
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();

            // A valid status line proves the tunnel carried a request and
            // returned a response. Anything else means this endpoint is not
            // actually a proxy — often a plain web server on the same port.
            if status.starts_with("HTTP/1.0") || status.starts_with("HTTP/1.1") {
                HealthResult::working(&proxy.id, start.elapsed().as_millis() as u64)
            } else if status.is_empty() {
                HealthResult::inconclusive(&proxy.id, "no HTTP response")
            } else {
                HealthResult::failed(&proxy.id, format!("not an HTTP proxy: {status}"))
            }
        }
        Ok(Err(e)) => HealthResult::failed(&proxy.id, format!("read error: {e}")),
        Err(_) => HealthResult::inconclusive(&proxy.id, "read timeout"),
    }
}

impl HealthChecker {
    pub fn new(
        concurrency: usize,
        timeout_secs: u64,
        connect_timeout_secs: u64,
        target_url: String,
    ) -> Self {
        Self {
            concurrency: Arc::new(Semaphore::new(concurrency)),
            target_url,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Probe a single proxy, taking one concurrency permit.
    pub async fn check(&self, proxy: &ProxyInfo) -> HealthResult {
        let _permit = self.concurrency.acquire().await.expect("Semaphore closed");
        let (host, port, path) = parse_target(&self.target_url);
        check_one(
            proxy,
            &host,
            port,
            &path,
            self.connect_timeout,
            self.timeout,
        )
        .await
    }

    /// Probe every proxy with bounded concurrency.
    ///
    /// `checked` is reset to 0 and incremented as each probe finishes, so callers
    /// can expose live progress instead of a counter frozen at zero for the
    /// whole batch.
    /// Probe every proxy, handing each result to `on_result` as it arrives.
    ///
    /// Results are streamed rather than returned as one vector because a batch
    /// of ten thousand proxies runs for minutes. Collecting first and applying
    /// afterwards left the pool reporting zero working proxies for the whole
    /// run: nothing was marked usable until the last probe finished.
    pub async fn check_batch<F, Fut>(
        &self,
        proxies: &[ProxyInfo],
        checked: Arc<AtomicUsize>,
        mut on_result: F,
    ) -> CheckCounts
    where
        F: FnMut(HealthResult) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let total = proxies.len();
        checked.store(0, Ordering::Relaxed);

        // Channel capacity equals the batch size, so probe tasks never block on
        // the result send and the drain loop is not part of the critical path.
        let (tx, mut rx) = tokio::sync::mpsc::channel(total.max(1));

        for proxy in proxies {
            let proxy = proxy.clone();
            // Take the permit before spawning so at most `concurrency` probes
            // are in flight at any moment.
            let permit = self
                .concurrency
                .clone()
                .acquire_owned()
                .await
                .expect("Semaphore closed");
            let (host, port, path) = parse_target(&self.target_url);
            let timeout = self.timeout;
            let connect_timeout = self.connect_timeout;
            let tx = tx.clone();
            let checked = checked.clone();

            tokio::spawn(async move {
                let _permit = permit;
                let result = check_one(&proxy, &host, port, &path, connect_timeout, timeout).await;
                checked.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(result).await;
            });
        }
        drop(tx);

        let mut counts = CheckCounts::default();
        while let Some(result) = rx.recv().await {
            match result.verdict {
                Verdict::Working => counts.working += 1,
                Verdict::Failed => counts.failed += 1,
                Verdict::Inconclusive => counts.inconclusive += 1,
            }
            on_result(result).await;
        }

        tracing::info!(
            checked = counts.working + counts.failed + counts.inconclusive,
            total,
            working = counts.working,
            failed = counts.failed,
            inconclusive = counts.inconclusive,
            "Health check batch complete"
        );
        counts
    }
}

/// Tally of a streamed batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CheckCounts {
    pub working: usize,
    pub failed: usize,
    pub inconclusive: usize,
}

#[cfg(test)]
mod tests {
    use super::parse_target;

    #[test]
    fn parses_url_with_path() {
        let (host, port, path) = parse_target("http://httpbin.org/ip");
        assert_eq!(host, "httpbin.org");
        assert_eq!(port, 80);
        assert_eq!(path, "/ip");
    }

    #[test]
    fn parses_bare_host_and_port() {
        assert_eq!(
            parse_target("example.com"),
            ("example.com".into(), 80, "/".into())
        );
        assert_eq!(
            parse_target("https://example.com:8443/healthz"),
            ("example.com".into(), 8443, "/healthz".into())
        );
    }

    #[test]
    fn strips_query_and_ipv6_brackets() {
        let (host, port, path) = parse_target("http://[::1]:8080/status?x=1");
        assert_eq!(host, "::1");
        assert_eq!(port, 8080);
        assert_eq!(path, "/status");
    }
}

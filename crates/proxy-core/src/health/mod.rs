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
    timeout: Duration,
}

#[derive(Debug)]
pub struct HealthResult {
    pub proxy_id: String,
    pub alive: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
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
/// real HTTP response. A host that merely accepts TCP connections is not a
/// working proxy and must not be counted as alive.
async fn check_one(
    proxy: &ProxyInfo,
    host: &str,
    port: u16,
    path: &str,
    timeout: Duration,
) -> HealthResult {
    let start = Instant::now();

    let mut conn =
        match tokio::time::timeout(timeout, Forwarder::connect_to(proxy, host, port)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                return HealthResult {
                    proxy_id: proxy.id.clone(),
                    alive: false,
                    latency_ms: 0,
                    error: Some(format!("connect error: {e}")),
                }
            }
            Err(_) => {
                return HealthResult {
                    proxy_id: proxy.id.clone(),
                    alive: false,
                    latency_ms: 0,
                    error: Some("connect timeout".into()),
                }
            }
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
        Ok(Err(e)) => {
            return HealthResult {
                proxy_id: proxy.id.clone(),
                alive: false,
                latency_ms: 0,
                error: Some(format!("write error: {e}")),
            }
        }
        Err(_) => {
            return HealthResult {
                proxy_id: proxy.id.clone(),
                alive: false,
                latency_ms: 0,
                error: Some("write timeout".into()),
            }
        }
    }

    let mut buf = Vec::new();
    match tokio::time::timeout(timeout, conn.read_to_end(&mut buf)).await {
        Ok(Ok(_)) => {
            let response = String::from_utf8_lossy(&buf);
            let status = response.lines().next().unwrap_or_default().trim().to_string();
            let alive = status.starts_with("HTTP/1.")
                && status
                    .split_whitespace()
                    .nth(1)
                    .and_then(|code| code.parse::<u16>().ok())
                    .is_some_and(|code| (200..400).contains(&code));

            HealthResult {
                proxy_id: proxy.id.clone(),
                alive,
                latency_ms: start.elapsed().as_millis() as u64,
                error: if alive {
                    None
                } else {
                    Some(format!("unexpected response: {status}"))
                },
            }
        }
        Ok(Err(e)) => HealthResult {
            proxy_id: proxy.id.clone(),
            alive: false,
            latency_ms: 0,
            error: Some(format!("read error: {e}")),
        },
        Err(_) => HealthResult {
            proxy_id: proxy.id.clone(),
            alive: false,
            latency_ms: 0,
            error: Some("read timeout".into()),
        },
    }
}

impl HealthChecker {
    pub fn new(concurrency: usize, timeout_secs: u64, target_url: String) -> Self {
        Self {
            concurrency: Arc::new(Semaphore::new(concurrency)),
            target_url,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Probe a single proxy, taking one concurrency permit.
    pub async fn check(&self, proxy: &ProxyInfo) -> HealthResult {
        let _permit = self.concurrency.acquire().await.expect("Semaphore closed");
        let (host, port, path) = parse_target(&self.target_url);
        check_one(proxy, &host, port, &path, self.timeout).await
    }

    /// Probe every proxy with bounded concurrency.
    ///
    /// `checked` is reset to 0 and incremented as each probe finishes, so callers
    /// can expose live progress instead of a counter frozen at zero for the
    /// whole batch.
    pub async fn check_batch(
        &self,
        proxies: &[ProxyInfo],
        checked: Arc<AtomicUsize>,
    ) -> Vec<HealthResult> {
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
            let tx = tx.clone();
            let checked = checked.clone();

            tokio::spawn(async move {
                let _permit = permit;
                let result = check_one(&proxy, &host, port, &path, timeout).await;
                checked.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(result).await;
            });
        }
        drop(tx);

        let mut results = Vec::with_capacity(total);
        while let Some(result) = rx.recv().await {
            results.push(result);
        }

        tracing::info!(
            "Health check batch: {}/{} probes completed",
            results.len(),
            total
        );
        results
    }
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
        assert_eq!(parse_target("example.com"), ("example.com".into(), 80, "/".into()));
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
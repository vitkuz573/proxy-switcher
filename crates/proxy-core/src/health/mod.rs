use crate::proxy::connector::base::Carriage;
use crate::proxy::ProxyInfo;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

/// A probe target that answered a direct request.
#[derive(Debug, Clone)]
pub struct ActiveTarget {
    pub url: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    /// The address the engine reached the target from.
    pub own_exit_ip: Option<std::net::IpAddr>,
    /// Whether the engine reached the target directly.
    ///
    /// An unverified target still works: the proxies may reach it even when
    /// the engine cannot. What is lost is the comparison against the engine's
    /// own address, and with it the ability to spot an impostor.
    pub verified: bool,
    checked_at: std::time::Instant,
}

pub struct HealthChecker {
    concurrency: Arc<Semaphore>,
    /// Probe targets, tried in order.
    ///
    /// More than one, because the target is a third party: it can be
    /// overloaded or simply down, and the network a proxy sits on may not see
    /// the target the engine itself reaches. A single unreachable target
    /// would otherwise mark every proxy in the pool as dead.
    target_urls: Vec<String>,
    /// How many targets one proxy may try.
    ///
    /// Every target that does not answer costs the proxy a full timeout, and
    /// the pool is large: a proxy that tried all of them would make the
    /// batch several times longer than it needs to be, and the pool would be
    /// confirmed less often than the proxies die. Two is enough to widen the
    /// working set without holding the round up.
    max_targets_per_proxy: usize,
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
    /// Firewall mark for the socket to the proxy.
    ///
    /// The health check opens the very connection the TUN would otherwise
    /// capture, so it has to bypass the TUN: a probe that is routed through a
    /// proxy measures the wrong thing, and a probe to the active proxy itself
    /// is a loop.
    mark: Option<u32>,
    /// Opens the connection to the proxy.
    forwarder: crate::forwarder::Forwarder,
    /// The target currently in use, and when it was last confirmed.
    ///
    /// The target is verified directly before it is used, so the engine only
    /// ever draws conclusions about a proxy when it can reach the target
    /// itself.
    active_target: std::sync::RwLock<Option<ActiveTarget>>,
    /// How long a confirmed target stays valid.
    target_ttl: std::time::Duration,
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
    /// Whether the proxy carried the request through a tunnel.
    pub tunneled: bool,
    /// How much the proxy revealed about the client.
    pub anonymity: Option<crate::proxy::Anonymity>,
    /// The address the target reported for the request.
    ///
    /// This is the address the traffic actually left from. Comparing it with
    /// the engine's own address is what proves the endpoint is a working proxy
    /// rather than something that merely answers HTTP.
    pub exit_ip: Option<std::net::IpAddr>,
}

impl HealthResult {
    pub fn working(
        proxy_id: &str,
        latency_ms: u64,
        exit_ip: Option<std::net::IpAddr>,
        tunneled: bool,
    ) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Working,
            latency_ms,
            error: None,
            exit_ip,
            tunneled,
            anonymity: None,
        }
    }

    pub fn failed(proxy_id: &str, reason: impl Into<String>) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Failed,
            latency_ms: 0,
            error: Some(reason.into()),
            exit_ip: None,
            tunneled: false,
            anonymity: None,
        }
    }

    pub fn inconclusive(proxy_id: &str, reason: impl Into<String>) -> Self {
        Self {
            proxy_id: proxy_id.to_string(),
            verdict: Verdict::Inconclusive,
            latency_ms: 0,
            error: Some(reason.into()),
            exit_ip: None,
            tunneled: false,
            anonymity: None,
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
    mark: Option<u32>,
    own_exit_ip: Option<std::net::IpAddr>,
    forwarder: &crate::forwarder::Forwarder,
) -> HealthResult {
    let start = Instant::now();

    // One deadline for the whole probe, not one per phase.
    //
    // The phases used to get the full budget each, so a proxy that hung could
    // take the connect timeout, then the write timeout, then the read timeout:
    // thirteen seconds for one probe, and the batch moved at a third of the
    // rate the concurrency allowed. The phases now share what is left of the
    // budget, so a probe costs what it is allowed to cost.
    let deadline = start + timeout;

    let mut conn = match tokio::time::timeout(
        connect_timeout.min(timeout),
        forwarder.connect_to(proxy, host, port, mark),
    )
    .await
    {
        Ok(Ok(conn)) => conn,
        // Refused, reset, DNS failure: the endpoint is not usable as a tunnel.
        //
        // It may still be a plain-forwarding proxy, which is no use for a
        // tunnel but plenty of use for HTTP. A large part of the public proxy
        // population answers CONNECT with an error and then serves ordinary
        // requests perfectly well; calling those proxies broken throws away
        // working entries.
        Ok(Err(tunnel_error)) => {
            if !Carriage::is_plain_http(port) {
                return HealthResult::failed(&proxy.id, format!("connect error: {tunnel_error}"));
            }

            return match check_plain_forwarding(proxy, host, path, timeout, mark, start).await {
                Ok(result) => result,
                Err(reason) => HealthResult::failed(
                    &proxy.id,
                    format!("no tunnel ({tunnel_error}) and no plain forwarding: {reason}"),
                ),
            };
        }
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

    let remaining = || deadline.saturating_duration_since(Instant::now());

    match tokio::time::timeout(remaining(), conn.write_all(request.as_bytes())).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return HealthResult::failed(&proxy.id, format!("write error: {e}")),
        Err(_) => return HealthResult::inconclusive(&proxy.id, "write timeout"),
    }

    let mut buf = Vec::new();
    match tokio::time::timeout(remaining(), conn.read_to_end(&mut buf)).await {
        Ok(Ok(_)) => {
            let response = String::from_utf8_lossy(&buf);
            let status = response
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();

            // The status line alone is not proof. A plain web server on the
            // same port, an intercepting middlebox, or a captive portal all
            // answer with a valid status line, and all of them were counted
            // as working proxies.
            //
            // What proves it is the target itself answering: the request went
            // through the tunnel and came back from the destination.
            if !status.starts_with("HTTP/1.0") && !status.starts_with("HTTP/1.1") {
                return if status.is_empty() {
                    HealthResult::inconclusive(&proxy.id, "no HTTP response")
                } else {
                    HealthResult::failed(&proxy.id, format!("not an HTTP proxy: {status}"))
                };
            }

            let code = status
                .split_whitespace()
                .nth(1)
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(0);

            // 407 is a proxy that wants credentials: it cannot be used. The
            // tunnel was established, so this is the proxy answering, and it
            // is the one thing that is conclusively wrong with it.
            if code == 407 {
                return HealthResult::failed(&proxy.id, "proxy requires authentication");
            }

            // A non-success answer is not evidence against the proxy. The
            // tunnel was established, so the proxy did relay the request; the
            // status is the target's business, and the target may be down,
            // overloaded or blocking this network. Counting that as a failure
            // condemns the whole pool whenever a third-party service has a
            // bad day.
            if !(200..300).contains(&code) {
                return HealthResult::inconclusive(&proxy.id, format!("target answered {status}"));
            }

            // The body carries the address the target saw. It is both the
            // proof that the request really travelled through the proxy and
            // the address the traffic will leave from.
            let body = String::from_utf8_lossy(&buf);
            let view = parse_target_view(&body);
            let Some(exit_ip) = view.exit_ip else {
                // The target always reports the address it sees. An answer
                // without one did not come from the target: the proxy
                // answered in its place.
                //
                // That is not a rare accident. A captive portal, an
                // intercepting middlebox and a proxy that serves its own
                // page all answer with a status of 200 and a page of their
                // own, and every one of them was being counted as a working
                // proxy. The address is what tells them apart, so an answer
                // without one is the proxy failing.
                return HealthResult::failed(
                    &proxy.id,
                    "the proxy answered in place of the target",
                );
            };

            // The same address the engine gets means the request never left
            // through the proxy: the endpoint answered, but did not relay.
            if own_exit_ip == Some(exit_ip) {
                return HealthResult::failed(&proxy.id, "the endpoint did not proxy the request");
            }

            HealthResult {
                proxy_id: proxy.id.clone(),
                verdict: Verdict::Working,
                latency_ms: start.elapsed().as_millis() as u64,
                error: None,
                exit_ip: Some(exit_ip),
                tunneled: true,
                anonymity: classify_anonymity(&view, own_exit_ip),
            }
        }
        Ok(Err(e)) => HealthResult::failed(&proxy.id, format!("read error: {e}")),
        Err(_) => HealthResult::inconclusive(&proxy.id, "read timeout"),
    }
}

/// Probe a proxy that does not tunnel: send an ordinary request with an
/// absolute URI and read the response.
///
/// The response is the target's, relayed by the proxy, and the address in it
/// is the proxy's own — the same proof a tunnel gives, reached the other way.
async fn check_plain_forwarding(
    proxy: &ProxyInfo,
    host: &str,
    path: &str,
    timeout: Duration,
    mark: Option<u32>,
    start: Instant,
) -> Result<HealthResult, String> {
    // The connection to the proxy is bounded, like every other one.
    //
    // It was not, and that was enough to hang a whole round: a proxy that
    // blackholes the connection — the SYN goes out and nothing comes back —
    // failed the tunnel, and the fallback connection then waited forever. The
    // round never finished, and with it the whole check.
    let address = std::net::SocketAddr::new(
        proxy
            .host
            .parse::<std::net::IpAddr>()
            .map_err(|_| "the proxy address is not numeric".to_string())?,
        proxy.port,
    );

    let mut stream = tokio::time::timeout(timeout, crate::net::connect_tcp(address, mark))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| e.to_string())?;

    crate::proxy::connector::http_plain::HttpPlainConnector::write_absolute_request(
        &mut stream,
        path,
        host,
    )
    .await
    .map_err(|e| e.to_string())?;

    let response = tokio::time::timeout(
        timeout,
        crate::proxy::connector::http_plain::read_response(&mut stream),
    )
    .await
    .map_err(|_| "read timeout".to_string())?
    .map_err(|e| e.to_string())?;

    let status = response
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !status.starts_with("HTTP/1.") {
        return Err(format!("not an HTTP response: {status}"));
    }

    let code = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);

    if code == 407 {
        return Err("the proxy requires authentication".to_string());
    }
    if !(200..300).contains(&code) {
        return Err(format!("the target answered {status}"));
    }

    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default();

    let Some(exit_ip) = extract_exit_ip(body) else {
        // The same rule as for a tunnel: an answer without an address did not
        // come from the target.
        return Err("the proxy answered in place of the target".to_string());
    };

    Ok(HealthResult::working(
        &proxy.id,
        start.elapsed().as_millis() as u64,
        Some(exit_ip),
        false,
    ))
}

/// What the target saw of the request.
struct TargetView {
    /// The address the request came from.
    exit_ip: Option<std::net::IpAddr>,
    /// Headers the target received.
    headers: Vec<(String, String)>,
}

/// Parse what the target reported.
///
/// The echo services answer either with a bare address or with a small JSON
/// document. The JSON form also carries the request headers, which is what
/// says whether the proxy leaked the client's address.
fn parse_target_view(body: &str) -> TargetView {
    let mut headers = Vec::new();

    // A JSON object of headers: "Key": ["value"] or "Key": "value".
    if body.contains("\"headers\"") {
        let mut rest = body;
        while let Some(key_start) = rest.find('"') {
            let after = &rest[key_start + 1..];
            let Some(key_end) = after.find('"') else {
                break;
            };
            let key = &after[..key_end];
            rest = &after[key_end + 1..];

            let Some(colon) = rest.find(':') else {
                continue;
            };
            let value_part = rest[colon + 1..].trim_start();
            let value = value_part
                .trim_start_matches('[')
                .split('"')
                .nth(1)
                .unwrap_or_default()
                .to_string();

            if !key.is_empty() && !value.is_empty() {
                headers.push((key.to_ascii_lowercase(), value));
            }
        }
    }

    TargetView {
        exit_ip: extract_exit_ip(body),
        headers,
    }
}

/// Classify what the proxy revealed about the client.
///
/// - transparent: the proxy passed the client's own address on, so the
///   destination can see who it is really talking to.
/// - anonymous: the proxy identified itself but did not leak the address.
/// - elite: the proxy left no trace at all.
fn classify_anonymity(
    view: &TargetView,
    own_exit_ip: Option<std::net::IpAddr>,
) -> Option<crate::proxy::Anonymity> {
    use crate::proxy::Anonymity;

    let forwarded = view
        .headers
        .iter()
        .find(|(name, _)| name == "x-forwarded-for")
        .map(|(_, value)| value.clone());

    if let (Some(forwarded), Some(own)) = (&forwarded, own_exit_ip) {
        if forwarded.contains(&own.to_string()) {
            return Some(Anonymity::Transparent);
        }
    }

    let identifies_itself = view.headers.iter().any(|(name, _)| {
        matches!(
            name.as_str(),
            "via" | "proxy-connection" | "x-forwarded-for" | "x-forwarded-proto" | "forwarded"
        )
    });

    if identifies_itself {
        Some(Anonymity::Anonymous)
    } else {
        Some(Anonymity::Elite)
    }
}

/// Extract the client address from a response body.
///
/// The echo services answer either with a bare address or with a small JSON
/// document, so both are accepted. The address is what proves the request
/// travelled through the proxy, so a body without one is a failure.
fn extract_exit_ip(body: &str) -> Option<std::net::IpAddr> {
    // A JSON document: prefer the "ip" or "origin" field.
    for key in ["\"ip\"", "\"origin\"", "\"query\""] {
        if let Some(value) = body.split(key).nth(1) {
            let value = value.split('"').nth(1).unwrap_or_default();
            if let Ok(ip) = value.trim().parse::<std::net::IpAddr>() {
                return Some(ip);
            }
        }
    }

    // Otherwise, the first thing in the body that parses as an address.
    body.split(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '<')
        .filter(|token| !token.is_empty())
        .find_map(|token| {
            token
                .trim_matches(|c: char| !c.is_ascii_graphic())
                .parse()
                .ok()
        })
}

impl HealthChecker {
    pub fn new(
        concurrency: usize,
        timeout_secs: u64,
        connect_timeout_secs: u64,
        target_urls: Vec<String>,
        mark: Option<u32>,
    ) -> Self {
        Self::with_max_targets_per_proxy(
            concurrency,
            timeout_secs,
            connect_timeout_secs,
            target_urls,
            mark,
            2,
        )
    }

    /// Build a checker with the number of targets a proxy may try.
    pub fn with_max_targets_per_proxy(
        concurrency: usize,
        timeout_secs: u64,
        connect_timeout_secs: u64,
        target_urls: Vec<String>,
        mark: Option<u32>,
        max_targets_per_proxy: usize,
    ) -> Self {
        Self {
            concurrency: Arc::new(Semaphore::new(concurrency)),
            target_urls,
            max_targets_per_proxy: max_targets_per_proxy.max(1),
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            timeout: Duration::from_secs(timeout_secs),
            mark,
            forwarder: crate::forwarder::Forwarder::new(),
            active_target: std::sync::RwLock::new(None),
            target_ttl: Duration::from_secs(300),
        }
    }

    /// The engine's own address, as the target sees it.
    pub fn own_exit_ip(&self) -> Option<std::net::IpAddr> {
        self.active_target
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(|t| t.own_exit_ip))
    }

    /// The target currently in use.
    pub fn active_target_url(&self) -> Option<String> {
        self.active_target
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().map(|t| t.url.clone()))
    }

    /// The targets the probes may use.
    ///
    /// A proxy is only known to work when a target answers through it. The
    /// targets are not equally reachable from everywhere: the network a
    /// proxy sits on may be unable to see the target the engine itself
    /// reaches, and the other way round. A probe that proves nothing — the
    /// target did not answer — therefore moves on to the next target, and the
    /// working set is every proxy that can reach at least one of them.
    pub fn targets(&self, own_exit_ip: Option<std::net::IpAddr>) -> Vec<ActiveTarget> {
        self.target_urls
            .iter()
            .map(|url| {
                let (host, port, path) = parse_target(url);
                ActiveTarget {
                    url: url.clone(),
                    host,
                    port,
                    path,
                    own_exit_ip,
                    verified: own_exit_ip.is_some(),
                    checked_at: std::time::Instant::now(),
                }
            })
            .collect()
    }

    /// Find a target the engine can actually reach.
    ///
    /// The target is requested directly, without a proxy, and the address the
    /// target reports is the engine's own egress address. Two things follow
    /// from that:
    ///
    /// - a proxy that reports the same address is not proxying, and
    /// - when no target answers, nothing is known about the proxies, so the
    ///   batch must not run. Marking fifteen thousand proxies dead because a
    ///   third-party service is unreachable is the worst possible failure
    ///   mode: the pool empties and there is nothing left to proxy with.
    ///
    /// The result is cached, because the check runs on every batch.
    pub async fn resolve_target(&self) -> Option<ActiveTarget> {
        if let Ok(slot) = self.active_target.read() {
            if let Some(target) = slot.as_ref() {
                if target.checked_at.elapsed() < self.target_ttl {
                    return Some(target.clone());
                }
            }
        }

        for url in &self.target_urls {
            let (host, port, path) = parse_target(url);
            match self.request_direct(&host, port, &path).await {
                Ok(Some(exit_ip)) => {
                    let target = ActiveTarget {
                        url: url.clone(),
                        host,
                        port,
                        path,
                        own_exit_ip: Some(exit_ip),
                        verified: true,
                        checked_at: std::time::Instant::now(),
                    };
                    if let Ok(mut slot) = self.active_target.write() {
                        *slot = Some(target.clone());
                    }
                    return Some(target);
                }
                Ok(None) => {
                    tracing::warn!(target = %url, "probe target answered without an address");
                }
                Err(e) => {
                    tracing::warn!(target = %url, error = %e, "probe target is unreachable");
                }
            }
        }

        // No target answered directly. That does not mean the proxies cannot
        // reach one: they are elsewhere, on networks that are often less
        // restricted than the engine's own. The batch still runs against the
        // first configured target — a target that is down from here produces
        // inconclusive verdicts, which change nothing in the pool, rather than
        // a skipped batch or a pool full of wrongly condemned proxies.
        let url = self.target_urls.first()?;

        let (host, port, path) = parse_target(url);
        let target = ActiveTarget {
            url: url.clone(),
            host,
            port,
            path,
            own_exit_ip: None,
            verified: false,
            checked_at: std::time::Instant::now(),
        };
        if let Ok(mut slot) = self.active_target.write() {
            *slot = Some(target.clone());
        }
        Some(target)
    }

    /// Request a target directly, bypassing the TUN.
    async fn request_direct(
        &self,
        host: &str,
        port: u16,
        path: &str,
    ) -> Result<Option<std::net::IpAddr>, String> {
        let timeout = self.timeout;

        let mut conn = tokio::time::timeout(
            timeout,
            self.forwarder.connect_direct(host, port, self.mark),
        )
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| e.to_string())?;

        let authority = if port == 80 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\n\
             User-Agent: proxy-switcher-health/1.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );

        use tokio::io::AsyncWriteExt;
        tokio::time::timeout(timeout, conn.write_all(request.as_bytes()))
            .await
            .map_err(|_| "write timed out".to_string())?
            .map_err(|e| e.to_string())?;

        let mut buf = Vec::new();
        tokio::time::timeout(timeout, conn.read_to_end(&mut buf))
            .await
            .map_err(|_| "read timed out".to_string())?
            .map_err(|e| e.to_string())?;

        Ok(extract_exit_ip(&String::from_utf8_lossy(&buf)))
    }

    /// Probe a single proxy, taking one concurrency permit.
    pub async fn check(&self, proxy: &ProxyInfo) -> HealthResult {
        let _permit = self.concurrency.acquire().await.expect("Semaphore closed");
        let target = self.resolve_target().await;
        let own_exit_ip = target.as_ref().and_then(|target| target.own_exit_ip);
        let targets = self.targets(own_exit_ip);

        self.probe_against(proxy, &targets).await
    }

    /// Probe a proxy against the targets, in order, until one of them answers.
    ///
    /// A verdict that proves something — the proxy works, or the proxy is
    /// broken — ends the probe. A verdict that proves nothing means the
    /// target did not answer, and the next target is given a chance.
    async fn probe_against(&self, proxy: &ProxyInfo, targets: &[ActiveTarget]) -> HealthResult {
        let mut inconclusive: Option<HealthResult> = None;

        for target in targets.iter().take(self.max_targets_per_proxy) {
            let result = check_one(
                proxy,
                &target.host,
                target.port,
                &target.path,
                self.connect_timeout,
                self.timeout,
                self.mark,
                target.own_exit_ip,
                &self.forwarder,
            )
            .await;

            if result.verdict != Verdict::Inconclusive {
                return result;
            }

            inconclusive = Some(result);
        }

        inconclusive.unwrap_or_else(|| {
            HealthResult::inconclusive(&proxy.id, "no probe target is configured")
        })
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

        // The counter is not reset here: a batch is run in rounds, and the
        // progress of the whole batch is what the operator watches. The
        // caller sets the counter to zero when the batch starts.

        // A target that the engine cannot reach directly is not a reason to
        // stop. The proxies live on other networks, which are often less
        // restricted than the engine's own — a target that is blocked here may
        // be perfectly reachable through a proxy. The batch runs anyway; a
        // target that is genuinely down produces inconclusive verdicts, which
        // change nothing in the pool.
        let resolved = self.resolve_target().await;
        let own_exit_ip = resolved.as_ref().and_then(|target| target.own_exit_ip);
        let targets = self.targets(own_exit_ip);

        if targets.is_empty() {
            tracing::error!("no probe target is configured; skipping the batch");
            return CheckCounts::default();
        }

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
            let timeout = self.timeout;
            let connect_timeout = self.connect_timeout;
            let targets = targets.clone();
            let max_targets = self.max_targets_per_proxy;
            let mark = self.mark;
            let forwarder = self.forwarder.clone();
            let tx = tx.clone();
            let checked = checked.clone();

            tokio::spawn(async move {
                let _permit = permit;

                // The targets are tried in order, until one of them answers.
                // A proxy that cannot see the first target may well see the
                // second, and the working set is every proxy that can reach
                // at least one of them.
                let mut inconclusive: Option<HealthResult> = None;

                for target in targets.iter().take(max_targets) {
                    let result = check_one(
                        &proxy,
                        &target.host,
                        target.port,
                        &target.path,
                        connect_timeout,
                        timeout,
                        mark,
                        target.own_exit_ip,
                        &forwarder,
                    )
                    .await;

                    if result.verdict != Verdict::Inconclusive {
                        checked.fetch_add(1, Ordering::Relaxed);
                        let _ = tx.send(result).await;
                        return;
                    }

                    inconclusive = Some(result);
                }

                checked.fetch_add(1, Ordering::Relaxed);
                let _ = tx
                    .send(inconclusive.unwrap_or_else(|| {
                        HealthResult::inconclusive(&proxy.id, "no probe target is configured")
                    }))
                    .await;
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
    use super::*;

    /// The body of an answer that did not come from the target.
    ///
    /// A captive portal, an intercepting middlebox and a proxy that serves its
    /// own page all answer with a status of 200 and a page of their own.
    const INTERCEPTED: &str =
        "<!DOCTYPE html><html lang=\"ru\"><head><title>Blocked</title></head></html>";

    #[test]
    fn an_answer_without_an_address_is_not_the_target() {
        // The target always reports the address it sees.
        assert!(extract_exit_ip("135.136.35.62\n").is_some());
        assert!(extract_exit_ip(r#"{"origin":"135.136.35.62"}"#).is_some());

        // A page of the proxy's own is not an address.
        assert!(extract_exit_ip(INTERCEPTED).is_none());
    }

    #[test]
    fn a_json_answer_carries_the_address_and_the_headers() {
        let view = parse_target_view(
            r#"{"origin":"135.136.35.62","headers":{"X-Forwarded-For":["135.136.35.62"]}}"#,
        );

        assert_eq!(view.exit_ip, Some("135.136.35.62".parse().unwrap()));
        assert!(view
            .headers
            .iter()
            .any(|(name, value)| name == "x-forwarded-for" && value.contains("135.136.35.62")));
    }

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

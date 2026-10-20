//! A local proxy that any program can point at.
//!
//! The engine already knows how to open a tunnel through a proxy, how to tell
//! a working proxy from a dead one, and how to find the next one. This
//! module puts that behind a local port that speaks the HTTP proxy
//! protocol, so a program is given one address and never has to know which
//! public proxy is carrying its traffic.
//!
//! What it accepts:
//!
//! - `CONNECT host:port` — a tunnel, for TLS and for anything else.
//! - an absolute URI in the request line — an ordinary HTTP request.
//!
//! What it does about a dead proxy: the failure is recorded, the proxy steps
//! aside, and the next one is tried. The client only learns about it when
//! every candidate has failed, and then it is told so at once rather than
//! being left waiting.
//!
//! Who is allowed to use it, and how their traffic is carried, is decided by
//! the credential they authenticate with. The credential carries a policy:
//! how the proxy underneath is chosen, which countries the exit address may
//! be in, which protocols may be used. See [`crate::credentials`].
//!
//! Local addresses are never proxied. A program that points at this port and
//! then talks to the console on the same host would otherwise be talking to
//! itself through a proxy, which is a loop that never ends.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::credentials::{CredentialStore, ProxyPolicy, ProxyPolicyRecord, Rotation};
use crate::forwarder::{connect_to_host, Forwarder};
use crate::pool::ProxyPool;
use crate::proxy::connector::base::{Carriage, ConnectOptions, Target};
use crate::proxy::connector::BoxedStream;
use crate::proxy::ProxyInfo;

/// How the local proxy behaves.
#[derive(Debug, Clone)]
pub struct LocalProxyConfig {
    /// Whether the listener runs at all.
    pub enabled: bool,
    /// The address it listens on.
    pub host: String,
    /// The port it listens on.
    pub port: u16,
    /// Whether a client without a credential is let in.
    ///
    /// The credential is what carries the policy, so without one there is
    /// nothing to decide how the traffic is carried. The default policy
    /// applies.
    pub allow_anonymous: bool,
    /// How many proxies are tried before the client is refused.
    pub max_attempts: usize,
    /// The budget for opening one tunnel.
    pub timeout: Duration,
}

impl Default for LocalProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".to_string(),
            port: 9098,
            allow_anonymous: false,
            max_attempts: 3,
            timeout: Duration::from_secs(10),
        }
    }
}

/// What the local proxy has done.
#[derive(Debug, Default)]
pub struct LocalProxyStats {
    /// The connections the clients opened.
    pub connections: AtomicU64,
    /// The tunnels that were established.
    pub tunnels: AtomicU64,
    /// The failures: a proxy that could not carry the connection.
    pub failures: AtomicU64,
    /// The times the proxy was changed because the one in front died.
    pub rotations: AtomicU64,
    /// The clients that were refused.
    pub refused: AtomicU64,
    /// The bytes carried towards the destination.
    pub bytes_out: AtomicU64,
    /// The bytes carried back to the client.
    pub bytes_in: AtomicU64,
}

/// What the local proxy is doing right now.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalProxyState {
    /// Whether the listener is running.
    pub enabled: bool,
    /// The address a program should be given.
    pub address: String,
    /// The proxy in front right now.
    pub current_proxy: Option<String>,
    /// The clients that hold a proxy of their own.
    pub sessions: usize,
    /// The connections the clients opened.
    pub connections: u64,
    /// The tunnels that were established.
    pub tunnels: u64,
    /// The failures.
    pub failures: u64,
    /// The rotations.
    pub rotations: u64,
    /// The clients that were refused.
    pub refused: u64,
    /// The bytes carried towards the destination.
    pub bytes_out: u64,
    /// The bytes carried back to the client.
    pub bytes_in: u64,
}

/// A client that holds a proxy of its own.
#[derive(Debug, Clone)]
struct Session {
    /// The proxy this client is pinned to.
    proxy_id: String,
    /// When the pin runs out.
    expires_at: Option<Instant>,
}

/// The proxy in front, for rotation by time.
#[derive(Debug, Clone)]
struct Slot {
    /// The proxy in front.
    proxy_id: Option<String>,
    /// When it is replaced.
    changes_at: Instant,
}

/// The listener.
pub struct LocalProxy {
    config: LocalProxyConfig,
    pool: Arc<ProxyPool>,
    forwarder: Arc<Forwarder>,
    /// The firewall mark the engine's own sockets carry.
    mark: Option<u32>,
    stats: Arc<LocalProxyStats>,
    /// The clients that hold a proxy of their own.
    sessions: Arc<tokio::sync::Mutex<HashMap<String, Session>>>,
    /// The proxies in front, for rotation by time.
    slots: Arc<tokio::sync::Mutex<HashMap<String, Slot>>>,
    /// The proxies used lately, so a rotation spreads over the pool.
    recent: Arc<tokio::sync::Mutex<VecDeque<String>>>,
    /// The credentials clients authenticate with.
    credentials: Option<Arc<CredentialStore>>,
}

impl LocalProxy {
    /// Build the listener.
    pub fn new(
        config: LocalProxyConfig,
        pool: Arc<ProxyPool>,
        mark: Option<u32>,
        credentials: Option<Arc<CredentialStore>>,
    ) -> Self {
        Self {
            config,
            pool,
            forwarder: Arc::new(Forwarder::new()),
            mark,
            stats: Arc::new(LocalProxyStats::default()),
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            slots: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            recent: Arc::new(tokio::sync::Mutex::new(VecDeque::new())),
            credentials,
        }
    }

    /// The address a program should be given.
    pub fn address(&self) -> String {
        format!("http://{}:{}", self.config.host, self.config.port)
    }

    /// What the proxy is doing.
    pub async fn state(&self) -> LocalProxyState {
        let current = self
            .slots
            .lock()
            .await
            .values()
            .filter_map(|slot| slot.proxy_id.clone())
            .next();

        LocalProxyState {
            enabled: self.config.enabled,
            address: self.address(),
            current_proxy: current,
            sessions: self.sessions.lock().await.len(),
            connections: self.stats.connections.load(Ordering::Relaxed),
            tunnels: self.stats.tunnels.load(Ordering::Relaxed),
            failures: self.stats.failures.load(Ordering::Relaxed),
            rotations: self.stats.rotations.load(Ordering::Relaxed),
            refused: self.stats.refused.load(Ordering::Relaxed),
            bytes_out: self.stats.bytes_out.load(Ordering::Relaxed),
            bytes_in: self.stats.bytes_in.load(Ordering::Relaxed),
        }
    }

    /// Run the listener until the process ends.
    pub async fn serve(self: Arc<Self>) -> Result<()> {
        if !self.config.enabled {
            info!("the local proxy is off");
            return Ok(());
        }

        let listener = tokio::net::TcpListener::bind((self.config.host.as_str(), self.config.port))
            .await
            .with_context(|| {
                format!(
                    "could not bind the local proxy on {}:{}",
                    self.config.host, self.config.port
                )
            })?;

        info!(address = %self.address(), "the local proxy is listening");

        loop {
            let (socket, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    warn!("the local proxy could not accept a connection: {e}");
                    continue;
                }
            };

            let proxy = self.clone();
            tokio::spawn(async move {
                proxy.stats.connections.fetch_add(1, Ordering::Relaxed);
                if let Err(e) = proxy.handle(socket).await {
                    debug!("the local proxy gave up on {peer}: {e}");
                }
            });
        }
    }

    /// Serve one client.
    async fn handle(&self, mut client: tokio::net::TcpStream) -> Result<()> {
        let peer = client
            .peer_addr()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let request = match read_request_head(&mut client).await? {
            Some(request) => request,
            None => return Ok(()),
        };

        let target = match request.target() {
            Some(target) => target,
            None => {
                // Not a proxy request: the client is talking to this port as
                // if it were an ordinary server. The answer says what this is.
                let body = "This port is an HTTP proxy. Point the program at it as a proxy.\n";
                client
                    .write_all(
                        format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await?;
                client.shutdown().await?;
                return Ok(());
            }
        };

        // The credential decides how the traffic is carried. The name in it
        // tells the clients apart: every program on this host connects from
        // the same address, so the address alone cannot.
        let credentials = request.credentials();
        let identity = credentials
            .as_ref()
            .map(|(username, _)| username.clone())
            .unwrap_or_else(|| peer.clone());

        let policy = match self.policy_for(credentials.as_ref()).await {
            Some(policy) => policy,
            None => {
                self.stats.refused.fetch_add(1, Ordering::Relaxed);
                return self
                    .refuse(
                        &mut client,
                        407,
                        "Proxy Authentication Required",
                        "This proxy needs a credential. Use http://name:password@host:port.",
                    )
                    .await;
            }
        };

        // A local destination is not proxied. A program that points at this
        // port and then talks to the console on the same host would be
        // talking to itself through a proxy.
        if is_local_target(&target) {
            return self.serve_direct(&mut client, &request, &target).await;
        }

        let proxy = self.proxy_for(&identity, &policy).await;

        match request.method.as_str() {
            "CONNECT" => self.serve_tunnel(&mut client, &target, proxy).await,
            _ => {
                self.serve_request(&mut client, &request, &target, proxy)
                    .await
            }
        }
    }

    /// The policy a client's traffic is carried under.
    async fn policy_for(
        &self,
        credentials: Option<&(String, String)>,
    ) -> Option<ProxyPolicyRecord> {
        let Some((username, password)) = credentials else {
            if !self.config.allow_anonymous {
                return None;
            }
            return Some(ProxyPolicyRecord {
                name: "default".to_string(),
                settings: ProxyPolicy::default(),
                builtin: true,
            });
        };

        let store = self.credentials.as_ref()?;
        match store.verify(username, password).await {
            Some(policy) => Some(policy),
            None => {
                warn!("the local proxy refused the credential {username}");
                None
            }
        }
    }

    /// The proxy a client's traffic is carried by.
    ///
    /// How it is chosen is the policy's decision: a sticky session keeps the
    /// proxy it was given, a rotation by time shares one proxy between the
    /// clients of a policy, and a rotation per request gives every request a
    /// proxy of its own.
    async fn proxy_for(&self, identity: &str, policy: &ProxyPolicyRecord) -> Option<ProxyInfo> {
        let settings = &policy.settings;

        match settings.rotation {
            Rotation::Sticky => self.sticky_proxy(identity, settings).await,
            Rotation::Time => self.timed_proxy(&policy.name, settings).await,
            Rotation::PerRequest => self.per_request_proxy(settings).await,
        }
    }

    /// The proxy a client is pinned to.
    async fn sticky_proxy(&self, identity: &str, policy: &ProxyPolicy) -> Option<ProxyInfo> {
        let mut sessions = self.sessions.lock().await;

        // An expired pin is dropped, and the client is given a new one.
        if let Some(session) = sessions.get(identity) {
            let expired = session.expires_at.is_some_and(|at| Instant::now() >= at);
            if !expired {
                if let Some(proxy) = self.pool.get(&session.proxy_id).await {
                    if policy.allows(&proxy) {
                        return Some(proxy);
                    }
                }
            }
            sessions.remove(identity);
        }

        // A client that is not pinned is given a proxy that no other client
        // holds, so the sessions do not all share one exit address.
        let pinned: HashSet<String> = sessions
            .values()
            .map(|session| session.proxy_id.clone())
            .collect();

        let Some(proxy) = self.pool.best_matching(&pinned, policy).await else {
            return self.pool.active().await.filter(|p| policy.allows(p));
        };

        tracing::debug!(
            id = %proxy.id,
            protocol = %proxy.protocol,
            score = proxy.score,
            last_success = ?proxy.last_success,
            "the local proxy pinned a client to a proxy"
        );

        let expires_at = if policy.sticky_secs > 0 {
            Some(Instant::now() + Duration::from_secs(policy.sticky_secs))
        } else {
            None
        };

        sessions.insert(
            identity.to_string(),
            Session {
                proxy_id: proxy.id.clone(),
                expires_at,
            },
        );

        Some(proxy)
    }

    /// The proxy in front for a policy, replaced on a schedule.
    async fn timed_proxy(&self, policy_name: &str, policy: &ProxyPolicy) -> Option<ProxyInfo> {
        let interval = Duration::from_secs(policy.rotate_interval_secs.max(1));
        let mut slots = self.slots.lock().await;

        let now = Instant::now();
        let slot = slots.get(policy_name);

        if let Some(slot) = slot {
            if now < slot.changes_at {
                if let Some(proxy) = self
                    .pool
                    .get(&slot.proxy_id.clone().unwrap_or_default())
                    .await
                {
                    if policy.allows(&proxy) {
                        return Some(proxy);
                    }
                }
            }
        }

        // The window is over: another proxy takes the place.
        let avoid = self.recently_used().await;
        let Some(proxy) = self.pool.best_matching(&avoid, policy).await else {
            return self.pool.active().await.filter(|p| policy.allows(p));
        };

        slots.insert(
            policy_name.to_string(),
            Slot {
                proxy_id: Some(proxy.id.clone()),
                changes_at: now + interval,
            },
        );

        self.remember(&proxy.id).await;
        Some(proxy)
    }

    /// A proxy of this request's own.
    async fn per_request_proxy(&self, policy: &ProxyPolicy) -> Option<ProxyInfo> {
        let avoid = self.recently_used().await;
        let Some(proxy) = self.pool.best_matching(&avoid, policy).await else {
            return self.pool.active().await.filter(|p| policy.allows(p));
        };

        self.remember(&proxy.id).await;
        Some(proxy)
    }

    /// The proxies used lately.
    ///
    /// The traffic is spread over the pool: a proxy that was just used is
    /// not used again straight away, so the exit address changes.
    async fn recently_used(&self) -> HashSet<String> {
        self.recent.lock().await.iter().cloned().collect()
    }

    /// Remember that a proxy was used.
    async fn remember(&self, id: &str) {
        let mut recent = self.recent.lock().await;
        recent.push_back(id.to_string());
        while recent.len() > 16 {
            recent.pop_front();
        }
    }

    /// Carry a tunnel: `CONNECT host:port`.
    async fn serve_tunnel(
        &self,
        client: &mut tokio::net::TcpStream,
        target: &Target,
        proxy: Option<ProxyInfo>,
    ) -> Result<()> {
        let Some(proxy) = proxy else {
            return self
                .refuse(
                    client,
                    502,
                    "Bad Gateway",
                    "no proxy matches the policy of this credential",
                )
                .await;
        };

        let upstream = match self.open_tunnel(target, proxy).await {
            Ok(upstream) => upstream,
            Err(e) => {
                return self
                    .refuse(
                        client,
                        502,
                        "Bad Gateway",
                        &format!("no proxy could carry the connection: {e}"),
                    )
                    .await;
            }
        };

        client
            .write_all(
                b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: proxy-switcher\r\n\r\n",
            )
            .await
            .context("answering the CONNECT")?;
        client.flush().await?;

        self.stats.tunnels.fetch_add(1, Ordering::Relaxed);

        self.pipe(client, upstream).await
    }

    /// Carry an ordinary request: the request line holds an absolute URI.
    async fn serve_request(
        &self,
        client: &mut tokio::net::TcpStream,
        request: &RequestHead,
        target: &Target,
        proxy: Option<ProxyInfo>,
    ) -> Result<()> {
        let Some(proxy) = proxy else {
            return self
                .refuse(
                    client,
                    502,
                    "Bad Gateway",
                    "no proxy matches the policy of this credential",
                )
                .await;
        };

        // The proxy is asked for a tunnel when it can carry one.
        if proxy.supports_tunnel {
            match self.open_tunnel(target, proxy.clone()).await {
                Ok(mut upstream) => {
                    upstream.write_all(&request.raw).await?;
                    upstream.flush().await?;
                    self.stats.tunnels.fetch_add(1, Ordering::Relaxed);
                    return self.pipe(client, upstream).await;
                }
                Err(e) => {
                    // A proxy that refuses CONNECT may still forward plain
                    // requests, and a large part of the public proxy
                    // population does exactly that. The request is not lost
                    // over a tunnel the proxy would not open.
                    if !Carriage::is_plain_http(target.port) {
                        return self
                            .refuse(
                                client,
                                502,
                                "Bad Gateway",
                                &format!("no proxy could carry the connection: {e}"),
                            )
                            .await;
                    }
                }
            }
        }

        if !Carriage::is_plain_http(target.port) {
            return self
                .refuse(
                    client,
                    502,
                    "Bad Gateway",
                    "the proxy in front cannot carry a tunnel to this port",
                )
                .await;
        }

        // Plain forwarding: the request goes to the proxy with an absolute
        // URI, and the answer comes back the same way.
        let options = ConnectOptions {
            mark: self.mark,
            timeout: self.config.timeout,
            credentials: None,
        };

        let mut upstream = match self.connect_plain(&proxy, &options).await {
            Ok(upstream) => upstream,
            Err(e) => {
                self.record_failure(&proxy).await;
                return self
                    .refuse(
                        client,
                        502,
                        "Bad Gateway",
                        &format!("the proxy failed: {e}"),
                    )
                    .await;
            }
        };

        upstream.write_all(&request.raw).await?;
        upstream.flush().await?;

        let mut response = Vec::new();
        upstream.read_to_end(&mut response).await?;
        client.write_all(&response).await?;
        client.shutdown().await?;

        self.stats
            .bytes_out
            .fetch_add(request.raw.len() as u64, Ordering::Relaxed);
        self.stats
            .bytes_in
            .fetch_add(response.len() as u64, Ordering::Relaxed);

        Ok(())
    }

    /// Carry a request to a local address, without a proxy.
    async fn serve_direct(
        &self,
        client: &mut tokio::net::TcpStream,
        request: &RequestHead,
        target: &Target,
    ) -> Result<()> {
        let stream = match tokio::time::timeout(
            self.config.timeout,
            connect_to_host(&target.host, target.port, self.mark),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                return self
                    .refuse(
                        client,
                        502,
                        "Bad Gateway",
                        &format!("the host refused: {e}"),
                    )
                    .await;
            }
            Err(_) => {
                return self
                    .refuse(
                        client,
                        504,
                        "Gateway Timeout",
                        "the host did not answer in time",
                    )
                    .await;
            }
        };

        let mut upstream: BoxedStream = Box::new(stream);
        upstream.write_all(&request.raw).await?;
        upstream.flush().await?;

        self.pipe(client, upstream).await
    }

    /// Open a tunnel through a proxy, moving on when the proxy is dead.
    async fn open_tunnel(&self, target: &Target, proxy: ProxyInfo) -> Result<BoxedStream> {
        let mut avoided: HashSet<String> = HashSet::new();
        let mut last_error: Option<anyhow::Error> = None;
        let mut current = proxy;

        for _ in 0..self.config.max_attempts.max(1) {
            let options = ConnectOptions {
                mark: self.mark,
                timeout: self.config.timeout,
                credentials: None,
            };

            match self
                .forwarder
                .connect_to_with(&current, target, &options)
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!(
                        "the proxy {} could not carry the connection: {e}",
                        current.id
                    );
                    self.stats.failures.fetch_add(1, Ordering::Relaxed);
                    self.record_failure(&current).await;
                    avoided.insert(current.id.clone());

                    // A server that does not answer does not answer on its
                    // other ports either, and the pool is full of the same
                    // endpoint published under many names. The whole host is
                    // left out, so the next attempt is a different server.
                    avoided.insert(format!("host:{}", current.host));
                    last_error = Some(e);

                    // The next candidate is chosen right away, so the client
                    // does not wait for the pool to catch up.
                    //
                    // The proxy the device routes through is not touched: the
                    // local proxy has its own rotation, and the two must not
                    // fight over the same selection.
                    match self.pool.best_avoiding(&avoided).await {
                        Some(next) => {
                            self.stats.rotations.fetch_add(1, Ordering::Relaxed);
                            current = next;
                        }
                        None => break,
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("no proxy could carry the connection")))
    }

    /// Open a plain-forwarding connection to an HTTP proxy.
    async fn connect_plain(
        &self,
        proxy: &ProxyInfo,
        options: &ConnectOptions,
    ) -> Result<BoxedStream> {
        let addrs = crate::net::resolve_host_port(&proxy.host, proxy.port).await?;
        let stream = crate::net::connect_first(&addrs, options.mark).await?;
        Ok(Box::new(stream))
    }

    /// Record a failure against the proxy.
    async fn record_failure(&self, proxy: &ProxyInfo) {
        self.pool.report_failure(&proxy.id).await;

        // A client pinned to a dead proxy is released, so the next connection
        // is carried by another one.
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, session| session.proxy_id != proxy.id);

        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if slot.proxy_id.as_deref() == Some(proxy.id.as_str()) {
                slot.proxy_id = None;
            }
        }
    }

    /// Carry bytes in both directions until one side stops.
    async fn pipe(
        &self,
        client: &mut tokio::net::TcpStream,
        mut upstream: BoxedStream,
    ) -> Result<()> {
        // A connection that has been idle for a long time is finished. The
        // client is gone or the target is silent; either way the tunnel has
        // served its purpose.
        let idle = self.config.timeout * 60;

        match tokio::time::timeout(idle, tokio::io::copy_bidirectional(client, &mut upstream)).await
        {
            Ok(result) => {
                let (from_client, from_upstream) = result?;
                self.stats
                    .bytes_out
                    .fetch_add(from_client, Ordering::Relaxed);
                self.stats
                    .bytes_in
                    .fetch_add(from_upstream, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => {
                let _ = client.shutdown().await;
                Ok(())
            }
        }
    }

    /// Tell the client that the request cannot be carried.
    async fn refuse(
        &self,
        client: &mut tokio::net::TcpStream,
        status: u16,
        reason: &str,
        detail: &str,
    ) -> Result<()> {
        let body = format!("{detail}\n");
        client
            .write_all(
                format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
                     Proxy-Authenticate: Basic realm=\"proxy-switcher\"\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await?;
        client.shutdown().await?;
        Ok(())
    }
}

/// The head of an HTTP request.
struct RequestHead {
    method: String,
    /// The request line and the headers, as they arrived.
    raw: Vec<u8>,
}

impl RequestHead {
    /// The target the request is for.
    ///
    /// `CONNECT` gives an authority, an absolute URI gives a full address, and
    /// an origin-form request line gives nothing: that is not a proxy
    /// request.
    fn target(&self) -> Option<Target> {
        let line = self.raw.split(|b| *b == b'\n').next()?;
        let line = String::from_utf8_lossy(line);
        let mut parts = line.split_whitespace();

        let method = parts.next()?.to_string();
        let request_target = parts.next()?;

        if method.eq_ignore_ascii_case("CONNECT") {
            return parse_authority(request_target);
        }

        let (host, _path) = split_absolute_uri(request_target)?;

        match host.rsplit_once(':') {
            Some((name, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
                Some(Target {
                    host: name.to_string(),
                    port: port.parse().ok()?,
                })
            }
            _ => Some(Target {
                host: host.clone(),
                port: 80,
            }),
        }
    }

    /// The name and the password the client authenticates with.
    fn credentials(&self) -> Option<(String, String)> {
        let head = String::from_utf8_lossy(&self.raw);
        let header = head.lines().find(|line| {
            line.split_once(':')
                .map(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
                .unwrap_or(false)
        })?;

        let value = header.split_once(':')?.1.trim();

        // The value is "Basic <encoded>". The encoding is base64, and the
        // name and the password are separated by a colon.
        let encoded = value.split_whitespace().nth(1)?;
        let decoded = base64_decode(encoded)?;
        let decoded = String::from_utf8_lossy(&decoded).to_string();

        let (name, password) = decoded.split_once(':')?;
        Some((name.to_string(), password.to_string()))
    }
}

/// Split an absolute URI into its host and path.
fn split_absolute_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri
        .strip_prefix("http://")
        .or_else(|| uri.strip_prefix("https://"))?;

    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };

    Some((authority.to_string(), path.to_string()))
}

/// Parse an authority into a host and a port.
fn parse_authority(authority: &str) -> Option<Target> {
    if let Some((host, port)) = authority.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return Some(Target {
                host: host.to_string(),
                port: port.parse().ok()?,
            });
        }
    }

    Some(Target {
        host: authority.to_string(),
        port: 443,
    })
}

/// Decode base64, without a dependency.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;

    for byte in input.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = TABLE.iter().position(|c| *c == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }

    Some(out)
}

/// Whether the target is on this host or on the local network.
fn is_local_target(target: &Target) -> bool {
    match target.host.parse::<std::net::IpAddr>() {
        Ok(address) => crate::routing::is_local(address),
        // A name cannot be told apart from a local one without resolving it,
        // and resolving it here would be a request the proxy makes on its own
        // account. A name is left to the tunnel, which resolves it where the
        // proxy is.
        Err(_) => target.host == "localhost" || target.host.ends_with(".localhost"),
    }
}

/// Read the head of a request: the request line and the headers.
async fn read_request_head(client: &mut tokio::net::TcpStream) -> Result<Option<RequestHead>> {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let read = client.read(&mut byte).await?;
        if read == 0 {
            break;
        }
        raw.push(byte[0]);

        // The head ends with a blank line.
        if raw.len() >= 4 && raw[raw.len() - 4..] == *b"\r\n\r\n" {
            break;
        }

        // A head that grows past this is not a head.
        if raw.len() > 64 * 1024 {
            bail!("the request head is too large");
        }
    }

    if raw.is_empty() {
        return Ok(None);
    }

    let line = String::from_utf8_lossy(&raw);
    let method = line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();

    Ok(Some(RequestHead { method, raw }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> RequestHead {
        RequestHead {
            method: raw.split_whitespace().next().unwrap_or("").to_string(),
            raw: raw.as_bytes().to_vec(),
        }
    }

    #[test]
    fn a_connect_names_its_target() {
        let request = head("CONNECT example.com:443 HTTP/1.1\r\n\r\n");
        let target = request.target().expect("a target");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 443);
    }

    #[test]
    fn a_connect_without_a_port_means_https() {
        let request = head("CONNECT example.com HTTP/1.1\r\n\r\n");
        let target = request.target().expect("a target");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 443);
    }

    #[test]
    fn an_absolute_uri_names_its_target() {
        let request = head("GET http://example.com/path?q=1 HTTP/1.1\r\n\r\n");
        let target = request.target().expect("a target");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 80);
    }

    #[test]
    fn an_absolute_uri_may_name_a_port() {
        let request = head("GET http://example.com:8080/path HTTP/1.1\r\n\r\n");
        let target = request.target().expect("a target");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 8080);
    }

    #[test]
    fn an_origin_form_request_is_not_a_proxy_request() {
        let request = head("GET /path HTTP/1.1\r\n\r\n");
        assert!(request.target().is_none());
    }

    #[test]
    fn the_credentials_tell_the_clients_apart() {
        // "session-1:secret" in base64.
        let request = head(
            "GET http://example.com/ HTTP/1.1\r\nProxy-Authorization: Basic c2Vzc2lvbi0xOnNlY3JldA==\r\n\r\n",
        );
        let (name, password) = request.credentials().expect("credentials");
        assert_eq!(name, "session-1");
        assert_eq!(password, "secret");
    }

    #[test]
    fn a_client_without_a_name_has_no_credentials() {
        let request = head("GET http://example.com/ HTTP/1.1\r\n\r\n");
        assert!(request.credentials().is_none());
    }

    #[test]
    fn local_addresses_are_never_proxied() {
        let local = Target {
            host: "127.0.0.1".to_string(),
            port: 8088,
        };
        assert!(is_local_target(&local));

        let remote = Target {
            host: "example.com".to_string(),
            port: 80,
        };
        assert!(!is_local_target(&remote));
    }
}

pub mod connector;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInfo {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub protocol: ProxyProtocol,
    pub anonymity: Anonymity,
    pub latency_ms: Option<u64>,
    pub country: Option<String>,
    /// When the proxy was last probed, successfully or not.
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
    /// When the proxy last actually worked. Drives staleness eviction.
    pub last_success: Option<chrono::DateTime<chrono::Utc>>,
    /// When the proxy entered the pool.
    ///
    /// The unverified TTL is measured from here, not from `last_checked`:
    /// a failed probe refreshes `last_checked`, so measuring from there meant
    /// the age reset on every attempt and a proxy that never worked was never
    /// evicted — the pool only ever grew.
    pub first_seen: chrono::DateTime<chrono::Utc>,
    /// When this proxy should next be probed.
    ///
    /// Explicit scheduling state rather than something derived from the
    /// failure count: the pool re-probes only what is due, so a proxy that has
    /// been dead for a week is looked at a few times a day instead of on every
    /// cycle. Without this the batch size was the whole unverified pool, and
    /// it grew with every scrape.
    pub next_probe_at: chrono::DateTime<chrono::Utc>,
    /// The address traffic left from, as the target reported it.
    ///
    /// Two different proxy entries often share one exit address — the same
    /// endpoint published under many names. Seeing the address makes that
    /// visible.
    #[serde(default)]
    pub exit_ip: Option<String>,
    /// Consecutive failed probes. A single failure never evicts a proxy.
    pub consecutive_failures: u32,
    pub score: f64,
}

impl ProxyInfo {
    /// A freshly discovered proxy: known to exist, proven to work by nothing.
    pub fn discovered(mut self) -> Self {
        let now = chrono::Utc::now();
        self.last_checked = None;
        self.last_success = None;
        self.consecutive_failures = 0;
        self.score = 0.0;
        self.first_seen = now;
        // Due immediately: a new proxy should be probed as soon as possible.
        self.next_probe_at = now;
        self
    }
}

/// Serialised in lowercase: the API contract, the generated SDK and the web UI
/// all use `"http"`/`"socks5"`, and serde's default would emit `"Http"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks4,
    Socks5,
}

impl std::fmt::Display for ProxyProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.scheme())
    }
}

impl ProxyProtocol {
    pub fn scheme(&self) -> &'static str {
        match self {
            ProxyProtocol::Http => "http",
            ProxyProtocol::Https => "https",
            ProxyProtocol::Socks4 => "socks4",
            ProxyProtocol::Socks5 => "socks5",
        }
    }
}

impl std::str::FromStr for ProxyProtocol {
    type Err = String;

    /// Parses the same lowercase names serde writes, so a value stored by one
    /// release is readable by the next.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "http" => Ok(ProxyProtocol::Http),
            "https" => Ok(ProxyProtocol::Https),
            "socks4" => Ok(ProxyProtocol::Socks4),
            "socks5" => Ok(ProxyProtocol::Socks5),
            other => Err(format!("unknown protocol: {other}")),
        }
    }
}

/// Serialised in lowercase to match `ProxyProtocol` and the API contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Anonymity {
    Transparent,
    Anonymous,
    Elite,
    Unknown,
}

impl std::str::FromStr for Anonymity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "transparent" => Ok(Anonymity::Transparent),
            "anonymous" => Ok(Anonymity::Anonymous),
            "elite" => Ok(Anonymity::Elite),
            "unknown" => Ok(Anonymity::Unknown),
            other => Err(format!("unknown anonymity: {other}")),
        }
    }
}

impl ProxyInfo {
    pub fn proxy_url(&self) -> String {
        let scheme = match self.protocol {
            ProxyProtocol::Http => "http",
            ProxyProtocol::Https => "https",
            ProxyProtocol::Socks4 => "socks4",
            ProxyProtocol::Socks5 => "socks5",
        };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

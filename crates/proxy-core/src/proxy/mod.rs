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
    /// Consecutive failed probes. A single failure never evicts a proxy.
    pub consecutive_failures: u32,
    pub score: f64,
}

impl ProxyInfo {
    /// A freshly discovered proxy: known to exist, proven to work by nothing.
    pub fn discovered(mut self) -> Self {
        self.last_checked = None;
        self.last_success = None;
        self.consecutive_failures = 0;
        self.score = 0.0;
        self.first_seen = chrono::Utc::now();
        self
    }
}

/// Serialised in lowercase: the API contract, the generated SDK and the web UI
/// all use `"http"`/`"socks5"`, and serde's default would emit `"Http"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks4,
    Socks5,
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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

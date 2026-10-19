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

/// Serialised in lowercase to match `ProxyProtocol` and the API contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Anonymity {
    Transparent,
    Anonymous,
    Elite,
    Unknown,
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

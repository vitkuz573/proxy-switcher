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
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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

use std::time::Duration;

/// Where a connection should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// The authority form used in an HTTP request line and a CONNECT.
    pub fn authority(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Credentials for a proxy that requires them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}

/// How a connection should be opened.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Firewall mark for the socket.
    ///
    /// The engine's own traffic carries one so it bypasses the TUN. A
    /// connection to the proxy that is itself routed through a proxy is a
    /// loop.
    pub mark: Option<u32>,
    /// Budget for establishing the tunnel.
    pub timeout: Duration,
    /// Credentials, for a proxy that asks for them.
    pub credentials: Option<Credentials>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            mark: None,
            timeout: Duration::from_secs(5),
            credentials: None,
        }
    }
}

/// How the traffic is being carried.
///
/// A plain HTTP proxy can carry a request in two ways, and which one is
/// allowed depends on the traffic: a `CONNECT` tunnel carries anything, while
/// plain forwarding can only carry an HTTP request and needs the request line
/// rewritten to an absolute URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carriage {
    /// Any byte stream, in both directions.
    Stream,
    /// One HTTP request, in one direction.
    HttpRequest,
}

impl Carriage {
    /// Whether the target is plain HTTP.
    ///
    /// Only plain HTTP can be carried by a proxy that does not tunnel.
    pub fn is_plain_http(port: u16) -> bool {
        port == 80
    }
}

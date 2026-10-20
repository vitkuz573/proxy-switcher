use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::proxy::connector::base::{ConnectOptions, Target};
use crate::proxy::connector::{BoxedStream, ConnectorRegistry};
use crate::proxy::{ProxyInfo, ProxyProtocol};

/// A stream carrying traffic, whatever the protocol underneath.
pub type ForwardConnection = BoxedStream;

/// Opens streams to targets, directly or through a proxy.
///
/// The protocols live in `proxy::connector`, each behind the `ProxyConnector`
/// trait. This type is the single place that knows about the registry, so a
/// new protocol is one module and one registration rather than another branch
/// in every caller.
#[derive(Clone)]
pub struct Forwarder {
    connectors: std::sync::Arc<ConnectorRegistry>,
}

impl Default for Forwarder {
    fn default() -> Self {
        Self::with_connectors(ConnectorRegistry::with_defaults())
    }
}

impl Forwarder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A forwarder with a custom connector table.
    pub fn with_connectors(connectors: ConnectorRegistry) -> Self {
        Self {
            connectors: std::sync::Arc::new(connectors),
        }
    }

    /// The protocols this forwarder can open.
    pub fn protocols(&self) -> Vec<ProxyProtocol> {
        self.connectors.protocols()
    }

    /// Connect straight to the target.
    pub async fn connect_direct(
        &self,
        target: &str,
        port: u16,
        mark: Option<u32>,
    ) -> Result<ForwardConnection> {
        // The direct path has a budget too: a destination that accepts the
        // connection and then says nothing must not hold the caller.
        match tokio::time::timeout(DIRECT_TIMEOUT, connect_to_host(target, port, mark)).await {
            Ok(result) => {
                let stream = result?;
                Ok(Box::new(stream))
            }
            Err(_) => anyhow::bail!(
                "{target}:{port} did not answer within {}s",
                DIRECT_TIMEOUT.as_secs()
            ),
        }
    }

    /// Connect to the target through the proxy.
    pub async fn connect_to(
        &self,
        proxy: &ProxyInfo,
        target: &str,
        port: u16,
        mark: Option<u32>,
    ) -> Result<ForwardConnection> {
        self.connect_to_with(
            proxy,
            &Target::new(target, port),
            &ConnectOptions {
                mark,
                ..ConnectOptions::default()
            },
        )
        .await
    }

    /// Connect to the target through the proxy, with full options.
    pub async fn connect_to_with(
        &self,
        proxy: &ProxyInfo,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<ForwardConnection> {
        let connector = self
            .connectors
            .get(proxy.protocol)
            .ok_or_else(|| anyhow::anyhow!("no connector for the {} protocol", proxy.protocol))?;

        if !connector.supports(target) {
            anyhow::bail!(
                "the {} protocol cannot carry a connection to {}:{}",
                proxy.protocol,
                target.host,
                target.port
            );
        }

        // The budget is the connector's business, and it is enforced here so
        // that every protocol is covered: a proxy that accepts the
        // connection and then says nothing must not hold the caller for
        // longer than the budget allows.
        match tokio::time::timeout(options.timeout, connector.connect(proxy, target, options)).await
        {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "the {} proxy did not answer within {}s",
                proxy.protocol,
                options.timeout.as_secs()
            ),
        }
    }
}

/// How long the direct path may take to open a connection.
const DIRECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connect to a host, skipping the resolver for an address literal.
///
/// The resolver is not free: for a numeric address it is pure overhead, and
/// asking it to resolve a literal with a service of `0` fails outright.
pub async fn connect_to_host(
    host: &str,
    port: u16,
    mark: Option<u32>,
) -> Result<tokio::net::TcpStream> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(crate::net::connect_tcp(std::net::SocketAddr::new(ip, port), mark).await?);
    }

    let addrs = crate::net::resolve(host, port).await?;
    if addrs.is_empty() {
        anyhow::bail!("no address for {host}:{port}");
    }

    Ok(crate::net::connect_first(&addrs, mark).await?)
}

/// Read an HTTP response's status line from a stream.
pub async fn read_status_line<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    // The status line ends at the first line feed.
    while buf.len() < 4096 {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buf);
    Ok(text.lines().next().unwrap_or_default().trim().to_string())
}

/// Write a complete HTTP request to a stream.
pub async fn write_request<S: AsyncWrite + Unpin>(stream: &mut S, request: &str) -> Result<()> {
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

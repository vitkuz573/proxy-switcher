use crate::proxy::{ProxyInfo, ProxyProtocol};
use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio_socks::tcp::Socks5Stream;

pub enum ForwardConnection {
    Direct(tokio::net::TcpStream),
    HttpConnect(tokio::net::TcpStream),
    Socks5(Socks5Stream<tokio::net::TcpStream>),
    Socks4(tokio_socks::tcp::Socks4Stream<tokio::net::TcpStream>),
}

impl AsyncRead for ForwardConnection {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ForwardConnection::Direct(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ForwardConnection::HttpConnect(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ForwardConnection::Socks5(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ForwardConnection::Socks4(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl ForwardConnection {
    /// Non-blocking read. Returns `WouldBlock` if no data available.
    pub fn try_read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ForwardConnection::Direct(s) => s.try_read(buf),
            ForwardConnection::HttpConnect(s) => s.try_read(buf),
            ForwardConnection::Socks5(s) => {
                use std::ops::Deref;
                s.deref().try_read(buf)
            }
            ForwardConnection::Socks4(s) => {
                use std::ops::Deref;
                s.deref().try_read(buf)
            }
        }
    }
}

impl AsyncWrite for ForwardConnection {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ForwardConnection::Direct(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ForwardConnection::HttpConnect(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ForwardConnection::Socks5(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ForwardConnection::Socks4(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ForwardConnection::Direct(s) => std::pin::Pin::new(s).poll_flush(cx),
            ForwardConnection::HttpConnect(s) => std::pin::Pin::new(s).poll_flush(cx),
            ForwardConnection::Socks5(s) => std::pin::Pin::new(s).poll_flush(cx),
            ForwardConnection::Socks4(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ForwardConnection::Direct(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ForwardConnection::HttpConnect(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ForwardConnection::Socks5(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ForwardConnection::Socks4(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub struct Forwarder;

impl Forwarder {
    /// Connect straight to the target.
    ///
    /// `mark` sets a firewall mark on the socket, which makes the connection
    /// bypass the TUN's routing table. The engine's own traffic always passes
    /// it: a connection to the proxy under test that is itself routed through
    /// a proxy is a loop.
    pub async fn connect_direct(
        target: &str,
        port: u16,
        mark: Option<u32>,
    ) -> Result<ForwardConnection> {
        let stream = connect_to_host(target, port, mark).await?;
        Ok(ForwardConnection::Direct(stream))
    }

    /// Connect through a proxy to a target.
    ///
    /// The socket to the proxy is created here rather than by the SOCKS
    /// libraries, so it can carry the bypass mark.
    pub async fn connect_to(
        proxy: &ProxyInfo,
        target: &str,
        port: u16,
        mark: Option<u32>,
    ) -> Result<ForwardConnection> {
        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);

        match proxy.protocol {
            ProxyProtocol::Socks5 => {
                let socket = connect_to_proxy(&proxy_addr, mark).await?;
                let stream =
                    Socks5Stream::connect_with_socket(socket, format!("{target}:{port}").as_str())
                        .await?;
                Ok(ForwardConnection::Socks5(stream))
            }
            ProxyProtocol::Socks4 => {
                let socket = connect_to_proxy(&proxy_addr, mark).await?;
                let stream = tokio_socks::tcp::Socks4Stream::connect_with_socket(
                    socket,
                    format!("{target}:{port}").as_str(),
                )
                .await?;
                Ok(ForwardConnection::Socks4(stream))
            }
            ProxyProtocol::Http | ProxyProtocol::Https => {
                let mut s = connect_to_proxy(&proxy_addr, mark).await?;
                let connect =
                    format!("CONNECT {target}:{port} HTTP/1.1\r\nHost: {target}:{port}\r\n\r\n");
                use tokio::io::AsyncWriteExt;
                s.write_all(connect.as_bytes()).await?;

                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await?;
                let response = String::from_utf8_lossy(&buf[..n]);
                let alive =
                    response.contains("200 Connection established") || response.contains("200 OK");
                if !alive {
                    anyhow::bail!(
                        "HTTP CONNECT failed: {}",
                        response.lines().next().unwrap_or("unknown")
                    );
                }
                Ok(ForwardConnection::HttpConnect(s))
            }
        }
    }
}

/// Open the connection to the proxy itself, carrying the bypass mark.
async fn connect_to_proxy(proxy_addr: &str, mark: Option<u32>) -> Result<tokio::net::TcpStream> {
    let (host, port) = split_addr(proxy_addr)?;
    connect_to_host(&host, port, mark).await
}

/// Split a `host:port` string.
fn split_addr(addr: &str) -> Result<(String, u16)> {
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("malformed address: {addr}"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("malformed port in {addr}"))?;
    Ok((host.to_string(), port))
}

/// Connect to a host, skipping the resolver for an address literal.
///
/// The resolver is not free: for a numeric address it is pure overhead, and
/// asking it to resolve a literal with a service of `0` fails outright.
async fn connect_to_host(
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

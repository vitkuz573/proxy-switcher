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
    pub async fn connect_direct(target: &str, port: u16) -> Result<ForwardConnection> {
        let stream = tokio::net::TcpStream::connect(format!("{target}:{port}")).await?;
        Ok(ForwardConnection::Direct(stream))
    }

    pub async fn connect_to(proxy: &ProxyInfo, target: &str, port: u16) -> Result<ForwardConnection> {
        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);

        match proxy.protocol {
            ProxyProtocol::Socks5 => {
                let stream = Socks5Stream::connect(proxy_addr.as_str(), format!("{target}:{port}").as_str()).await?;
                Ok(ForwardConnection::Socks5(stream))
            }
            ProxyProtocol::Socks4 => {
                let stream = tokio_socks::tcp::Socks4Stream::connect(
                    proxy_addr.as_str(),
                    format!("{target}:{port}").as_str(),
                )
                .await?;
                Ok(ForwardConnection::Socks4(stream))
            }
            ProxyProtocol::Http | ProxyProtocol::Https => {
                let stream = tokio::net::TcpStream::connect(proxy_addr).await?;
                let connect = format!(
                    "CONNECT {target}:{port} HTTP/1.1\r\nHost: {target}:{port}\r\n\r\n"
                );
                use tokio::io::AsyncWriteExt;
                let mut s = stream;
                s.write_all(connect.as_bytes()).await?;

                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await?;
                let response = String::from_utf8_lossy(&buf[..n]);
                let alive = response.contains("200 Connection established") || response.contains("200 OK");
                if !alive {
                    anyhow::bail!("HTTP CONNECT failed: {}", response.lines().next().unwrap_or("unknown"));
                }
                Ok(ForwardConnection::HttpConnect(s))
            }
        }
    }
}

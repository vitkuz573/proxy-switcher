use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net;
use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::{BoxedStream, ProxyConnector};

/// An HTTP proxy.
///
/// Two ways of using it, and the difference matters:
///
/// - `CONNECT` establishes a tunnel and can carry anything, including TLS.
/// - Plain forwarding sends the request to the proxy with an absolute URI,
///   and can only carry an HTTP request.
///
/// A great many public proxies implement only the second. Treating them as
/// unusable throws away a large part of the pool; treating them as if they
/// supported CONNECT produces connections that die on the first write.
pub struct HttpConnector;

impl HttpConnector {
    /// Open the connection to the proxy itself.
    pub(crate) async fn open(
        proxy: &ProxyInfo,
        options: &ConnectOptions,
    ) -> Result<tokio::net::TcpStream> {
        let addr = net::resolve_host_port(&proxy.host, proxy.port).await?;
        Ok(net::connect_first(&addr, options.mark).await?)
    }

    /// Establish a tunnel with CONNECT.
    pub(crate) async fn tunnel(
        stream: tokio::net::TcpStream,
        target: &Target,
    ) -> Result<tokio::net::TcpStream> {
        let request = format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n",
            target = target.authority()
        );

        let mut stream = stream;
        stream
            .write_all(request.as_bytes())
            .await
            .context("writing the CONNECT request")?;

        let mut buf = [0u8; 1024];
        let n = stream
            .read(&mut buf)
            .await
            .context("reading the CONNECT response")?;

        let response = String::from_utf8_lossy(&buf[..n]);
        let status = response.lines().next().unwrap_or_default();

        if !status.contains(" 200 ") {
            bail!("HTTP CONNECT failed: {status}");
        }

        Ok(stream)
    }
}

#[async_trait]
impl ProxyConnector for HttpConnector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Http
    }

    fn carriage(&self) -> Carriage {
        Carriage::Stream
    }

    async fn connect(
        &self,
        proxy: &ProxyInfo,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<BoxedStream> {
        let stream = Self::open(proxy, options).await?;
        let tunneled = Self::tunnel(stream, target).await?;
        Ok(Box::new(tunneled))
    }
}

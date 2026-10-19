use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::http::HttpConnector;
use super::{BoxedStream, ProxyConnector};

/// An HTTP proxy used without a tunnel.
///
/// A large part of the public proxy population does not implement CONNECT: it
/// accepts an ordinary HTTP request whose target is written as an absolute
/// URI and returns the response. Such a proxy is perfectly usable for HTTP
/// traffic and useless for anything else.
///
/// The engine has to tell the two apart before it opens a connection, because
/// a CONNECT to a proxy that does not support it is answered with an error,
/// and treating that error as "the proxy is broken" throws away a working
/// proxy. The probe does exactly that: it tries the tunnel first, and falls
/// back to plain forwarding when the proxy refuses the CONNECT.
pub struct HttpPlainConnector;

impl HttpPlainConnector {
    /// Write a request with an absolute URI, as a forward proxy expects.
    pub(crate) async fn write_absolute_request<S: tokio::io::AsyncWrite + Unpin>(
        stream: &mut S,
        path: &str,
        host: &str,
    ) -> Result<()> {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\n\
             User-Agent: proxy-switcher/1.0\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            },
        );

        stream
            .write_all(request.as_bytes())
            .await
            .context("writing the request to the proxy")?;
        stream.flush().await.context("flushing the request")?;
        Ok(())
    }
}

#[async_trait]
impl ProxyConnector for HttpPlainConnector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Http
    }

    fn carriage(&self) -> Carriage {
        Carriage::HttpRequest
    }

    fn supports(&self, target: &Target) -> bool {
        // Only plain HTTP can be carried without a tunnel.
        Carriage::is_plain_http(target.port)
    }

    async fn connect(
        &self,
        proxy: &ProxyInfo,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<BoxedStream> {
        if !Self::supports(self, target) {
            bail!(
                "a proxy without CONNECT can only carry plain HTTP, not port {}",
                target.port
            );
        }

        let stream = HttpConnector::open(proxy, options).await?;
        Ok(Box::new(stream))
    }
}

/// Read a whole HTTP response from a plain-forwarding proxy.
pub async fn read_response<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> Result<String> {
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .context("reading the response from the proxy")?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

use anyhow::{Context, Result};
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;

use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::http::HttpConnector;
use super::{BoxedStream, ProxyConnector};

/// An HTTPS proxy: TLS to the proxy itself, then a CONNECT through it.
///
/// This is a different protocol from an HTTP proxy, not a variant of it. The
/// previous code treated it as the same thing, which meant an HTTPS proxy
/// never worked: the CONNECT was sent in cleartext to a port that expects a
/// TLS handshake.
pub struct HttpsConnector;

impl HttpsConnector {
    /// A TLS client that trusts the usual public roots.
    ///
    /// A proxy's own certificate is usually issued by a public authority; a
    /// self-signed one is a configuration problem, not a reason to skip
    /// verification.
    fn client() -> Result<TlsConnector> {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(TlsConnector::from(Arc::new(config)))
    }
}

use std::sync::Arc;

#[async_trait]
impl ProxyConnector for HttpsConnector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Https
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
        let stream = HttpConnector::open(proxy, options).await?;

        let server_name = ServerName::try_from(proxy.host.clone())
            .map_err(|_| anyhow::anyhow!("invalid proxy host name: {}", proxy.host))?;

        let tls = Self::client()?
            .connect(server_name, stream)
            .await
            .context("TLS handshake with the proxy")?;

        // The CONNECT now travels inside the TLS session.
        let mut tls = tls;
        let request = format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n",
            target = target.authority()
        );
        tls.write_all(request.as_bytes())
            .await
            .context("writing the CONNECT request")?;

        let mut buf = [0u8; 1024];
        let n = tls
            .read(&mut buf)
            .await
            .context("reading the CONNECT response")?;
        let response = String::from_utf8_lossy(&buf[..n]);
        let status = response.lines().next().unwrap_or_default();
        if !status.contains(" 200 ") {
            anyhow::bail!("HTTP CONNECT failed: {status}");
        }

        Ok(Box::new(tls))
    }
}

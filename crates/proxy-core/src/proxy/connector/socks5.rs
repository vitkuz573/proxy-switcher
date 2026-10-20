use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::{BoxedStream, ProxyConnector};

/// A SOCKS5 proxy, with optional username and password.
///
/// SOCKS5 has three handshake variants and the proxy picks which one it
/// wants: no authentication, username and password, or a local address the
/// client must use. Only the first was implemented, so every proxy that
/// asked for credentials was treated as broken.
pub struct Socks5Connector;

impl Socks5Connector {
    /// Perform the handshake and return the stream.
    pub(crate) async fn handshake(
        mut stream: tokio::net::TcpStream,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<tokio::net::TcpStream> {
        // Greeting: offer both no-auth and username/password.
        let greeting = vec![0x05u8, 0x02, 0x00, 0x02];
        stream
            .write_all(&greeting)
            .await
            .context("writing the SOCKS5 greeting")?;

        let mut answer = [0u8; 2];
        stream
            .read_exact(&mut answer)
            .await
            .context("reading the SOCKS5 greeting answer")?;

        if answer[0] != 0x05 {
            bail!("not a SOCKS5 proxy: version {}", answer[0]);
        }

        match answer[1] {
            0x00 => {}
            0x02 => {
                let credentials = options
                    .credentials
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("the proxy requires a username and password"))?;

                let mut request = vec![0x01u8, credentials.username.len() as u8];
                request.extend_from_slice(credentials.username.as_bytes());
                request.push(credentials.password.len() as u8);
                request.extend_from_slice(credentials.password.as_bytes());

                stream
                    .write_all(&request)
                    .await
                    .context("writing the SOCKS5 credentials")?;

                let mut reply = [0u8; 2];
                stream
                    .read_exact(&mut reply)
                    .await
                    .context("reading the SOCKS5 authentication answer")?;

                if reply[1] != 0x00 {
                    bail!("the proxy rejected the credentials");
                }
            }
            other => bail!("the proxy asked for an unsupported authentication method: {other}"),
        }

        // The request. A name is sent as a domain so the proxy resolves it:
        // the client cannot always resolve it, and the proxy usually can.
        let mut request = vec![0x05u8, 0x01, 0x00];

        if let Ok(ip) = target.host.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(v4) => {
                    request.push(0x01);
                    request.extend_from_slice(&v4.octets());
                }
                std::net::IpAddr::V6(v6) => {
                    request.push(0x04);
                    request.extend_from_slice(&v6.octets());
                }
            }
        } else {
            if target.host.len() > 255 {
                bail!("the host name is too long for SOCKS5");
            }
            request.push(0x03);
            request.push(target.host.len() as u8);
            request.extend_from_slice(target.host.as_bytes());
        }

        request.extend_from_slice(&target.port.to_be_bytes());

        stream
            .write_all(&request)
            .await
            .context("writing the SOCKS5 connect request")?;

        let mut reply = [0u8; 4];
        stream
            .read_exact(&mut reply)
            .await
            .context("reading the SOCKS5 connect reply")?;

        if reply[1] != 0x00 {
            bail!(
                "the SOCKS5 proxy refused the connection: {}",
                socks_error(reply[1])
            );
        }

        // Drain the bound address.
        let bound_len = match reply[3] {
            0x01 => 4,
            0x04 => 16,
            0x03 => {
                let mut len = [0u8; 1];
                stream
                    .read_exact(&mut len)
                    .await
                    .context("reading the SOCKS5 bound name")?;
                len[0] as usize
            }
            _ => bail!("the SOCKS5 proxy returned an unknown address type"),
        };

        let mut bound = vec![0u8; bound_len + 2];
        stream
            .read_exact(&mut bound)
            .await
            .context("reading the SOCKS5 bound address")?;

        Ok(stream)
    }
}

/// The SOCKS5 reply code, in words.
fn socks_error(code: u8) -> &'static str {
    match code {
        0x01 => "general failure",
        0x02 => "connection not allowed",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    }
}

#[async_trait]
impl ProxyConnector for Socks5Connector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Socks5
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
        let started = std::time::Instant::now();
        let addrs = crate::net::resolve_host_port(&proxy.host, proxy.port).await?;
        tracing::debug!(
            "socks5: {} resolved to {} in {:?}",
            proxy.host,
            addrs.len(),
            started.elapsed()
        );
        let stream = crate::net::connect_first(&addrs, options.mark).await?;
        tracing::debug!(
            "socks5: connected to {} in {:?}",
            proxy.host,
            started.elapsed()
        );
        let established = Self::handshake(stream, target, options).await?;
        tracing::debug!(
            "socks5: tunnel to {}:{} ready in {:?}",
            target.host,
            target.port,
            started.elapsed()
        );
        Ok(Box::new(established))
    }
}

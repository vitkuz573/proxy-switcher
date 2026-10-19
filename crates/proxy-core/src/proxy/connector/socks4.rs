use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::{BoxedStream, ProxyConnector};

/// A SOCKS4 proxy, and SOCKS4a.
///
/// SOCKS4 cannot carry a host name: it takes an address, and the address
/// `0.0.0.x` means "look the name up yourself". SOCKS4a is the variant that
/// adds that, and it is the only way a SOCKS4 proxy can reach a host by name.
///
/// The two are told apart by what the client sends: a SOCKS4 request has four
/// bytes of address and a SOCKS4a request has a name after them. The server
/// cannot tell them apart, so the client chooses.
pub struct Socks4Connector;

impl Socks4Connector {
    /// Perform the handshake.
    ///
    /// `resolve_locally` decides between SOCKS4 and SOCKS4a: when the target
    /// is a name, SOCKS4a is the only variant that can carry it.
    pub(crate) async fn handshake(
        mut stream: tokio::net::TcpStream,
        target: &Target,
    ) -> Result<tokio::net::TcpStream> {
        let mut request = vec![0x04u8, 0x01];

        match target.host.parse::<std::net::Ipv4Addr>() {
            Ok(ip) => {
                request.extend_from_slice(&ip.octets());
            }
            Err(_) => {
                // SOCKS4a: an address of 0.0.0.x means "the name follows".
                request.extend_from_slice(&[0, 0, 0, 1]);
            }
        }

        request.extend_from_slice(&target.port.to_be_bytes());
        request.extend_from_slice(b"proxy-switcher\0");

        if target.host.parse::<std::net::Ipv4Addr>().is_err() {
            request.extend_from_slice(target.host.as_bytes());
            request.push(0);
        }

        stream
            .write_all(&request)
            .await
            .context("writing the SOCKS4 request")?;

        let mut reply = [0u8; 8];
        stream
            .read_exact(&mut reply)
            .await
            .context("reading the SOCKS4 reply")?;

        if reply[1] != 0x5a {
            bail!(
                "the SOCKS4 proxy refused the connection: {}",
                socks4_error(reply[1])
            );
        }

        Ok(stream)
    }
}

fn socks4_error(code: u8) -> &'static str {
    match code {
        0x5b => "request rejected or failed",
        0x5c => "request rejected because the identd is unreachable",
        0x5d => "request rejected because the identd does not match",
        _ => "unknown error",
    }
}

#[async_trait]
impl ProxyConnector for Socks4Connector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Socks4
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
        let addrs = crate::net::resolve_host_port(&proxy.host, proxy.port).await?;
        let stream = crate::net::connect_first(&addrs, options.mark).await?;
        let established = Self::handshake(stream, target).await?;
        Ok(Box::new(established))
    }
}

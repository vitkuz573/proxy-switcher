use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use sha2::{Digest, Sha224};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;

use crate::proxy::ProxyInfo;
use crate::proxy::ProxyProtocol;

use super::base::{Carriage, ConnectOptions, Target};
use super::{BoxedStream, ProxyConnector};

/// A Trojan proxy.
///
/// Trojan is TLS to the server, then a password and a SOCKS5-shaped address.
/// It is the protocol of most of the servers that are actually reachable
/// today, and it is a poor fit for a plain HTTP proxy list: the servers are
/// named, they carry a credential, and they expect TLS.
///
/// The credential travels as the hex of the password's SHA-224, so it is never
/// on the wire in the clear even before the session is encrypted.
#[derive(Default)]
pub struct TrojanConnector {
    /// The roots to trust, when they are not the public ones.
    ///
    /// The seam exists so the protocol can be tested against a server with a
    /// certificate of its own, rather than only being written.
    roots: Option<RootCertStore>,
}

impl TrojanConnector {
    /// A connector that trusts a given set of roots.
    pub fn with_roots(roots: RootCertStore) -> Self {
        Self { roots: Some(roots) }
    }
}

impl TrojanConnector {
    /// The credential, as the protocol wants it: hex of SHA-224.
    fn credential(password: &str) -> String {
        let digest = Sha224::digest(password.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The address, in the SOCKS5 shape the protocol uses.
    fn address(target: &Target) -> Vec<u8> {
        let mut out = Vec::with_capacity(10);

        if let Ok(ip) = target.host.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(v4) => {
                    out.push(0x01);
                    out.extend_from_slice(&v4.octets());
                }
                std::net::IpAddr::V6(v6) => {
                    out.push(0x04);
                    out.extend_from_slice(&v6.octets());
                }
            }
        } else {
            out.push(0x03);
            out.push(target.host.len() as u8);
            out.extend_from_slice(target.host.as_bytes());
        }

        out.extend_from_slice(&target.port.to_be_bytes());
        out
    }

    fn client(&self) -> Result<TlsConnector> {
        let roots = self.roots.clone().unwrap_or_else(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            roots
        });

        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(TlsConnector::from(std::sync::Arc::new(config)))
    }
}

#[async_trait]
impl ProxyConnector for TrojanConnector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Trojan
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
        let password = proxy.password.as_deref().ok_or_else(|| {
            anyhow::anyhow!("a Trojan proxy needs a password, and this one has none")
        })?;

        // The server is named, not addressed by number: the name is also the
        // certificate's subject, and TLS has no way to say otherwise.
        let server_name = ServerName::try_from(proxy.host.clone()).map_err(|_| {
            anyhow::anyhow!(
                "a Trojan server has to be named, and {:?} is an address",
                proxy.host
            )
        })?;

        let addrs = crate::net::resolve_host_port(&proxy.host, proxy.port).await?;
        let stream = crate::net::connect_first(&addrs, options.mark).await?;

        let mut tls = self
            .client()?
            .connect(server_name, stream)
            .await
            .context("TLS handshake with the Trojan server")?;

        // The credential, then the address, then the payload.
        tls.write_all(Self::credential(password).as_bytes())
            .await
            .context("writing the Trojan credential")?;
        tls.write_all(b"\r\n")
            .await
            .context("writing the Trojan credential")?;

        let mut request = vec![0x01u8];
        request.extend(Self::address(target));
        tls.write_all(&request)
            .await
            .context("writing the Trojan address")?;

        // The reply is a SOCKS5-shaped answer.
        // The reply is a SOCKS5-shaped answer: version, code, a reserved byte,
        // and then the address type.
        let mut header = [0u8; 4];
        tls.read_exact(&mut header)
            .await
            .context("reading the Trojan reply")?;

        if header[1] != 0x00 {
            bail!(
                "the Trojan server refused the connection: {}",
                reply_text(header[1])
            );
        }

        // The bound address and its port. The port has to be consumed as
        // well: leaving it in the stream puts it in front of the payload.
        match header[3] {
            0x01 => {
                let mut bound = [0u8; 4 + 2];
                tls.read_exact(&mut bound).await?;
            }
            0x04 => {
                let mut bound = [0u8; 16 + 2];
                tls.read_exact(&mut bound).await?;
            }
            0x03 => {
                let mut len = [0u8; 1];
                tls.read_exact(&mut len).await?;
                let mut bound = vec![0u8; len[0] as usize + 2];
                tls.read_exact(&mut bound).await?;
            }
            other => bail!(
                "the Trojan server returned an unknown address type: {other} (reply: {header:?})"
            ),
        }

        Ok(Box::new(tls))
    }
}

/// The SOCKS5 reply code, in words.
fn reply_text(code: u8) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_is_the_hex_of_the_hash() {
        let credential = TrojanConnector::credential("secret");
        assert_eq!(
            credential.len(),
            56,
            "SHA-224 is 28 bytes, so 56 hex characters"
        );
        assert!(credential.chars().all(|c| c.is_ascii_hexdigit()));
        // The same password always gives the same credential.
        assert_eq!(credential, TrojanConnector::credential("secret"));
        assert_ne!(credential, TrojanConnector::credential("other"));
    }

    #[test]
    fn the_address_uses_the_socks5_shape() {
        let target = Target::new("198.51.100.7", 443);
        let address = TrojanConnector::address(&target);
        assert_eq!(address[0], 0x01, "an IPv4 address");
        assert_eq!(address[1..5], [198, 51, 100, 7]);
        assert_eq!(&address[5..7], &443u16.to_be_bytes());
    }

    #[test]
    fn a_name_is_sent_as_a_name() {
        let target = Target::new("example.com", 80);
        let address = TrojanConnector::address(&target);
        assert_eq!(address[0], 0x03, "a domain name");
        assert_eq!(address[1] as usize, "example.com".len());
    }
}

//! The Trojan connector, exercised against a server that speaks the protocol.
//!
//! The point is to check the handshake itself: the credential, the address and
//! the reply. A connector that has only been written is a connector that has
//! never been run.

use std::sync::Arc;

use anyhow::Result;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use proxy_core::proxy::connector::base::{ConnectOptions, Target};
use proxy_core::proxy::connector::trojan::TrojanConnector;
use proxy_core::proxy::connector::ProxyConnector;
use proxy_core::proxy::{Anonymity, ProxyInfo, ProxyProtocol};

/// The server's certificate and the root that trusts it, as one thing.
///
/// They have to be the same pair: a client that trusts one authority and a
/// server that presents a leaf signed by another fail the handshake, which
/// looks exactly like a broken certificate.
struct TestCertificate {
    certificates: Vec<CertificateDer<'static>>,
    key_der: Vec<u8>,
    roots: RootCertStore,
}

impl TestCertificate {
    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::try_from(self.key_der.clone()).expect("the private key")
    }
}

/// A certificate authority and a leaf signed by it.
fn test_certificate() -> TestCertificate {
    let mut authority_params =
        rcgen::CertificateParams::new(Vec::<String>::new()).expect("the parameters");
    authority_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    authority_params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyCertSign,
    ];

    let authority_key = rcgen::KeyPair::generate().expect("a key");
    let authority = rcgen::CertifiedIssuer::self_signed(authority_params, authority_key)
        .expect("the authority");
    let authority_der = CertificateDer::from(authority.der().to_vec());

    let leaf_params =
        rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("the parameters");
    let leaf_key = rcgen::KeyPair::generate().expect("a key");
    let leaf = leaf_params
        .signed_by(&leaf_key, &authority)
        .expect("the leaf");

    let mut roots = RootCertStore::empty();
    roots.add(authority_der).expect("the root");

    TestCertificate {
        certificates: vec![CertificateDer::from(leaf.der().to_vec())],
        key_der: leaf_key.serialize_der(),
        roots,
    }
}

/// A server that speaks the Trojan handshake and then echoes.
async fn trojan_server(listener: TcpListener, certificate: TestCertificate) {
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificate.certificates.clone(), certificate.key())
        .expect("the server configuration");
    let acceptor = TlsAcceptor::from(Arc::new(config));

    while let Ok((stream, _)) = listener.accept().await {
        let acceptor = acceptor.clone();

        tokio::spawn(async move {
            let mut tls = match acceptor.accept(stream).await {
                Ok(tls) => tls,
                Err(_) => return,
            };

            // The credential, terminated by a line break.
            let mut credential = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = tls.read(&mut byte).await {
                if byte[0] == b'\r' {
                    break;
                }
                credential.push(byte[0]);
            }
            let _ = tls.read(&mut byte).await;

            // The address, in the SOCKS5 shape: a command, an address type
            // and then an address whose length depends on that type.
            let mut command = [0u8; 1];
            if tls.read_exact(&mut command).await.is_err() {
                return;
            }
            let mut kind = [0u8; 1];
            if tls.read_exact(&mut kind).await.is_err() {
                return;
            }
            let length = match kind[0] {
                0x01 => 4,
                0x04 => 16,
                0x03 => {
                    let mut len = [0u8; 1];
                    if tls.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    len[0] as usize
                }
                _ => return,
            };
            let mut address = vec![0u8; length + 2];
            if tls.read_exact(&mut address).await.is_err() {
                return;
            }

            // The reply, in the SOCKS5 shape.
            let mut reply = vec![0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0];
            reply.extend_from_slice(&0u16.to_be_bytes());
            let _ = tls.write_all(&reply).await;

            // The payload, echoed.
            let mut payload = [0u8; 64];
            if let Ok(n) = tls.read(&mut payload).await {
                let _ = tls.write_all(&payload[..n]).await;
            }
        });
    }
}

/// Connect through a Trojan proxy and exchange a payload.
async fn round_trip(certificate: TestCertificate, password: &str) -> Result<Vec<u8>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let roots = certificate.roots.clone();
    tokio::spawn(trojan_server(listener, certificate));

    let proxy = ProxyInfo {
        id: format!("localhost:{port}"),
        host: "localhost".to_string(),
        port,
        protocol: ProxyProtocol::Trojan,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: None,
        country_reported: None,
        last_checked: None,
        last_success: None,
        first_seen: chrono::Utc::now(),
        next_probe_at: chrono::Utc::now(),
        supports_tunnel: true,
        exit_ip: None,
        password: Some(password.into()),
        consecutive_failures: 0,
        score: 0.0,
    };

    let connector = TrojanConnector::with_roots(roots);
    let mut stream = connector
        .connect(
            &proxy,
            &Target::new("example.com", 80),
            &ConnectOptions::default(),
        )
        .await?;

    stream.write_all(b"ping").await?;
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await?;

    Ok(buf.to_vec())
}

#[tokio::test]
async fn carries_a_payload_through_the_handshake() {
    let echo = round_trip(test_certificate(), "secret").await;
    assert!(echo.is_ok(), "the handshake failed: {echo:?}");
    assert_eq!(echo.unwrap(), b"ping".to_vec());
}

#[tokio::test]
async fn refuses_a_server_the_client_does_not_trust() {
    // Roots that trust nothing: the handshake must fail rather than proceed.
    let mut certificate = test_certificate();
    certificate.roots = RootCertStore::empty();

    let result = round_trip(certificate, "secret").await;
    assert!(result.is_err(), "an untrusted server must be refused");
}

#[tokio::test]
async fn refuses_a_proxy_without_a_password() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("the address").port();
    tokio::spawn(trojan_server(listener, test_certificate()));

    let proxy = ProxyInfo {
        id: format!("localhost:{port}"),
        host: "localhost".to_string(),
        port,
        protocol: ProxyProtocol::Trojan,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: None,
        country_reported: None,
        last_checked: None,
        last_success: None,
        first_seen: chrono::Utc::now(),
        next_probe_at: chrono::Utc::now(),
        supports_tunnel: true,
        exit_ip: None,
        password: None,
        consecutive_failures: 0,
        score: 0.0,
    };

    let connector = TrojanConnector::with_roots(test_certificate().roots);
    let result = connector
        .connect(
            &proxy,
            &Target::new("example.com", 80),
            &ConnectOptions::default(),
        )
        .await;

    assert!(
        result.is_err(),
        "a proxy without a password must be refused"
    );
}

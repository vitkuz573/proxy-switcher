//! Shadowsocks, the AEAD version.
//!
//! The protocol is simple and unforgiving: a session starts with a random
//! salt, the salt and the password give a key, and the payload travels in
//! chunks that are each encrypted on their own. There is no handshake to
//! speak of and no way to ask the server what it is — the connection either
//! carries the address and the payload, or it does not.
//!
//! The pieces:
//!
//! - the password gives a master key, the way OpenSSL derives one;
//! - the master key and the salt give a session key, by HKDF-SHA1;
//! - the payload is cut into chunks, and each chunk is sealed with
//!   ChaCha20-Poly1305 under a counter that moves on with every sealing.
//!
//! The chunk is a sealed length followed by a sealed payload. The length is
//! sealed as well, so the size of the payload is not readable from the wire.
//!
//! The two directions have keys of their own: the client opens with its salt,
//! and the server answers with one of its own.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use anyhow::{bail, Context as _, Result};
use async_trait::async_trait;
use chacha20poly1305::ChaCha20Poly1305;
use hkdf::Hkdf;
use md5::{Digest, Md5};
use sha1::Sha1;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::proxy::connector::base::{ConnectOptions, Target};
use crate::proxy::connector::{BoxedStream, ProxyConnector};
use crate::proxy::{ProxyInfo, ProxyProtocol};

/// The size of the authentication tag.
const TAG: usize = 16;

/// The largest payload a chunk may carry.
const MAX_CHUNK: usize = 0x3fff;

/// The size of the random salt that opens a session.
const SALT_LEN: usize = 16;

/// The size of the key.
const KEY_LEN: usize = 32;

/// The length of the sealed chunk header: the length and its tag.
const SEALED_LEN: usize = 2 + TAG;

/// A Shadowsocks proxy.
pub struct ShadowsocksConnector;

impl ShadowsocksConnector {
    /// The key the password gives.
    ///
    /// The derivation is the one OpenSSL does, with MD5: the digest of the
    /// password, then the digest of that digest and the password, and so on
    /// until there are enough bytes. Shadowsocks has used it since the first
    /// version, and every server expects it.
    pub fn master_key(password: &str, length: usize) -> Vec<u8> {
        let password = password.as_bytes();
        let mut key = Vec::with_capacity(length);
        let mut previous: Vec<u8> = Vec::new();

        while key.len() < length {
            let mut hasher = Md5::new();
            hasher.update(&previous);
            hasher.update(password);
            previous = hasher.finalize().to_vec();
            key.extend_from_slice(&previous);
        }

        key.truncate(length);
        key
    }

    /// The key a session uses.
    ///
    /// The salt is what the client chose, and the master key is the secret.
    /// The two together give the key the chunks are sealed under.
    pub fn session_key(master: &[u8], salt: &[u8], length: usize) -> Result<Vec<u8>> {
        let hkdf = Hkdf::<Sha1>::new(Some(salt), master);
        let mut key = vec![0u8; length];
        hkdf.expand(b"ss-subkey", &mut key)
            .map_err(|_| anyhow::anyhow!("the session key could not be derived"))?;
        Ok(key)
    }

    /// The address of the target, in the form the protocol uses.
    ///
    /// A name is sent as a name: the server resolves it, from wherever the
    /// server is, which is the point of the proxy.
    pub fn encode_address(host: &str, port: u16) -> Vec<u8> {
        let mut out = Vec::with_capacity(host.len() + 7);

        match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(address)) => {
                out.push(0x01);
                out.extend_from_slice(&address.octets());
            }
            Ok(std::net::IpAddr::V6(address)) => {
                out.push(0x04);
                out.extend_from_slice(&address.octets());
            }
            Err(_) => {
                out.push(0x03);
                out.push(host.len() as u8);
                out.extend_from_slice(host.as_bytes());
            }
        }

        out.extend_from_slice(&port.to_be_bytes());
        out
    }

    /// The address the protocol carries, read back.
    pub fn decode_address(raw: &[u8]) -> Option<(String, u16)> {
        let port = u16::from_be_bytes([*raw.get(raw.len().checked_sub(2)?)?, *raw.last()?]);

        match *raw.first()? {
            0x01 => {
                let bytes: [u8; 4] = raw.get(1..5)?.try_into().ok()?;
                Some((std::net::Ipv4Addr::from(bytes).to_string(), port))
            }
            0x04 => {
                let bytes: [u8; 16] = raw.get(1..17)?.try_into().ok()?;
                Some((std::net::Ipv6Addr::from(bytes).to_string(), port))
            }
            0x03 => {
                let length = *raw.get(1)? as usize;
                let name = raw.get(2..2 + length)?;
                Some((String::from_utf8_lossy(name).to_string(), port))
            }
            _ => None,
        }
    }
}

#[async_trait]
impl ProxyConnector for ShadowsocksConnector {
    fn protocol(&self) -> ProxyProtocol {
        ProxyProtocol::Shadowsocks
    }

    async fn connect(
        &self,
        proxy: &ProxyInfo,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<BoxedStream> {
        let Some(password) = proxy.password.as_deref() else {
            bail!("a shadowsocks proxy needs its password");
        };

        let addrs = crate::net::resolve_host_port(&proxy.host, proxy.port).await?;
        let stream = crate::net::connect_first(&addrs, options.mark).await?;

        // The session opens with a random salt, and the salt and the password
        // give the key everything after it is sealed under.
        let mut salt = [0u8; SALT_LEN];
        rand::fill(&mut salt);

        // The cipher the proxy wants, and the key it gives. The method
        // travels with the entry, because the same password under a
        // different cipher is a different session.
        let cipher = match proxy.cipher.as_deref() {
            Some(name) => match Cipher::from_name(name) {
                Some(cipher) => cipher,
                None => bail!("the shadowsocks proxy wants the cipher {name}, which is not known"),
            },
            None => Cipher::ChaCha20Poly1305,
        };

        let master = ShadowsocksConnector::master_key(password, cipher.key_len());
        let key = ShadowsocksConnector::session_key(&master, &salt, cipher.key_len())?;

        let mut stream = stream;
        stream.write_all(&salt).await.context("writing the salt")?;

        let mut session = ShadowsocksStream::new(stream, key, master, cipher)?;

        // The address of the target goes first, as the first chunk.
        let address = ShadowsocksConnector::encode_address(&target.host, target.port);
        session.write_all(&address).await?;
        session.flush().await?;

        Ok(Box::new(session))
    }
}

/// The ciphers a session may use.
///
/// The method travels with the entry, because the same password under a
/// different cipher is a different session. Most of the lists carry
/// ChaCha20-Poly1305, which is the default; the AES variants are there for
/// the ones that do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    /// `chacha20-ietf-poly1305` — the default, and the one most lists carry.
    ChaCha20Poly1305,
    /// `aes-128-gcm`.
    Aes128Gcm,
    /// `aes-256-gcm`.
    Aes256Gcm,
}

impl Cipher {
    /// The cipher a name asks for.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "chacha20-ietf-poly1305" | "chacha20-poly1305" | "chacha20" => {
                Some(Cipher::ChaCha20Poly1305)
            }
            "aes-128-gcm" => Some(Cipher::Aes128Gcm),
            "aes-256-gcm" => Some(Cipher::Aes256Gcm),
            _ => None,
        }
    }

    /// The name the cipher is known by.
    pub fn name(&self) -> &'static str {
        match self {
            Cipher::ChaCha20Poly1305 => "chacha20-ietf-poly1305",
            Cipher::Aes128Gcm => "aes-128-gcm",
            Cipher::Aes256Gcm => "aes-256-gcm",
        }
    }

    /// The length of the key the cipher takes.
    pub fn key_len(&self) -> usize {
        match self {
            Cipher::Aes128Gcm => 16,
            _ => KEY_LEN,
        }
    }
}

/// The cipher that does the work.
enum CipherKind {
    ChaCha(Box<ChaCha20Poly1305>),
    Aes128(Box<Aes128Gcm>),
    Aes256(Box<Aes256Gcm>),
}

/// The cipher of one direction.
struct Seal {
    cipher: CipherKind,
    counter: u64,
}

impl Seal {
    fn new(cipher: Cipher, key: &[u8]) -> Result<Self> {
        let expected = cipher.key_len();
        if key.len() != expected {
            anyhow::bail!("the key is not {expected} bytes long");
        }

        let cipher = match cipher {
            Cipher::ChaCha20Poly1305 => CipherKind::ChaCha(Box::new(
                ChaCha20Poly1305::new_from_slice(key)
                    .map_err(|_| anyhow::anyhow!("the key is not {KEY_LEN} bytes long"))?,
            )),
            Cipher::Aes128Gcm => CipherKind::Aes128(Box::new(
                Aes128Gcm::new_from_slice(key)
                    .map_err(|_| anyhow::anyhow!("the key is not 16 bytes long"))?,
            )),
            Cipher::Aes256Gcm => CipherKind::Aes256(Box::new(
                Aes256Gcm::new_from_slice(key)
                    .map_err(|_| anyhow::anyhow!("the key is not {KEY_LEN} bytes long"))?,
            )),
        };

        Ok(Self { cipher, counter: 0 })
    }

    /// The nonce for the next sealing.
    ///
    /// The counter is little endian and moves on with every sealing, so no
    /// nonce is ever used twice under one key.
    fn next(counter: &mut u64) -> [u8; 12] {
        let mut bytes = [0u8; 12];
        bytes[..8].copy_from_slice(&counter.to_le_bytes());
        *counter += 1;
        bytes
    }

    /// Seal a piece of plaintext.
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let payload = Payload {
            msg: plaintext,
            aad: &[],
        };

        let sealed = match &mut self.cipher {
            CipherKind::ChaCha(cipher) => {
                let nonce = chacha20poly1305::Nonce::from(Self::next(&mut self.counter));
                cipher.encrypt(&nonce, payload)
            }
            CipherKind::Aes128(cipher) => {
                let nonce = aes_gcm::Nonce::from(Self::next(&mut self.counter));
                cipher.encrypt(&nonce, payload)
            }
            CipherKind::Aes256(cipher) => {
                let nonce = aes_gcm::Nonce::from(Self::next(&mut self.counter));
                cipher.encrypt(&nonce, payload)
            }
        };

        sealed.map_err(|_| anyhow::anyhow!("the chunk could not be sealed"))
    }

    /// Open a sealed piece.
    fn open(&mut self, sealed: &[u8]) -> Result<Vec<u8>> {
        let payload = Payload {
            msg: sealed,
            aad: &[],
        };

        let opened = match &mut self.cipher {
            CipherKind::ChaCha(cipher) => {
                let nonce = chacha20poly1305::Nonce::from(Self::next(&mut self.counter));
                cipher.decrypt(&nonce, payload)
            }
            CipherKind::Aes128(cipher) => {
                let nonce = aes_gcm::Nonce::from(Self::next(&mut self.counter));
                cipher.decrypt(&nonce, payload)
            }
            CipherKind::Aes256(cipher) => {
                let nonce = aes_gcm::Nonce::from(Self::next(&mut self.counter));
                cipher.decrypt(&nonce, payload)
            }
        };

        opened.map_err(|_| anyhow::anyhow!("the chunk could not be opened"))
    }
}

/// A stream that carries a Shadowsocks session.
///
/// The plaintext goes in and comes out; everything on the wire is sealed.
pub struct ShadowsocksStream {
    stream: tokio::net::TcpStream,
    /// The master key, which the answer's key is derived from.
    master: Vec<u8>,
    /// The cipher the session uses.
    cipher: Cipher,
    /// The seal the requests go under.
    write: Seal,
    /// The seal the answers come under.
    ///
    /// The server opens with a salt of its own, and the answer arrives under
    /// a key of its own. The key is only known once the salt has been read,
    /// so it is made on the first read.
    read: Option<Seal>,
    /// The plaintext that came out of the last chunk and is not yet read.
    plaintext: Vec<u8>,
    /// How much of the plaintext has been read.
    read_at: usize,
}

impl ShadowsocksStream {
    /// A stream that seals and opens with the given key.
    ///
    /// The master key is kept as well: the answer arrives under a key of the
    /// server's own, and that key is derived from the same master.
    pub fn new(
        stream: tokio::net::TcpStream,
        key: Vec<u8>,
        master: Vec<u8>,
        cipher: Cipher,
    ) -> Result<Self> {
        Ok(Self {
            stream,
            master,
            cipher,
            write: Seal::new(cipher, &key)?,
            read: None,
            plaintext: Vec::new(),
            read_at: 0,
        })
    }
}

impl AsyncWrite for ShadowsocksStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // The plaintext is cut into chunks and each chunk is sealed on its
        // own. The chunk is written whole: a part of a chunk on the wire is
        // not something the other side can read.
        let mut written = 0;

        while written < buf.len() {
            let end = (written + MAX_CHUNK).min(buf.len());
            let chunk = &buf[written..end];

            // The length goes out first, sealed, and the payload after it.
            // The size of the payload is not readable from the wire.
            let length = (chunk.len() as u16).to_be_bytes();
            let header = match self.write.seal(&length) {
                Ok(sealed) => sealed,
                Err(e) => return Poll::Ready(Err(io::Error::other(e))),
            };
            let payload = match self.write.seal(chunk) {
                Ok(sealed) => sealed,
                Err(e) => return Poll::Ready(Err(io::Error::other(e))),
            };

            let framed = [header, payload].concat();
            let mut written_now = 0;

            while written_now < framed.len() {
                match Pin::new(&mut self.stream).poll_write(cx, &framed[written_now..]) {
                    Poll::Ready(Ok(0)) => {
                        return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)))
                    }
                    Poll::Ready(Ok(n)) => written_now += n,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }
            }

            written = end;
        }

        Poll::Ready(Ok(written))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl AsyncRead for ShadowsocksStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            // What came out of the last chunk is handed out first.
            if self.read_at < self.plaintext.len() {
                let available = self.plaintext.len() - self.read_at;
                let take = available.min(buf.remaining());
                let start = self.read_at;
                let end = start + take;
                buf.put_slice(&self.plaintext[start..end]);
                self.read_at = end;
                return Poll::Ready(Ok(()));
            }

            // The server opens with a salt of its own, and the answer arrives
            // under a key of its own.
            if self.read.is_none() {
                let mut salt = [0u8; SALT_LEN];
                match poll_read_exact(&mut self.stream, &mut salt, cx) {
                    Poll::Ready(Ok(true)) => {}
                    // The server closed the connection: the stream is over.
                    Poll::Ready(Ok(false)) => return Poll::Ready(Ok(())),
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }

                // The key of the answer is the server's salt and the master
                // key. The master key is the one the client sent with, so it
                // is kept beside the stream.
                let key = match ShadowsocksConnector::session_key(
                    &self.master,
                    &salt,
                    self.cipher.key_len(),
                ) {
                    Ok(key) => key,
                    Err(e) => return Poll::Ready(Err(io::Error::other(e))),
                };
                self.read = match Seal::new(self.cipher, &key) {
                    Ok(seal) => Some(seal),
                    Err(e) => return Poll::Ready(Err(io::Error::other(e))),
                };
            }

            // The next chunk: the sealed length, then the sealed payload.
            let mut header = [0u8; SEALED_LEN];
            match poll_read_exact(&mut self.stream, &mut header, cx) {
                Poll::Ready(Ok(true)) => {}
                // The stream ended between the chunks.
                Poll::Ready(Ok(false)) => return Poll::Ready(Ok(())),
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }

            let seal = self.read.as_mut().expect("the key was just made");
            let opened_header = match seal.open(&header) {
                Ok(opened) => opened,
                Err(e) => return Poll::Ready(Err(io::Error::other(e))),
            };

            if opened_header.len() != 2 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the chunk header is not a length",
                )));
            }

            let length = u16::from_be_bytes([opened_header[0], opened_header[1]]) as usize;
            if length == 0 {
                // The end of the stream.
                return Poll::Ready(Ok(()));
            }
            if length > MAX_CHUNK {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the chunk is longer than the protocol allows",
                )));
            }

            let mut sealed = vec![0u8; length + TAG];
            match poll_read_exact(&mut self.stream, &mut sealed, cx) {
                Poll::Ready(Ok(true)) => {}
                Poll::Ready(Ok(false)) => {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)))
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }

            let seal = self.read.as_mut().expect("the key was just made");
            let opened = match seal.open(&sealed) {
                Ok(opened) => opened,
                Err(e) => return Poll::Ready(Err(io::Error::other(e))),
            };

            self.plaintext = opened;
            self.read_at = 0;
        }
    }
}

/// Read exactly as many bytes as the buffer holds.
///
/// The answer says whether the buffer is full. A stream that ends before
/// anything was read is not a fault — it is the end of the stream. A stream
/// that ends in the middle of a chunk is a fault, and says so.
fn poll_read_exact<T>(
    reader: &mut T,
    buf: &mut [u8],
    cx: &mut Context<'_>,
) -> Poll<io::Result<bool>>
where
    T: AsyncRead + Unpin,
{
    let mut read = 0;

    while read < buf.len() {
        let mut chunk = ReadBuf::new(&mut buf[read..]);

        match Pin::new(&mut *reader).poll_read(cx, &mut chunk) {
            Poll::Ready(Ok(())) => {
                if chunk.filled().is_empty() {
                    return if read == 0 {
                        Poll::Ready(Ok(false))
                    } else {
                        Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)))
                    };
                }
                read += chunk.filled().len();
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }
    }

    Poll::Ready(Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn the_key_comes_from_the_password() {
        // The derivation is fixed: the same password always gives the same
        // key, and a server that was set up with this password opens the
        // session.
        let first = ShadowsocksConnector::master_key("secret", KEY_LEN);
        let second = ShadowsocksConnector::master_key("secret", KEY_LEN);

        assert_eq!(first, second);
        assert_eq!(first.len(), KEY_LEN);
        assert_ne!(first, ShadowsocksConnector::master_key("another", KEY_LEN));
    }

    #[test]
    fn the_session_key_depends_on_the_salt() {
        let master = ShadowsocksConnector::master_key("secret", KEY_LEN);
        let one = ShadowsocksConnector::session_key(&master, &[1u8; SALT_LEN], KEY_LEN).unwrap();
        let two = ShadowsocksConnector::session_key(&master, &[2u8; SALT_LEN], KEY_LEN).unwrap();

        assert_eq!(one.len(), KEY_LEN);
        assert_ne!(one, two);
    }

    #[test]
    fn an_address_is_written_the_way_the_protocol_wants_it() {
        let name = ShadowsocksConnector::encode_address("example.com", 443);
        assert_eq!(name[0], 0x03);
        assert_eq!(name[1] as usize, "example.com".len());
        assert_eq!(&name[2..13], b"example.com");
        assert_eq!(&name[13..15], &443u16.to_be_bytes());

        let address = ShadowsocksConnector::encode_address("198.51.100.7", 80);
        assert_eq!(address[0], 0x01);
        assert_eq!(&address[1..5], &[198, 51, 100, 7]);
        assert_eq!(&address[5..7], &80u16.to_be_bytes());

        // The address survives the round trip.
        assert_eq!(
            ShadowsocksConnector::decode_address(&name),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            ShadowsocksConnector::decode_address(&address),
            Some(("198.51.100.7".to_string(), 80))
        );
    }

    /// A server that answers the way the protocol says, for the test.
    ///
    /// The server opens with a salt of its own, reads the address the client
    /// sent, and answers with the salt and the data.
    async fn serve(
        mut socket: tokio::net::TcpStream,
        password: &str,
        answer: &[u8],
    ) -> (String, u16, Vec<u8>) {
        let master = ShadowsocksConnector::master_key(password, KEY_LEN);

        let mut salt = [0u8; SALT_LEN];
        socket.read_exact(&mut salt).await.unwrap();
        let key = ShadowsocksConnector::session_key(&master, &salt, KEY_LEN).unwrap();
        let mut seal = Seal::new(Cipher::ChaCha20Poly1305, &key).unwrap();

        // The address, in the first chunk, and then what the client has to
        // say. The server reads everything before it answers: a socket that
        // is closed with unread data in it sends a reset, and the reset
        // takes the answer with it.
        let address = read_chunk(&mut socket, &mut seal).await;
        let request = read_chunk(&mut socket, &mut seal).await;

        // The answer: our own salt, and then the data.
        let mut server_salt = [0u8; SALT_LEN];
        rand::fill(&mut server_salt);
        socket.write_all(&server_salt).await.unwrap();

        let response_key =
            ShadowsocksConnector::session_key(&master, &server_salt, KEY_LEN).unwrap();
        let mut response_seal = Seal::new(Cipher::ChaCha20Poly1305, &response_key).unwrap();

        write_chunk(&mut socket, &mut response_seal, answer).await;

        let (host, port) = ShadowsocksConnector::decode_address(&address).unwrap();
        (host, port, request)
    }

    /// Read one chunk.
    async fn read_chunk(socket: &mut tokio::net::TcpStream, seal: &mut Seal) -> Vec<u8> {
        let mut header = [0u8; SEALED_LEN];
        socket.read_exact(&mut header).await.unwrap();

        let opened = seal.open(&header).unwrap();
        let length = u16::from_be_bytes([opened[0], opened[1]]) as usize;

        let mut sealed = vec![0u8; length + TAG];
        socket.read_exact(&mut sealed).await.unwrap();

        seal.open(&sealed).unwrap()
    }

    /// Write one chunk.
    async fn write_chunk(socket: &mut tokio::net::TcpStream, seal: &mut Seal, payload: &[u8]) {
        let length = (payload.len() as u16).to_be_bytes();
        let header = seal.seal(&length).unwrap();
        let sealed = seal.seal(payload).unwrap();

        socket.write_all(&header).await.unwrap();
        socket.write_all(&sealed).await.unwrap();
        socket.flush().await.unwrap();
    }

    #[tokio::test]
    async fn the_session_carries_the_address_and_the_data() {
        let password = "a password";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            serve(socket, password, b"the answer").await
        });

        let proxy = ProxyInfo {
            id: "ss".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            protocol: ProxyProtocol::Shadowsocks,
            anonymity: crate::proxy::Anonymity::Unknown,
            latency_ms: None,
            country: None,
            country_reported: None,
            last_checked: None,
            last_success: None,
            first_seen: chrono::Utc::now(),
            next_probe_at: chrono::Utc::now(),
            password: Some(password.into()),
            cipher: Some("chacha20-ietf-poly1305".into()),
            supports_tunnel: true,
            exit_ip: None,
            consecutive_failures: 0,
            score: 0.0,
        };

        let connector = ShadowsocksConnector;
        let target = Target::new("example.com", 443);
        let options = ConnectOptions::default();

        let mut stream = connector.connect(&proxy, &target, &options).await.unwrap();

        // What the client sends goes to the server, and what the server sends
        // comes back.
        stream.write_all(b"the request").await.unwrap();
        stream.flush().await.unwrap();

        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.unwrap();

        let (host, port, seen) = server.await.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(seen, b"the request");
        assert_eq!(answer, b"the answer");
    }
}

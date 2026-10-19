use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::proxy::ProxyProtocol;

pub mod base;
pub mod http;
pub mod http_plain;
pub mod https;
pub mod socks4;
pub mod socks5;

use base::{Carriage, ConnectOptions, Target};

/// A bidirectional byte stream, whatever carries it.
///
/// A tunnel, a TLS session, a SOCKS stream and a Shadowsocks stream are all
/// different types; the router only needs to read and write them.
pub trait ProxyStream: AsyncRead + AsyncWrite + Send + Sync + Unpin {
    /// Read whatever is available, without waiting.
    ///
    /// The router drains its connections on a tick rather than giving each one
    /// a task, so it needs a read that returns immediately. The default polls
    /// once, which is exactly that: a stream with nothing to say reports
    /// `WouldBlock`.
    fn try_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::task::{Context, Poll};

        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut read_buf = tokio::io::ReadBuf::new(buf);

        match AsyncRead::poll_read(Pin::new(self), &mut cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Ok(read_buf.filled().len()),
            Poll::Ready(Err(e)) => Err(e),
            Poll::Pending => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Send + Sync + Unpin> ProxyStream for T {}

/// A boxed stream.
pub type BoxedStream = Box<dyn ProxyStream>;

/// Opens a stream through a proxy.
///
/// Every protocol is one implementation of this trait, registered in the
/// connector table. Adding a protocol is a new module and one line in the
/// table — not another branch in a match that every caller has to know about.
#[async_trait]
pub trait ProxyConnector: Send + Sync {
    /// The protocol this connector implements.
    fn protocol(&self) -> ProxyProtocol;

    /// What this connector can carry.
    fn carriage(&self) -> Carriage {
        Carriage::Stream
    }

    /// Open a stream to the target through the proxy.
    async fn connect(
        &self,
        proxy: &crate::proxy::ProxyInfo,
        target: &Target,
        options: &ConnectOptions,
    ) -> Result<BoxedStream>;

    /// Whether this connector can carry the given target.
    ///
    /// A proxy that only forwards plain HTTP cannot carry a TLS stream, and
    /// pretending otherwise produces a connection that dies on the first
    /// write.
    fn supports(&self, _target: &Target) -> bool {
        true
    }
}

type Factory = Arc<dyn Fn() -> Arc<dyn ProxyConnector> + Send + Sync>;

/// The table of known connectors.
///
/// Keyed by protocol, so a lookup is a hash and not a chain of comparisons.
/// The table is built once and never changes at runtime.
pub struct ConnectorRegistry {
    connectors: HashMap<ProxyProtocol, Arc<dyn ProxyConnector>>,
}

impl ConnectorRegistry {
    /// Build the registry with every protocol the engine supports.
    pub fn with_defaults() -> Self {
        let mut registry = ConnectorRegistry {
            connectors: HashMap::new(),
        };

        registry.register(ProxyProtocol::Http, http::HttpConnector);
        registry.register(ProxyProtocol::Https, https::HttpsConnector);
        registry.register(ProxyProtocol::Socks5, socks5::Socks5Connector);
        registry.register(ProxyProtocol::Socks4, socks4::Socks4Connector);

        registry
    }

    /// Register an already built connector.
    pub fn insert(&mut self, protocol: ProxyProtocol, connector: Arc<dyn ProxyConnector>) {
        self.connectors.insert(protocol, connector);
    }

    /// Register a connector, replacing any previous one for the protocol.
    pub fn register(&mut self, protocol: ProxyProtocol, connector: impl ProxyConnector + 'static) {
        self.connectors
            .insert(protocol, Arc::new(connector) as Arc<dyn ProxyConnector>);
    }

    /// The connector for a protocol.
    pub fn get(&self, protocol: ProxyProtocol) -> Option<Arc<dyn ProxyConnector>> {
        self.connectors.get(&protocol).cloned()
    }

    /// The protocols the registry can open.
    pub fn protocols(&self) -> Vec<ProxyProtocol> {
        let mut protocols: Vec<ProxyProtocol> = self.connectors.keys().copied().collect();
        protocols.sort_by_key(|p| format!("{p:?}"));
        protocols
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// A registry of connector factories, for callers that need to build their
/// own connectors at runtime.
pub struct ConnectorFactories {
    factories: HashMap<ProxyProtocol, Factory>,
}

impl ConnectorFactories {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        protocol: ProxyProtocol,
        factory: impl Fn() -> Arc<dyn ProxyConnector> + Send + Sync + 'static,
    ) {
        self.factories.insert(protocol, Arc::new(factory));
    }

    pub fn build(&self) -> ConnectorRegistry {
        let mut registry = ConnectorRegistry {
            connectors: HashMap::new(),
        };
        for (protocol, factory) in &self.factories {
            registry.insert(*protocol, factory());
        }
        registry
    }
}

impl Default for ConnectorFactories {
    fn default() -> Self {
        Self::new()
    }
}

use crate::forwarder::ForwardConnection;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct FlowKey {
    pub src_ip: Ipv4Addr,
    pub src_port: u16,
    pub dst_ip: Ipv4Addr,
    pub dst_port: u16,
}

#[derive(Debug, Clone)]
pub struct TcpState {
    pub client_isn: u32,
    pub server_isn: u32,
    pub client_next_seq: u32,
    pub server_next_seq: u32,
}

pub struct TrackedConnection {
    pub conn: ForwardConnection,
    pub state: TcpState,
}

pub struct ConnectionTracker {
    inner: Arc<RwLock<HashMap<FlowKey, Arc<RwLock<TrackedConnection>>>>>,
}

impl ConnectionTracker {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    pub async fn insert(&self, key: FlowKey, conn: ForwardConnection, state: TcpState) {
        self.inner.write().await.insert(key, Arc::new(RwLock::new(TrackedConnection { conn, state })));
    }

    pub async fn get(&self, key: &FlowKey) -> Option<Arc<RwLock<TrackedConnection>>> {
        self.inner.read().await.get(key).cloned()
    }

    pub async fn remove(&self, key: &FlowKey) {
        self.inner.write().await.remove(key);
    }

    pub async fn keys(&self) -> Vec<FlowKey> {
        self.inner.read().await.keys().cloned().collect()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

impl Default for ConnectionTracker {
    fn default() -> Self {
        Self::new()
    }
}

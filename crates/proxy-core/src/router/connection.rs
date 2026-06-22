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

pub struct ConnectionTracker {
    inner: Arc<RwLock<HashMap<FlowKey, Arc<RwLock<ForwardConnection>>>>>,
}

impl ConnectionTracker {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    pub async fn insert(&self, key: FlowKey, conn: ForwardConnection) {
        self.inner.write().await.insert(key, Arc::new(RwLock::new(conn)));
    }

    pub async fn get(&self, key: &FlowKey) -> Option<Arc<RwLock<ForwardConnection>>> {
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
}

impl Default for ConnectionTracker {
    fn default() -> Self {
        Self::new()
    }
}

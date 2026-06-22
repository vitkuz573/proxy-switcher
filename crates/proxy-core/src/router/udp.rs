use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

const UDP_FLOW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct UdpFlowKey {
    pub src_ip: Ipv4Addr,
    pub src_port: u16,
    pub dst_ip: Ipv4Addr,
    pub dst_port: u16,
}

struct UdpFlowEntry {
    socket: Arc<tokio::net::UdpSocket>,
    last_active: Instant,
}

pub struct UdpTracker {
    inner: Arc<Mutex<HashMap<UdpFlowKey, UdpFlowEntry>>>,
    fwmark: u32,
}

impl UdpTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            fwmark: 0,
        }
    }

    pub fn with_fwmark(fwmark: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            fwmark,
        }
    }

    /// Apply SO_MARK to a socket fd to bypass policy routing.
    fn apply_fwmark(fd: std::os::unix::io::RawFd, mark: u32) {
        if mark == 0 {
            return;
        }
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_MARK,
                &mark as *const _ as *const libc::c_void,
                std::mem::size_of::<u32>() as libc::socklen_t,
            );
        }
    }

    /// Send data to destination, creating a new UDP flow if needed.
    pub async fn send_or_create(
        &self,
        key: &UdpFlowKey,
        dest: SocketAddr,
        data: &[u8],
    ) -> std::io::Result<()> {
        let socket = {
            let mut inner = self.inner.lock().await;
            if let Some(entry) = inner.get_mut(key) {
                entry.last_active = Instant::now();
                entry.socket.clone()
            } else {
                let socket = Arc::new(tokio::net::UdpSocket::bind("0.0.0.0:0").await?);
                Self::apply_fwmark(socket.as_raw_fd(), self.fwmark);
                socket.connect(dest).await?;
                inner.insert(
                    key.clone(),
                    UdpFlowEntry {
                        socket: socket.clone(),
                        last_active: Instant::now(),
                    },
                );
                socket
            }
        };
        socket.send(data).await?;
        Ok(())
    }

    /// Non-blocking recv on all tracked UDP flows.
    pub async fn recv_all(&self) -> Vec<(UdpFlowKey, Vec<u8>)> {
        let mut results = Vec::new();
        let mut to_remove = Vec::new();

        let sockets: Vec<(UdpFlowKey, Arc<tokio::net::UdpSocket>)> = {
            let inner = self.inner.lock().await;
            inner
                .iter()
                .map(|(k, e)| (k.clone(), e.socket.clone()))
                .collect()
        };

        for (key, socket) in sockets {
            let mut buf = vec![0u8; 65535];
            match socket.try_recv(&mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    results.push((key.clone(), buf));
                    let mut inner = self.inner.lock().await;
                    if let Some(entry) = inner.get_mut(&key) {
                        entry.last_active = Instant::now();
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    to_remove.push(key);
                }
            }
        }

        if !to_remove.is_empty() {
            let mut inner = self.inner.lock().await;
            for key in to_remove {
                inner.remove(&key);
            }
        }

        results
    }

    pub async fn cleanup_stale(&self) {
        let mut inner = self.inner.lock().await;
        inner.retain(|_, entry| entry.last_active.elapsed() < UDP_FLOW_TIMEOUT);
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

impl Default for UdpTracker {
    fn default() -> Self {
        Self::new()
    }
}

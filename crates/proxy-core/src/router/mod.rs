pub mod connection;

use crate::dns::DnsCache;
use crate::forwarder::Forwarder;
use crate::packet::{build_tcp_packet, ParsedPacket};
use crate::pool::ProxyPool;
use connection::{ConnectionTracker, FlowKey, TcpState};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

pub struct Router {
    pub tracker: ConnectionTracker,
    pool: Arc<ProxyPool>,
    dns_cache: Arc<DnsCache>,
}

impl Router {
    pub fn new(pool: Arc<ProxyPool>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
        }
    }

    pub fn with_dns_cache(pool: Arc<ProxyPool>, dns_cache: Arc<DnsCache>) -> Self {
        Self { tracker: ConnectionTracker::new(), pool, dns_cache }
    }

    pub fn dns_cache(&self) -> Arc<DnsCache> {
        self.dns_cache.clone()
    }

    /// Handle a SYN packet: open proxy tunnel, store TCP state, return SYN-ACK.
    pub async fn handle_outgoing(&self, packet: &ParsedPacket) -> Option<Vec<u8>> {
        if !packet.is_tcp_syn() {
            return None;
        }
        let ip = &packet.ip;
        let tcp = packet.tcp.as_ref().unwrap();

        let key = FlowKey {
            src_ip: ip.source,
            src_port: tcp.source_port,
            dst_ip: ip.destination,
            dst_port: tcp.destination_port,
        };

        // If flow already tracked, resend SYN-ACK with the existing ISN
        // instead of creating a duplicate proxy connection.
        if let Some(tracked) = self.tracker.get(&key).await {
            let guard = tracked.read().await;
            debug!("Resending SYN-ACK for tracked flow {:?}", key);
            return Some(build_tcp_packet(
                ip.destination,
                ip.source,
                tcp.destination_port,
                tcp.source_port,
                guard.state.server_isn,
                guard.state.client_isn.wrapping_add(1),
                0x12,
                &[],
            ));
        }

        let proxy = match self.pool.active().await {
            Some(p) => p,
            None => {
                warn!("No active proxy");
                return None;
            }
        };

        // Use hostname from DNS cache if available, fall back to IP
        let target_host = self.dns_cache.lookup(key.dst_ip).await
            .unwrap_or_else(|| key.dst_ip.to_string());

        debug!(
            "New conn: {}:{} -> {}:{} (target={})",
            key.src_ip, key.src_port, key.dst_ip, key.dst_port, target_host
        );

        match Forwarder::connect_to(&proxy, &target_host, key.dst_port).await {
            Ok(conn) => {
                let server_isn = rand::random::<u32>();
                let client_isn = tcp.sequence_number;
                let state = TcpState {
                    client_isn,
                    server_isn,
                    client_next_seq: client_isn.wrapping_add(1),
                    server_next_seq: server_isn.wrapping_add(1),
                };
                self.tracker.insert(key, conn, state).await;

                Some(build_tcp_packet(
                    ip.destination,
                    ip.source,
                    tcp.destination_port,
                    tcp.source_port,
                    server_isn,
                    client_isn.wrapping_add(1),
                    0x12,
                    &[],
                ))
            }
            Err(e) => {
                warn!("Proxy connect failed: {e}");
                None
            }
        }
    }

    /// Forward data payload through tracked proxy connection.
    /// Tracks TCP sequence number progression and cleans up on FIN/RST.
    pub async fn handle_data(&self, packet: &ParsedPacket) {
        let ip = &packet.ip;
        let tcp = match &packet.tcp {
            Some(t) => t,
            None => return,
        };

        let key = FlowKey {
            src_ip: ip.source,
            src_port: tcp.source_port,
            dst_ip: ip.destination,
            dst_port: tcp.destination_port,
        };

        if let Some(tracked) = self.tracker.get(&key).await {
            let mut guard = tracked.write().await;
            if !packet.payload.is_empty() {
                debug!("Forwarding {} bytes to proxy", packet.payload.len());
                if let Err(e) = guard.conn.write_all(&packet.payload).await {
                    debug!("Write error, closing: {e}");
                    drop(guard);
                    self.tracker.remove(&key).await;
                    return;
                }
                // Flush to ensure data is sent immediately
                let _ = guard.conn.flush().await;
                guard.state.client_next_seq =
                    tcp.sequence_number.wrapping_add(packet.payload.len() as u32);
            }
        }

        if packet.is_tcp_fin() {
            self.tracker.remove(&key).await;
        }
    }

    /// Read data from all proxy connections and build response IP packets
    /// with correct TCP sequence numbers.
    /// Uses non-blocking reads — skips connections with no data available.
    pub async fn pump_responses(&self) -> Vec<Vec<u8>> {
        let mut responses = Vec::new();
        for key in self.tracker.keys().await {
            let tracked = match self.tracker.get(&key).await {
                Some(c) => c,
                None => continue,
            };
            let mut guard = tracked.write().await;
            let mut buf = vec![0u8; 65536];
            match guard.conn.try_read(&mut buf) {
                Ok(n) => {
                    if n == 0 {
                        debug!("Pump: EOF for {key:?}, removing");
                        drop(guard);
                        self.tracker.remove(&key).await;
                    } else {
                        buf.truncate(n);
                        debug!("Pump: read {n} bytes for {key:?}");
                        let pkt = build_tcp_packet(
                            key.dst_ip,
                            key.src_ip,
                            key.dst_port,
                            key.src_port,
                            guard.state.server_next_seq,
                            guard.state.client_next_seq,
                            0x18,
                            &buf,
                        );
                        guard.state.server_next_seq =
                            guard.state.server_next_seq.wrapping_add(n as u32);
                        responses.push(pkt);
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // No data available yet, skip
                }
                Err(e) => {
                    debug!("Pump: read error for {key:?}: {e}, removing");
                    drop(guard);
                    self.tracker.remove(&key).await;
                }
            }
        }
        responses
    }
}

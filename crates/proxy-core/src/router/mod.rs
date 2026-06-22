pub mod connection;
pub mod udp;

use crate::dns::DnsCache;
use crate::forwarder::Forwarder;
use crate::packet::{build_tcp_packet, build_udp_packet, ParsedPacket};
use crate::pool::ProxyPool;
use connection::{ConnectionTracker, FlowKey, TcpState};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};
use udp::{UdpFlowKey, UdpTracker};

pub struct Router {
    pub tracker: ConnectionTracker,
    udp_tracker: UdpTracker,
    pool: Arc<ProxyPool>,
    dns_cache: Arc<DnsCache>,
}

impl Router {
    pub fn new(pool: Arc<ProxyPool>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::new(),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
        }
    }

    pub fn with_dns_cache(pool: Arc<ProxyPool>, dns_cache: Arc<DnsCache>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::new(),
            pool,
            dns_cache,
        }
    }

    pub fn with_fwmark(pool: Arc<ProxyPool>, fwmark: u32) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::with_fwmark(fwmark),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
        }
    }

    pub fn dns_cache(&self) -> Arc<DnsCache> {
        self.dns_cache.clone()
    }

    pub fn udp_tracker(&self) -> &UdpTracker {
        &self.udp_tracker
    }

    // ── TCP ────────────────────────────────────────────────────────────

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
                let _ = guard.conn.flush().await;
                guard.state.client_next_seq =
                    tcp.sequence_number.wrapping_add(packet.payload.len() as u32);
            }
        }

        if packet.is_tcp_fin() {
            self.tracker.remove(&key).await;
        }
    }

    /// Non-blocking read from all TCP proxy connections → build response packets.
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
                        debug!("Pump: EOF for {:?}, removing", key);
                        drop(guard);
                        self.tracker.remove(&key).await;
                    } else {
                        buf.truncate(n);
                        debug!("Pump: read {} bytes for {:?}", n, key);
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
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    debug!("Pump: read error for {:?}: {}, removing", key, e);
                    drop(guard);
                    self.tracker.remove(&key).await;
                }
            }
        }
        responses
    }

    // ── UDP ────────────────────────────────────────────────────────────

    /// Handle a UDP packet: forward payload to destination via UDP socket.
    /// DNS responses are parsed and cached for TCP hostname resolution.
    /// Returns the number of bytes sent.
    pub async fn handle_udp(&self, packet: &ParsedPacket) {
        if packet.payload.len() < 8 {
            return;
        }
        let src_port = u16::from_be_bytes([packet.payload[0], packet.payload[1]]);
        let dst_port = u16::from_be_bytes([packet.payload[2], packet.payload[3]]);
        let udp_payload = &packet.payload[8..];

        if udp_payload.is_empty() {
            return;
        }

        let key = UdpFlowKey {
            src_ip: packet.ip.source,
            src_port,
            dst_ip: packet.ip.destination,
            dst_port,
        };

        let dest = SocketAddr::new(
            std::net::IpAddr::V4(packet.ip.destination),
            dst_port,
        );

        if let Err(e) = self.udp_tracker.send_or_create(&key, dest, udp_payload).await {
            debug!("UDP send error: {e}");
        }
    }

    /// Read all available UDP responses, build response packets, and
    /// cache DNS A records for TCP forwarding.
    pub async fn pump_udp(&self) -> Vec<Vec<u8>> {
        let mut responses = Vec::new();
        let results = self.udp_tracker.recv_all().await;

        for (key, payload) in results {
            // If this was a DNS query (outgoing dst_port == 53),
            // parse the response for A records and cache them
            if key.dst_port == 53 {
                let mappings = crate::dns::parse_dns_response(&payload);
                for (hostname, ip) in mappings {
                    debug!("DNS cache: {} -> {}", ip, hostname);
                    self.dns_cache.insert(ip, hostname).await;
                }
            }

            let pkt = build_udp_packet(
                key.dst_ip,
                key.src_ip,
                key.dst_port,
                key.src_port,
                &payload,
            );
            responses.push(pkt);
        }

        responses
    }

    /// Remove stale UDP flows that have exceeded the idle timeout.
    pub async fn cleanup_udp(&self) {
        self.udp_tracker.cleanup_stale().await;
    }

    pub async fn active_tcp_conns(&self) -> usize {
        self.tracker.len().await
    }

    pub async fn active_udp_flows(&self) -> usize {
        self.udp_tracker.len().await
    }

    pub async fn dns_cache_entries(&self) -> Vec<(std::net::Ipv4Addr, String)> {
        self.dns_cache.entries().await
    }
}

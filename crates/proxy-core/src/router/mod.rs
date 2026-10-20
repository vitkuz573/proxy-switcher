pub mod connection;
pub mod udp;

use crate::dns::DnsCache;
use crate::forwarder::Forwarder;
use crate::packet::{build_tcp_packet, build_udp_packet, ParsedPacket};
use crate::pool::ProxyPool;
use crate::routing::{Decision, RoutePolicy};
use connection::{ConnectionTracker, FlowKey, TcpState};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};
use udp::{UdpFlowKey, UdpTracker};

pub struct Router {
    pub tracker: ConnectionTracker,
    udp_tracker: UdpTracker,
    pool: Arc<ProxyPool>,
    dns_cache: Arc<DnsCache>,
    /// Which destinations go through a proxy.
    policy: RoutePolicy,
    /// Opens the connections the router forwards.
    forwarder: Forwarder,
    /// The addresses the engine pointed at the device.
    ///
    /// The device is not the default route: the engine routes the policy's
    /// destinations into it and nothing else. An address that arrives here
    /// was put here on purpose, and the policy is the reason for it.
    routed: Arc<tokio::sync::RwLock<std::collections::HashSet<std::net::Ipv4Addr>>>,
    /// Where the response packets go.
    ///
    /// The tunnel is opened in the background, so the answer to a handshake
    /// can arrive after the loop has moved on. The device belongs to the
    /// loop, so the packets are handed to it through a channel.
    responder: Arc<tokio::sync::RwLock<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>,
    /// The tunnels the router has opened since it started.
    ///
    /// The live count says what is open right now, which is usually nothing:
    /// a request goes through in a fraction of a second. The total says how
    /// much the device has carried, which is the number an operator wants.
    tunnels_opened: Arc<std::sync::atomic::AtomicU64>,
    /// The bytes carried towards the destination.
    bytes_out: Arc<std::sync::atomic::AtomicU64>,
    /// The bytes carried back to the client.
    bytes_in: Arc<std::sync::atomic::AtomicU64>,
    /// The flows whose tunnel is being opened.
    ///
    /// A client that retransmits its handshake must not open a second
    /// tunnel: the first one is still on its way.
    pending: Arc<tokio::sync::Mutex<std::collections::HashSet<FlowKey>>>,
    /// Firewall mark for the sockets the router opens.
    ///
    /// The router's own connection to the proxy must not re-enter the TUN: a
    /// connection to the proxy that is itself routed through a proxy is a
    /// loop, and it is how the engine takes itself offline.
    mark: Option<u32>,
}

impl Router {
    pub fn new(pool: Arc<ProxyPool>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::new(),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
            forwarder: Forwarder::new(),
            policy: RoutePolicy::default(),
            routed: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            responder: Arc::new(tokio::sync::RwLock::new(
                tokio::sync::mpsc::unbounded_channel().0,
            )),
            pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            tunnels_opened: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_out: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_in: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mark: None,
        }
    }

    /// Build a router with an explicit routing policy and socket mark.
    pub fn with_policy(pool: Arc<ProxyPool>, policy: RoutePolicy, mark: Option<u32>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::new(),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
            forwarder: Forwarder::new(),
            policy,
            routed: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            responder: Arc::new(tokio::sync::RwLock::new(
                tokio::sync::mpsc::unbounded_channel().0,
            )),
            pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            tunnels_opened: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_out: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_in: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mark,
        }
    }

    pub fn policy(&self) -> &RoutePolicy {
        &self.policy
    }

    pub fn with_dns_cache(pool: Arc<ProxyPool>, dns_cache: Arc<DnsCache>) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::new(),
            pool,
            dns_cache,
            forwarder: Forwarder::new(),
            policy: RoutePolicy::default(),
            routed: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            responder: Arc::new(tokio::sync::RwLock::new(
                tokio::sync::mpsc::unbounded_channel().0,
            )),
            pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            tunnels_opened: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_out: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_in: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mark: None,
        }
    }

    pub fn with_fwmark(pool: Arc<ProxyPool>, fwmark: u32) -> Self {
        Self {
            tracker: ConnectionTracker::new(),
            udp_tracker: UdpTracker::with_fwmark(fwmark),
            pool,
            dns_cache: Arc::new(DnsCache::new()),
            forwarder: Forwarder::new(),
            policy: RoutePolicy::default(),
            routed: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            responder: Arc::new(tokio::sync::RwLock::new(
                tokio::sync::mpsc::unbounded_channel().0,
            )),
            pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            tunnels_opened: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_out: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_in: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mark: Some(fwmark),
        }
    }

    pub fn dns_cache(&self) -> Arc<DnsCache> {
        self.dns_cache.clone()
    }

    pub fn udp_tracker(&self) -> &UdpTracker {
        &self.udp_tracker
    }

    /// Tell the router which addresses the engine routed into the device.
    ///
    /// The set is the truth about what the device is carrying: an address
    /// that arrives here is one the policy covers, whether or not the router
    /// happens to know the name behind it.
    pub async fn set_routed_destinations(
        &self,
        addresses: std::collections::HashSet<std::net::Ipv4Addr>,
    ) {
        *self.routed.write().await = addresses;
    }

    /// Hand the response packets to the loop that owns the device.
    pub async fn set_responder(&self, sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        *self.responder.write().await = sender;
    }

    /// Send a response packet to the loop that owns the device.
    async fn respond(&self, packet: Vec<u8>) {
        let sender = self.responder.read().await.clone();
        let _ = sender.send(packet);
    }

    /// How many tunnels the router has opened since it started.
    pub fn tunnels_opened(&self) -> u64 {
        self.tunnels_opened
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many bytes the router carried towards the destination.
    pub fn bytes_out(&self) -> u64 {
        self.bytes_out.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many bytes the router carried back to the client.
    pub fn bytes_in(&self) -> u64 {
        self.bytes_in.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many addresses the engine routed into the device.
    pub async fn routed_destination_count(&self) -> usize {
        self.routed.read().await.len()
    }

    // ── TCP ────────────────────────────────────────────────────────────

    /// Handle a SYN packet: open proxy tunnel, store TCP state, return SYN-ACK.
    ///
    /// The tunnel is opened in the background. A proxy that does not answer
    /// would otherwise hold the whole device still — every other connection
    /// would wait for it — and a dead proxy is the normal case, not the
    /// exception. The handshake's answer travels back through the responder.
    pub async fn handle_outgoing(self: &Arc<Self>, packet: &ParsedPacket) -> Option<Vec<u8>> {
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

        // A handshake that is already on its way must not start a second
        // tunnel: the client retransmits until it is answered, and every
        // retransmission would open another connection to the proxy.
        {
            let mut pending = self.pending.lock().await;
            if !pending.insert(key.clone()) {
                debug!("Handshake already in flight for {:?}", key);
                return None;
            }
        }

        // Use hostname from DNS cache if available, fall back to IP
        let target_host = self
            .dns_cache
            .lookup(key.dst_ip)
            .await
            .unwrap_or_else(|| key.dst_ip.to_string());

        // Which traffic a proxy applies to. The default routes nothing, so a
        // proxy is an opt-in for specific destinations rather than a default
        // route for the whole host.
        let decision = self
            .decide(key.dst_ip, key.dst_port, Some(target_host.as_str()))
            .await;

        // The proxy's own address is never routed through the proxy.
        //
        // It is replaced when the proxy turns out to be dead and another one
        // takes over.
        let mut proxy = match decision {
            Decision::Direct => None,
            Decision::Proxy => self.pool.active().await,
        };

        if decision == Decision::Proxy && proxy.is_none() {
            // The policy says this traffic goes through a proxy and there is
            // none to send it to. Letting it out direct would undo the
            // policy silently, and the operator would never learn that their
            // traffic was not protected. The connection is refused instead:
            // the destinations the policy covers fail, and the rest of the
            // host is untouched.
            warn!("No active proxy for {target_host}, refusing the connection");
            self.pending.lock().await.remove(&key);
            self.respond(build_tcp_packet(
                ip.destination,
                ip.source,
                tcp.destination_port,
                tcp.source_port,
                0,
                tcp.sequence_number.wrapping_add(1),
                0x14,
                &[],
            ))
            .await;
            return None;
        }

        debug!(
            "New conn: {}:{} -> {}:{} (target={})",
            key.src_ip, key.src_port, key.dst_ip, key.dst_port, target_host
        );

        let router = self.clone();
        let client_isn = tcp.sequence_number;
        let source = ip.source;
        let destination = ip.destination;
        let source_port = tcp.source_port;
        let destination_port = tcp.destination_port;

        tokio::spawn(async move {
            // The connection is tried through the active proxy, and once more
            // through another one when that proxy turns out to be dead.
            //
            // A public proxy dies without warning, and the traffic it was
            // carrying has nowhere else to go: the policy says these
            // destinations are proxied, so letting them out direct would
            // undo the policy. Another proxy is tried instead, and the dead
            // one is recorded so it stops being a candidate.
            let mut attempt = 0;
            let mut connection = None;
            let mut last_error = None;
            // The proxies that could not carry the connection. None of them is
            // tried again.
            let mut avoided: std::collections::HashSet<String> = std::collections::HashSet::new();

            loop {
                let outcome = match proxy.as_ref() {
                    Some(proxy) => {
                        router
                            .forwarder
                            .connect_to(proxy, &target_host, destination_port, router.mark)
                            .await
                    }
                    None => {
                        router
                            .forwarder
                            .connect_direct(&target_host, destination_port, router.mark)
                            .await
                    }
                };

                match outcome {
                    Ok(conn) => {
                        connection = Some(conn);
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);

                        let Some(failed) = proxy.as_ref() else {
                            break;
                        };

                        // Three tries: a public proxy dies without warning,
                        // and the one in front is often dead long before the
                        // health check reaches it.
                        if attempt >= 2 {
                            break;
                        }
                        attempt += 1;

                        // The proxy is recorded as failed and stepped aside.
                        router.pool.report_failure(&failed.id).await;
                        avoided.insert(failed.id.clone());
                        let replacement = router
                            .pool
                            .activate_best_avoiding(&avoided, &std::collections::HashSet::new())
                            .await;

                        match replacement {
                            Some(next) => {
                                warn!(
                                    "the proxy {} could not carry the connection, trying {}",
                                    failed.id, next.id
                                );
                                proxy = Some(next);
                            }
                            None => {
                                warn!(
                                    "the proxy {} could not carry the connection and no other proxy is available",
                                    failed.id
                                );
                                break;
                            }
                        }
                    }
                }
            }

            router.pending.lock().await.remove(&key);

            match connection {
                Some(conn) => {
                    let server_isn = rand::random::<u32>();
                    let state = TcpState {
                        client_isn,
                        server_isn,
                        client_next_seq: client_isn.wrapping_add(1),
                        server_next_seq: server_isn.wrapping_add(1),
                    };
                    router.tracker.insert(key, conn, state).await;
                    router
                        .tunnels_opened
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    router
                        .respond(build_tcp_packet(
                            destination,
                            source,
                            destination_port,
                            source_port,
                            server_isn,
                            client_isn.wrapping_add(1),
                            0x12,
                            &[],
                        ))
                        .await;
                }
                None => {
                    // The client is waiting for an answer that will not come.
                    // A refusal ends the wait at once, and the client can try
                    // again instead of hanging until it gives up.
                    if let Some(e) = last_error {
                        warn!("Proxy connect failed: {e}");
                    }
                    router
                        .respond(build_tcp_packet(
                            destination,
                            source,
                            destination_port,
                            source_port,
                            0,
                            client_isn.wrapping_add(1),
                            0x14,
                            &[],
                        ))
                        .await;
                }
            }
        });

        None
    }

    /// Decide how a connection that arrived at the device should go.
    ///
    /// The address the engine routed is the strongest signal: the engine
    /// put it in the routing table because the policy covers it, and a
    /// policy written in names has to work even when the router never
    /// learned the name. The exemptions still come first — a local address
    /// or one the policy never proxies stays out of the proxy.
    async fn decide(&self, ip: std::net::Ipv4Addr, port: u16, hostname: Option<&str>) -> Decision {
        let address = IpAddr::V4(ip);

        if crate::routing::is_local(address) {
            return Decision::Direct;
        }

        if self.policy.matches_always_direct(address, port) {
            return Decision::Direct;
        }

        if self.routed.read().await.contains(&ip) {
            return Decision::Proxy;
        }

        self.policy.decide(address, port, hostname)
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
                self.bytes_out.fetch_add(
                    packet.payload.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                debug!("Forwarding {} bytes to proxy", packet.payload.len());
                if let Err(e) = guard.conn.write_all(&packet.payload).await {
                    debug!("Write error, closing: {e}");
                    drop(guard);
                    self.tracker.remove(&key).await;
                    return;
                }
                let _ = guard.conn.flush().await;
                guard.state.client_next_seq = tcp
                    .sequence_number
                    .wrapping_add(packet.payload.len() as u32);
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
                        self.bytes_in
                            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
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

        let dest = SocketAddr::new(std::net::IpAddr::V4(packet.ip.destination), dst_port);

        if let Err(e) = self
            .udp_tracker
            .send_or_create(&key, dest, udp_payload)
            .await
        {
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

            let pkt =
                build_udp_packet(key.dst_ip, key.src_ip, key.dst_port, key.src_port, &payload);
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

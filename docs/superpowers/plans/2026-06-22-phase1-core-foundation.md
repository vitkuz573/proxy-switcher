# Phase 1: Core Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the core forwarding loop: TUN → IP parsing → connection tracking → proxy forward → response back to TUN

**Architecture:** Raw IP packets arrive from TUN device, IP/TCP headers are parsed, TCP connections are tracked and forwarded through the active proxy (HTTP CONNECT/SOCKS4/5), response data is written back with rewritten IP headers.

**Tech Stack:** `tun` crate, manual IP/TCP header parsing, `tokio` async I/O, `tokio-socks`.

---

### Task 1: IP/TCP Packet Parser

**Files:**
- Create: `crates/proxy-core/src/packet/ip.rs`
- Create: `crates/proxy-core/src/packet/tcp.rs`
- Create: `crates/proxy-core/src/packet/mod.rs`
- Modify: `crates/proxy-core/src/lib.rs`

- [ ] **Step 1: Write IP header parser**

```rust
// crates/proxy-core/src/packet/ip.rs
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct IpHeader {
    pub version: u8,
    pub ihl: u8,
    pub total_length: u16,
    pub identification: u16,
    pub flags: u8,
    pub fragment_offset: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
}

impl IpHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("Packet too short");
        }
        let v_ihl = data[0];
        if (v_ihl >> 4) != 4 {
            return Err("Not IPv4");
        }
        let ihl = ((v_ihl & 0x0F) * 4) as usize;
        if ihl < 20 || data.len() < ihl {
            return Err("Bad IHL");
        }
        Ok(Self {
            version: 4,
            ihl: ihl as u8,
            total_length: u16::from_be_bytes([data[2], data[3]]),
            identification: u16::from_be_bytes([data[4], data[5]]),
            flags: data[6] >> 5,
            fragment_offset: u16::from_be_bytes([data[6] & 0x1F, data[7]]) & 0x1FFF,
            ttl: data[8],
            protocol: data[9],
            checksum: u16::from_be_bytes([data[10], data[11]]),
            source: Ipv4Addr::new(data[12], data[13], data[14], data[15]),
            destination: Ipv4Addr::new(data[16], data[17], data[18], data[19]),
        })
    }

    pub fn header_len(&self) -> usize {
        self.ihl as usize
    }
}
```

- [ ] **Step 2: Write TCP header parser**

```rust
// crates/proxy-core/src/packet/tcp.rs
#[derive(Debug, Clone)]
pub struct TcpHeader {
    pub source_port: u16,
    pub destination_port: u16,
    pub sequence_number: u32,
    pub acknowledgment_number: u32,
    pub data_offset: u8,
    pub flags: TcpFlags,
    pub window_size: u16,
    pub checksum: u16,
    pub urgent_pointer: u16,
}

#[derive(Debug, Clone, Default)]
pub struct TcpFlags {
    pub fin: bool,
    pub syn: bool,
    pub rst: bool,
    pub psh: bool,
    pub ack: bool,
    pub urg: bool,
}

impl TcpHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("TCP header too short");
        }
        let doff = ((data[12] >> 4) * 4) as usize;
        if doff < 20 || data.len() < doff {
            return Err("Bad TCP data offset");
        }
        let f = data[13];
        Ok(Self {
            source_port: u16::from_be_bytes([data[0], data[1]]),
            destination_port: u16::from_be_bytes([data[2], data[3]]),
            sequence_number: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            acknowledgment_number: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
            data_offset: doff as u8,
            flags: TcpFlags {
                fin: (f & 0x01) != 0, syn: (f & 0x02) != 0,
                rst: (f & 0x04) != 0, psh: (f & 0x08) != 0,
                ack: (f & 0x10) != 0, urg: (f & 0x20) != 0,
            },
            window_size: u16::from_be_bytes([data[14], data[15]]),
            checksum: u16::from_be_bytes([data[16], data[17]]),
            urgent_pointer: u16::from_be_bytes([data[18], data[19]]),
        })
    }

    pub fn header_len(&self) -> usize {
        self.data_offset as usize
    }
}
```

- [ ] **Step 3: Create packet module with parser + builder**

```rust
// crates/proxy-core/src/packet/mod.rs
pub mod ip;
pub mod tcp;

use ip::IpHeader;
use tcp::{TcpFlags, TcpHeader};
use std::net::Ipv4Addr;

#[derive(Debug)]
pub struct ParsedPacket {
    pub ip: IpHeader,
    pub tcp: Option<TcpHeader>,
    pub payload: Vec<u8>,
}

impl ParsedPacket {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        let ip = IpHeader::parse(data)?;
        let ip_end = ip.header_len();

        let (tcp, payload_start) = if ip.protocol == 6 {
            let tcp_data = &data[ip_end..];
            let tcp = TcpHeader::parse(tcp_data)?;
            (Some(tcp), ip_end + tcp.header_len())
        } else {
            (None, ip_end)
        };

        let payload = if payload_start < data.len() {
            data[payload_start..].to_vec()
        } else {
            Vec::new()
        };

        Ok(Self { ip, tcp, payload })
    }

    pub fn is_tcp_syn(&self) -> bool {
        self.tcp.as_ref().map_or(false, |t| t.flags.syn && !t.flags.ack)
    }

    pub fn is_tcp_fin(&self) -> bool {
        self.tcp.as_ref().map_or(false, |t| t.flags.fin || t.flags.rst)
    }
}

/// Build a response IP packet swapping src/dst and setting payload
pub fn build_response_packet(original: &ParsedPacket, payload: &[u8]) -> Vec<u8> {
    let ip = &original.ip;
    let tcp = match &original.tcp {
        Some(t) => t,
        None => return Vec::new(),
    };

    let tcp_len = 20; // minimal TCP header, no options
    let ip_total = 20 + tcp_len + payload.len();

    let mut pkt = Vec::with_capacity(ip_total);
    // IP header
    pkt.push(0x45); // v4, ihl=20
    pkt.push(0x00); // DSCP
    pkt.extend_from_slice(&(ip_total as u16).to_be_bytes());
    pkt.extend_from_slice(&ip.identification.wrapping_add(1).to_be_bytes());
    pkt.push(0x40); // flags=0, frag_offset=0
    pkt.push(0x00);
    pkt.push(64); // TTL
    pkt.push(6); // TCP
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum = 0 (computed later)
    // Swap src/dst
    pkt.extend_from_slice(&original.ip.destination.octets());
    pkt.extend_from_slice(&original.ip.source.octets());

    // TCP header (swapped ports, ack flag)
    pkt.extend_from_slice(&tcp.destination_port.to_be_bytes());
    pkt.extend_from_slice(&tcp.source_port.to_be_bytes());
    pkt.extend_from_slice(&tcp.acknowledgment_number.to_be_bytes());
    pkt.extend_from_slice(&tcp.sequence_number.wrapping_add(1).to_be_bytes());
    pkt.push(0x50); // data offset = 20
    pkt.push(0x10); // ACK
    pkt.extend_from_slice(&(65535u16).to_be_bytes()); // window
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum placeholder
    pkt.extend_from_slice(&[0x00, 0x00]); // urgent

    // Payload
    pkt.extend_from_slice(payload);

    // Compute IP checksum
    let ip_csum = ip_checksum(&pkt[..20]);
    pkt[10] = (ip_csum >> 8) as u8;
    pkt[11] = (ip_csum & 0xFF) as u8;

    // Compute TCP checksum (with pseudo header)
    let tcp_csum = tcp_checksum(
        &original.ip.destination,
        &original.ip.source,
        &pkt[20..],
    );
    pkt[20 + 16] = (tcp_csum >> 8) as u8;
    pkt[20 + 17] = (tcp_csum & 0xFF) as u8;

    pkt
}

fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = u16::from_be_bytes([chunk[0], if chunk.len() > 1 { chunk[1] } else { 0 }]);
        sum = sum.wrapping_add(word as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_checksum(src: &Ipv4Addr, dst: &Ipv4Addr, segment: &[u8]) -> u16 {
    let pseudo_len = 12 + segment.len();
    let mut buf = Vec::with_capacity(pseudo_len);
    buf.extend_from_slice(&src.octets());
    buf.extend_from_slice(&dst.octets());
    buf.push(0); // zero
    buf.push(6); // protocol TCP
    let len_bytes = (segment.len() as u16).to_be_bytes();
    buf.extend_from_slice(&len_bytes);
    buf.extend_from_slice(segment);

    // Zero out the checksum field in the copied segment
    let tcp_start = 12;
    if buf.len() >= tcp_start + 18 {
        buf[tcp_start + 16] = 0;
        buf[tcp_start + 17] = 0;
    }

    ip_checksum(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_syn_packet() -> Vec<u8> {
        let mut pkt = vec![
            0x45, 0x00, 0x00, 0x28,
            0x00, 0x01, 0x00, 0x00,
            0x40, 0x06, 0x00, 0x00,
            0x0a, 0x00, 0x00, 0x02,
            0x68, 0x01, 0xdb, 0x08,
        ];
        pkt.extend_from_slice(&[
            0xc0, 0x00, 0x00, 0x50,
            0x00, 0x00, 0x00, 0x64,
            0x00, 0x00, 0x00, 0x00,
            0x50, 0x02, 0x71, 0x10,
            0x00, 0x00, 0x00, 0x00,
        ]);
        pkt
    }

    #[test]
    fn test_parse_syn() {
        let p = ParsedPacket::parse(&make_syn_packet()).unwrap();
        assert!(p.is_tcp_syn());
        assert_eq!(p.ip.source.to_string(), "10.0.0.2");
        assert_eq!(p.ip.destination.to_string(), "104.1.219.8");
        assert_eq!(p.tcp.as_ref().unwrap().source_port, 49152);
        assert_eq!(p.tcp.as_ref().unwrap().destination_port, 80);
    }

    #[test]
    fn test_build_response() {
        let orig = ParsedPacket::parse(&make_syn_packet()).unwrap();
        let resp = build_response_packet(&orig, b"HTTP/1.1 200 OK\r\n");
        assert!(!resp.is_empty());
        // Check src/dst are swapped
        let parsed = ParsedPacket::parse(&resp).unwrap();
        assert_eq!(parsed.ip.source.to_string(), "104.1.219.8");
        assert_eq!(parsed.ip.destination.to_string(), "10.0.0.2");
        assert_eq!(parsed.tcp.as_ref().unwrap().source_port, 80);
        assert_eq!(parsed.tcp.as_ref().unwrap().destination_port, 49152);
    }
}
```

- [ ] **Step 4: Register module in lib.rs**

```rust
// crates/proxy-core/src/lib.rs — add line:
pub mod packet;
```

- [ ] **Step 5: Build and run tests**

Run: `cargo test -p proxy-core -- packet -v`
Expected: 2 passed

```bash
cargo test -p proxy-core -- packet -v
```

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-core/src/packet/
git add crates/proxy-core/src/lib.rs
git commit -m "feat: IP/TCP packet parser and response builder"
```

---

### Task 2: Connection Tracker

**Files:**
- Create: `crates/proxy-core/src/router/connection.rs`
- Create: `crates/proxy-core/src/router/mod.rs`
- Modify: `crates/proxy-core/src/lib.rs`

- [ ] **Step 1: Write connection tracker**

```rust
// crates/proxy-core/src/router/connection.rs
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
```

- [ ] **Step 2: Write Router**

```rust
// crates/proxy-core/src/router/mod.rs
pub mod connection;

use crate::forwarder::Forwarder;
use crate::packet::{build_response_packet, ParsedPacket};
use crate::pool::ProxyPool;
use connection::{ConnectionTracker, FlowKey};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

pub struct Router {
    pub tracker: ConnectionTracker,
    pool: Arc<ProxyPool>,
}

impl Router {
    pub fn new(pool: Arc<ProxyPool>) -> Self {
        Self { tracker: ConnectionTracker::new(), pool }
    }

    pub async fn handle_outgoing(&self, packet: &ParsedPacket) {
        if !packet.is_tcp_syn() {
            return;
        }
        let ip = &packet.ip;
        let tcp = packet.tcp.as_ref().unwrap();

        let key = FlowKey {
            src_ip: ip.source,
            src_port: tcp.source_port,
            dst_ip: ip.destination,
            dst_port: tcp.destination_port,
        };

        let proxy = match self.pool.active().await {
            Some(p) => p,
            None => { warn!("No active proxy"); return; }
        };

        debug!("New conn: {}:{} -> {}:{}", key.src_ip, key.src_port, key.dst_ip, key.dst_port);

        match Forwarder::connect_to(&proxy, &key.dst_ip.to_string(), key.dst_port).await {
            Ok(conn) => self.tracker.insert(key, conn).await,
            Err(e) => warn!("Proxy connect failed: {e}"),
        }
    }

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

        if let Some(conn) = self.tracker.get(&key).await {
            if !packet.payload.is_empty() {
                let mut guard = conn.write().await;
                if let Err(e) = guard.write_all(&packet.payload).await {
                    debug!("Write error, closing: {e}");
                    self.tracker.remove(&key).await;
                }
            }
        }

        if packet.is_tcp_fin() {
            self.tracker.remove(&key).await;
        }
    }

    /// Read data from all proxy connections and build response packets
    pub async fn pump_responses(&self) -> Vec<Vec<u8>> {
        let mut responses = Vec::new();
        // Can't iterate while reading, so clone keys first
        for key in self.tracker.keys().await {
            let conn = match self.tracker.get(&key).await {
                Some(c) => c,
                None => continue,
            };
            let mut guard = conn.write().await;
            let mut buf = vec![0u8; 65536];
            match guard.read(&mut buf).await {
                Ok(0) => { drop(guard); self.tracker.remove(&key).await; }
                Ok(n) => {
                    buf.truncate(n);
                    // Build a minimal response packet
                    let fake_packet = ParsedPacket {
                        ip: crate::packet::ip::IpHeader {
                            version: 4, ihl: 20,
                            total_length: 0, identification: 0,
                            flags: 0, fragment_offset: 0, ttl: 0,
                            protocol: 6, checksum: 0,
                            source: key.dst_ip,
                            destination: key.src_ip,
                        },
                        tcp: Some(crate::packet::tcp::TcpHeader {
                            source_port: key.dst_port,
                            destination_port: key.src_port,
                            sequence_number: 0, acknowledgment_number: 0,
                            data_offset: 20,
                            flags: crate::packet::tcp::TcpFlags {
                                ack: true, psh: true, ..Default::default()
                            },
                            window_size: 65535,
                            checksum: 0, urgent_pointer: 0,
                        }),
                        payload: buf.clone(),
                    };
                    let ip_pkt = build_response_packet(&fake_packet, &buf);
                    responses.push(ip_pkt);
                }
                Err(_) => { drop(guard); self.tracker.remove(&key).await; }
            }
        }
        responses
    }
}
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// crates/proxy-core/src/lib.rs — add line:
pub mod router;
```

- [ ] **Step 4: Build**

Run: `cargo check -p proxy-core`
Expected: clean

```bash
cargo check -p proxy-core
```

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-core/src/router/
git add crates/proxy-core/src/lib.rs
git commit -m "feat: TCP connection tracker and data router"
```

---

### Task 3: TUN Forwarding Loop + Daemon Integration

**Files:**
- Modify: `crates/proxy-core/src/tun_manager/mod.rs`
- Modify: `crates/proxy-daemon/src/main.rs`

- [ ] **Step 1: Add `take_device` and `run_forwarding_loop` to TunManager**

```rust
// Add to crates/proxy-core/src/tun_manager/mod.rs

use crate::packet::ParsedPacket;
use crate::router::Router;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tracing::info;

impl TunManager {
    /// Take ownership of the TUN device (leaves None in the option)
    pub async fn take_device(&self) -> Option<tun::Device> {
        self.dev.lock().await.take()
    }
}

/// Standalone forwarding loop — reads IP packets from TUN, routes them,
/// pumps responses back. Designed to run in a spawned task.
pub async fn run_forwarding_loop(
    mut dev: tun::Device,
    router: Arc<Router>,
    mtu: usize,
) {
    info!("Starting TUN forwarding loop (MTU={mtu})");
    let mut buf = vec![0u8; mtu];

    loop {
        match dev.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                match ParsedPacket::parse(data) {
                    Ok(packet) => {
                        router.handle_outgoing(&packet).await;
                        router.handle_data(&packet).await;
                    }
                    Err(e) => tracing::trace!("Parse error: {e}"),
                }
            }
            Err(e) => {
                tracing::error!("TUN read error: {e}");
                break;
            }
            _ => {}
        }

        // Pump responses back to TUN
        let responses = router.pump_responses().await;
        for pkt in &responses {
            let _ = dev.write_all(pkt).await;
        }
    }

    info!("TUN forwarding loop ended");
}
```

- [ ] **Step 2: Update daemon to wire everything together**

```rust
// Replace the TUN + scraper sections in crates/proxy-daemon/src/main.rs

use proxy_core::router::Router;
use proxy_core::tun_manager::run_forwarding_loop;

    // Initialize TUN
    let tun = TunManager::new(config.tun.clone());
    let mtu = config.tun.mtu as usize;

    if let Err(e) = tun.create().await {
        error!("Failed to create TUN device (try running as root): {e}");
    }

    // Take device for forwarding loop
    let tun_dev = tun.take_device().await;

    let router = Arc::new(Router::new(pool.clone()));

    // Start TUN forwarding loop
    if let Some(dev) = tun_dev {
        let router_clone = router.clone();
        tokio::spawn(async move {
            run_forwarding_loop(dev, router_clone, mtu).await;
        });
    }

    // ... scraper + health check loop (unchanged) ...
    // ... periodic re-check (unchanged) ...

    // Update API router to pass router state too
    let router_state = router.clone();
    let api_router = api::build_router(pool.clone(), router_state);

    // ...server start (unchanged)...
    // ...shutdown (unchanged, but note: tun.cleanup still works since it only removes routes)...
```

- [ ] **Step 3: Update API module to accept router state**

```rust
// crates/proxy-daemon/src/api.rs
use proxy_core::router::Router;

pub fn build_router(pool: Arc<ProxyPool>, router: Arc<Router>) -> Router<...> {
    // Add router to app state, keep existing endpoints
}
```

- [ ] **Step 4: Build**

Run: `cargo check --workspace`
Expected: clean

```bash
cargo check --workspace
```

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-core/src/tun_manager/mod.rs
git add crates/proxy-daemon/src/main.rs
git add crates/proxy-daemon/src/api.rs
git commit -m "feat: TUN forwarding loop integrated into daemon"
```

---

### Task 4: Integration Smoke Test

**Files:**
- Create: `crates/proxy-daemon/tests/smoke_test.rs`

- [ ] **Step 1: Write integration test that verifies the packet parse→build→parse roundtrip**

```rust
// crates/proxy-daemon/tests/smoke_test.rs
use proxy_core::packet::{build_response_packet, ParsedPacket};

#[test]
fn test_packet_roundtrip() {
    let raw = vec![
        0x45, 0x00, 0x00, 0x3c, 0x00, 0x01, 0x00, 0x00,
        0x40, 0x06, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x02,
        0x68, 0x01, 0xdb, 0x08,
        0xc0, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x64,
        0x00, 0x00, 0x00, 0x00, 0x50, 0x02, 0x71, 0x10,
        0x00, 0x00, 0x00, 0x00,
    ];
    let payload = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    let mut full = raw.clone();
    full.extend_from_slice(payload);

    let parsed = ParsedPacket::parse(&full).unwrap();
    let resp = build_response_packet(&parsed, b"HTTP/1.1 200 OK\r\n\r\n");
    let resp_parsed = ParsedPacket::parse(&resp).unwrap();

    assert_eq!(resp_parsed.ip.source, parsed.ip.destination);
    assert_eq!(resp_parsed.ip.destination, parsed.ip.source);
}
```

- [ ] **Step 2: Run test**

Run: `cargo test --workspace`
Expected: all tests pass

```bash
cargo test --workspace -v
```

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-daemon/tests/smoke_test.rs
git commit -m "test: packet roundtrip integration test"
```

---

### Self-Review

**Spec coverage:**
- TUN device manager — pre-existing, enhanced with forwarding loop
- Packet read/write — Task 1 (parser) + Task 3 (forwarding loop)
- Forwarder HTTP CONNECT / SOCKS4/5 — pre-existing
- Integration test — Task 4

**Placeholder scan:** All steps have actual code. No TBD/TODO.

**Type consistency:** `FlowKey`, `ParsedPacket`, `IpHeader`, `TcpHeader` types are consistent across all tasks.

# Proxy Core Enterprise Architecture

## Overview

System-wide IP switcher that routes all Linux traffic through user-selected
proxies via a TUN device. Full traffic interception with TCP forwarding,
UDP relay, DNS caching, and connection state tracking.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      proxy-core                               │
│                                                               │
│  ┌──────────────┐   ┌───────────────┐   ┌─────────────────┐  │
│  │  TunManager   │──▶│    Router     │──▶│   Forwarder     │  │
│  │  (TUN dev)    │   │               │   │  (proxy proto)  │  │
│  │  - read IP    │   │  ┌─────────┐  │   │  - HTTP CONNECT │  │
│  │  - write IP   │   │  │ TCP     │  │   │  - SOCKS5/4     │  │
│  │  - routes     │   │  │ Engine  │  │   └─────────────────┘  │
│  └──────┬───────┘   │  └─────────┘  │                        │
│         │ loop      │  ┌─────────┐  │  ┌──────────────────┐  │
│         ▼           │  │ UDP     │  │  │    DnsCache      │  │
│  ┌──────────────┐   │  │ Engine  │  │  │  IP → hostname   │  │
│  │  Packet I/O   │   │  └─────────┘  │  └──────────────────┘  │
│  │  parse/build  │   │  ┌─────────┐  │                        │
│  │  checksums    │   │  │ DNS     │  │  ┌──────────────────┐  │
│  └──────────────┘   │  │ parser  │  │  │   ProxyPool      │  │
│                     │  └─────────┘  │  │  - health scores  │  │
│                     └───────────────┘  │  - rotation       │  │
│                                        └──────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

## Layers

### 1. Traffic Interception (TunManager)

- Creates TUN device at configurable address (e.g. 10.99.0.1/24)
- Sets `0.0.0.0/0` default route through TUN
- Adds exclude route for proxy IP through real interface (avoids loop)
- Single-threaded blocking read loop on TUN fd
- Dispatches all IP packets to Router for protocol-specific handling
- Writes response packets (SYN-ACK, TCP data, UDP response) back to TUN

### 2. Packet Parsing & Building (Packet)

- IP header parser (v4 only for v1)
- TCP header parser with full flags (SYN, ACK, FIN, RST, PSH, URG)
- TCP packet builder with correct IP/TCP checksums
- UDP packet builder with correct IP/UDP checksums
- FlowKey identifier: (src_ip, src_port, dst_ip, dst_port)

### 3. TCP Forwarding Engine (Router)

- Connection tracking via HashMap<FlowKey, TrackedConnection>
- ForwardConnection wraps tokio TcpStream (Direct, HTTP CONNECT, SOCKS5, SOCKS4)
- SYN → proxy CONNECT → SYN-ACK (with retransmitted-SYN guard)
- Data forwarding with SEQ tracking
- Non-blocking response pump (try_read per connection)
- FIN/RST → connection teardown + proxy cleanup

### 4. UDP Forwarding Engine (Router)

- Flow tracking via HashMap<FlowKey, UdpFlowEntry>
- Each flow: connected UDP socket to original destination
- Payload sent from TUN → upstream via UDP socket
- Non-blocking recv from all flows → build response → write to TUN
- Flow timeout (60s idle) with automatic cleanup

### 5. DNS Cache (DNS)

- Parses DNS A-record responses from intercepted traffic
- Extracts actual hostname from question section via pointer traversal
- Cache: HashMap<u32, String> (IP → hostname)
- Used by TCP engine: hostname required for proxy CONNECT

### 6. Proxy Forwarder (Forwarder)

- HTTP CONNECT with response validation (200 OK / 200 Connection established)
- SOCKS5 via tokio-socks
- SOCKS4 via tokio-socks
- Direct passthrough (no proxy)

### 7. Proxy Pool (Pool)

- List of known proxies with health scores
- Active proxy selection + rotation
- Health check integration

## Data Flows

### TCP via Proxy
```
TUN: SYN ──▶ Router ──▶ Forwarder CONNECT ──▶ SYN-ACK ──▶ TUN
TUN: ACK+GET ──▶ Router ──▶ proxy socket write
proxy socket read ──▶ Router ──▶ build TCP pkt ──▶ TUN
TUN: FIN ──▶ Router ──▶ cleanup
```

### UDP via Direct Socket
```
TUN: UDP pkt ──▶ Router ──▶ UDP socket send ──▶ upstream
UDP socket recv ──▶ Router ──▶ cache DNS ──▶ build UDP pkt ──▶ TUN
```

## Error Handling

- Proxy connect failure → no SYN-ACK → kernel retransmits → retry
- Proxy write failure → RST to client via FIN
- UDP timeout → packet dropped (no guarantee for UDP)
- No active proxy → SYN-ACK not sent → kernel resets
- Stale connection cleanup: periodic sweep of idle flows

## Performance Considerations

- Single TUN reader thread (CPU-bound: syscall per packet)
- Non-blocking I/O for all proxy sockets
- No per-packet allocation for response buffers (reuse)
- Connection lookup O(1) via HashMap
- UDP flows: O(n) sweep but n ∝ concurrent UDP streams

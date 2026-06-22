---
reviewed: 2026-06-22T14:00:00Z
depth: deep
files_reviewed: 3
files_reviewed_list:
  - crates/proxy-core/src/router/connection.rs
  - crates/proxy-core/src/router/mod.rs
  - crates/proxy-core/src/lib.rs
findings:
  critical: 2
  warning: 5
  info: 3
  total: 10
status: issues_found
---

# Code Review: Task 2 — Connection Tracker and Router

**Reviewed:** 2026-06-22
**Depth:** deep (cross-file analysis including import graph)
**Files Reviewed:** 3
**Status:** `BLOCK` — critical architecture gaps prevent the forwarding loop from functioning

## Summary

The implementation closely follows the plan's code verbatim, with type-consistent imports and correct module wiring. However, two critical TCP state-management issues render the forwarding loop non-functional. The `ConnectionTracker` (connection.rs) is clean and well-factored. The `Router` (mod.rs) contains the structural issues. Overall: the components assemble correctly but the TCP flow logic is incomplete.

---

## Critical Issues

### CR-01: Missing SYN-ACK generation — TCP handshake never completes

**File:** `crates/proxy-core/src/router/mod.rs:22-53`
**Issue:** `handle_outgoing` intercepts TCP SYN packets and opens a proxy tunnel (`Forwarder::connect_to`), but never generates a SYN-ACK response to send back through the TUN device. Without a completed three-way handshake, the client's TCP stack will not transition to ESTABLISHED state, will not send data, and will eventually retransmit SYN and give up.

The TUN forwarding loop in the plan (Task 3) calls `handle_outgoing` then `handle_data` then `pump_responses` for each packet read. None of these generate a SYN-ACK. The client's kernel TCP stack is waiting for SYN-ACK from the remote server, but the SYN was intercepted and never forwarded. The proxy tunnel (HTTP CONNECT / SOCKS) is established between the proxy and the target server, but the client has no knowledge of this.

This is not a "will be fixed later" polish issue — without SYN-ACK generation, no TCP data will ever flow through the TUN, making the entire forwarding loop non-functional.

**Fix:** After `Forwarder::connect_to` succeeds in `handle_outgoing`, construct and return a SYN-ACK packet (with appropriate sequence/acknowledgment numbers matching the original SYN) through the TUN device. The response path — either via `pump_responses` returning a synthetic SYN-ACK, or via a dedicated method — must be wired into the forwarding loop. The generated SYN-ACK must carry:
- Sequence number = chosen ISN (e.g., a function of the client's ISN, or a random number tracked per-flow)
- Acknowledgment number = client's ISN + 1
- SYN + ACK flags

```rust
// In handle_outgoing, after successful proxy connect:
let syn_ack = build_syn_ack_packet(&key, &packet, client_isn);
// syn_ack must be returned from handle_outgoing or written to TUN
// The forwarding loop must handle this:
//   if let Some(response) = router.handle_outgoing(&packet).await {
//       dev.write_all(&response).await;
//   }
```

---

### CR-02: Hardcoded zero TCP sequence numbers in response packets

**File:** `crates/proxy-core/src/router/mod.rs:100-131`
**Issue:** `pump_responses` builds a `fake_packet` with `sequence_number: 0` and `acknowledgment_number: 0` for every proxy read. `build_response_packet` then uses these zeroed values to compute the response packet's TCP headers. As a result, every response sent to the client has:
- TCP sequence number = 0 (should be the proxy/server's actual sequence number)
- TCP acknowledgment number = 0 + n (should be client's sent seq + data length)

The client's TCP stack will reject these packets because:
1. The sequence number (0) is outside the expected receive window
2. The acknowledgment number doesn't match what the client expects

Even if CR-01 were fixed and data flowed from client to proxy, the response data would be sent with TCP headers that the client's kernel considers invalid and silently drops.

**Fix:** `pump_responses` must track per-connection TCP sequence/acknowledgment state. Either:
1. Store `(initial_client_seq, initial_server_seq)` in the tracker alongside each `ForwardConnection`, updated during SYN handshake, then use these when building response packets; or
2. Maintain a per-connection sequence number counter that increments with each read/write.

At minimum, synthetic response packets must carry:
- seq = the server's starting ISN (tracked during SYN-ACK generation)
- ack = the client's next expected byte (client_ISN + total_bytes_received)

```rust
// In FlowKey or associated state tracking:
pub struct FlowState {
    pub client_isn: u32,
    pub server_isn: u32,
    pub client_bytes_received: u32,
    pub server_bytes_received: u32,
}
```

---

## Warnings

### WR-01: `unwrap()` on `tcp.as_ref()` is brittle

**File:** `crates/proxy-core/src/router/mod.rs:27`
```rust
let tcp = packet.tcp.as_ref().unwrap();
```
**Issue:** While `is_tcp_syn()` (called on line 23) guarantees `tcp.is_some()` in the current implementation, this is an implicit coupling. If the implementation of `is_tcp_syn()` is ever refactored to not check `tcp.is_some()`, this line becomes a panic site. The function is in the same project, but the safety of `unwrap()` depends on the caller having already checked, which is a fragile pattern.

**Fix:** Use the same `if let` / `match` pattern used in `handle_data`:
```rust
let tcp = match &packet.tcp {
    Some(t) => t,
    None => return,
};
```
Or use `if let Some(tcp) = &packet.tcp` to guard the entire SYN handling block.

---

### WR-02: Unbounded ConnectionTracker growth (resource exhaustion risk)

**File:** `crates/proxy-core/src/router/connection.rs:16`
**Issue:** `ConnectionTracker` stores entries in a `HashMap` with no capacity limit, eviction policy, or TTL. Entries are only removed when a TCP FIN or RST packet arrives via `handle_data`. If FIN/RST packets are lost (common on lossy networks), or if an attacker sends many SYN packets through the TUN, both memory and file descriptors (each tracker entry holds an open `ForwardConnection` with a live proxy TCP stream) will grow without bound.

The proxy connections opened in `handle_outgoing` consume OS file descriptors and proxy server resources. A modest flood of 10,000 SYNs would open 10,000 concurrent proxy connections.

**Fix:** Implement at least one of:
1. Maximum capacity with LRU eviction (e.g., `lru::LruCache` instead of `HashMap`)
2. TTL timeout — remove stale entries after a configurable idle period
3. Per-source rate limiting to prevent SYN floods

```rust
use std::time::Instant;

pub struct TrackedConnection {
    pub conn: Arc<RwLock<ForwardConnection>>,
    pub created_at: Instant,
}
```

---

### WR-03: Head-of-line blocking in `pump_responses`

**File:** `crates/proxy-core/src/router/mod.rs:85-141`
**Issue:** `pump_responses` iterates all tracked connections sequentially, acquiring the write lock and performing an async `read` on each. If the *first* connection in iteration order has no data available, the entire task yields waiting for data from that connection. No other connection's data will be processed until that read completes. Since `HashMap` iteration order is non-deterministic, which connection blocks the others is unpredictable.

In practice, if connection A has data pending and connection B is idle, and B is iterated before A, B's idle read stalls the entire pump cycle. Responses from A are delayed until data arrives on B.

**Fix:** Use `tokio::select!` to read from all connections concurrently, or spawn individual read-per-connection tasks that push responses into a shared channel:
```rust
// Approach: spawn per-connection reader tasks
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
for key in tracker.keys().await {
    let tx = tx.clone();
    tokio::spawn(async move {
        // read from connection, send response through tx
    });
}
```

---

### WR-04: Inward-connection read mutates proxy while holding write lock on tracker

**File:** `crates/proxy-core/src/router/mod.rs:88-98`
**Issue:** In `pump_responses`, `self.tracker.get(&key).await` acquires a read lock on the tracker's inner `RwLock`, returns `Some(conn)`, then drops the read lock. Then `conn.write().await` acquires a write lock on the *connection*'s `RwLock`. While the connection is lock-protected, `handle_data` (which also acquires write lock via `conn.write().await`) will be blocked from writing data to that same connection. Since `pump_responses` reads from the proxy connection and `handle_data` writes to it, they contend on the same lock for every operation.

This is a correctness concern only under concurrent load — it's a contention/throughput issue. The lock granularity forces serialization of read and write on each connection.

**Fix:** For the read direction (`pump_responses`), consider using a read lock on the connection (`conn.read().await`) instead of a write lock for reading from the TCP stream. Only the write direction (`handle_data`) needs the write lock for actual writes.

---

### WR-05: Public `tracker` field breaks encapsulation

**File:** `crates/proxy-core/src/router/mod.rs:13`
```rust
pub struct Router {
    pub tracker: ConnectionTracker,  // <-- pub
    pool: Arc<ProxyPool>,
}
```
**Issue:** The `tracker` field is `pub`, allowing external code to bypass `Router`'s methods and manipulate connections directly (insert without opening a proxy tunnel, remove mid-flow, etc.). The `ConnectionTracker` itself also has all-public methods. This means external code can insert arbitrary entries into the connection map, bypassing the router's invariant that every tracked connection has a corresponding proxy tunnel.

**Fix:** Make `tracker` private and expose only the methods the router needs:
```rust
pub struct Router {
    tracker: ConnectionTracker,
    pool: Arc<ProxyPool>,
}
```

---

## Info

### IN-01: `is_empty()` added but unused in reviewed scope

**File:** `crates/proxy-core/src/router/connection.rs:24-26`
**Issue:** The `is_empty()` method was added to `ConnectionTracker` beyond what the plan specified. It's not called anywhere in the reviewed files. As a public API method it may be useful externally, but within this module it's dead code. If it's not needed externally, it should be removed.

---

### IN-02: Redundant double buffering in `pump_responses`

**File:** `crates/proxy-core/src/router/mod.rs:93,101,103`
**Issue:** In `pump_responses`:
1. `let mut buf = vec![0u8; 65536]` — allocates 64KB per connection per pump cycle
2. `buf.truncate(n)` — shrinks to actual data size
3. `payload: buf.clone()` — clones the (now truncated) buffer into the fake packet
4. `build_response_packet(&fake_packet, &buf)` — passes the same buffer as a parameter

`build_response_packet` uses `fake_packet.payload` only for computing `payload_len` (which is `buf.len()`, equal to `n`), then appends `payload` (=`&buf`) as the packet data. The cloned buffer in `fake_packet.payload` is only used for length calculation and could be avoided. A single `n` value would suffice.

**Fix:** Either:
- Store the length in the fake packet instead of cloning the full buffer
- Or build the response packet directly without the intermediate fake packet

---

### IN-03: Missing `#[cfg(test)]` modules

**File:** `crates/proxy-core/src/router/connection.rs`, `crates/proxy-core/src/router/mod.rs`
**Issue:** Neither file contains unit tests. While some integration-level testing exists in Task 4, the `ConnectionTracker` (HashMap concurrent operations, FlowKey hash/equality) and `Router` (FlowKey construction, FIN cleanup logic) would benefit from direct unit tests.

**Fix:** Add test modules:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_key_hash_eq() {
        let a = FlowKey { src_ip: Ipv4Addr::LOCALHOST, src_port: 1, dst_ip: Ipv4Addr::LOCALHOST, dst_port: 80 };
        let b = FlowKey { src_ip: Ipv4Addr::LOCALHOST, src_port: 1, dst_ip: Ipv4Addr::LOCALHOST, dst_port: 80 };
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_tracker_insert_get_remove() {
        let t = ConnectionTracker::new();
        // ... test lifecycle
    }
}
```

---

## Assessment

| Criteria | Verdict |
|---|---|
| Clear responsibility per file | **PASS** — connection.rs owns tracking, mod.rs owns routing, lib.rs owns wiring |
| Independently testable units | **FLAG** — ConnectionTracker yes, Router depends on concrete Forwarder and ProxyPool |
| Follows plan structure | **PASS** — matches plan code exactly (+ `is_empty` bonus method) |
| Bugs / security issues | **BLOCK** — CR-01 and CR-02 make the TCP forwarding non-functional |
| Code quality | **PASS** for connection.rs; **FLAG** for mod.rs (unwrap, public fields, head-of-line blocking) |

### `BLOCK`

The two critical issues (CR-01, CR-02) are fundamental TCP state management gaps. The router's forwarding loop as designed cannot complete the TCP three-way handshake, and even if it could, the response packets carry synthetic TCP headers that the client's kernel stack will reject. These are not edge cases — they are the core data path. The code compiles and reads well, but the TCP flow logic is incomplete.

**Recommendation:** Resolve CR-01 and CR-02 before integrating Task 3 (TUN forwarding loop). The current Router will accept and discard data silently.

---

_Reviewed: 2026-06-22T14:00:00Z_
_Reviewer: gsd-code-reviewer (deep mode)_
_Depth: deep (cross-file analysis)_

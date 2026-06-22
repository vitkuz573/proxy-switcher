# Code Review: Task 1 — IP/TCP Packet Parser

**Reviewed:** 2026-06-22
**Depth:** standard
**Files Reviewed:** 4
**Status:** issues_found

## Summary

Four files implementing an IP/TCP packet parser were reviewed. The core parsing logic in `ip.rs` and `tcp.rs` is clean, well-validated, and free of panics. The packet composition code in `mod.rs` (`build_response_packet`, `ip_checksum`, `tcp_checksum`) has a correctness bug in sequence number handling and several robustness gaps.

The tests cover the basic happy path (SYN packet parse + response build) but miss critical validations: checksum correctness, payload integrity, non-TCP packets, and edge cases.

**Assessment: FLAG** — Ship-blocking bug in `build_response_packet` sequence number logic; significant coverage gaps in tests.

---

## Strengths

1. **Clean separation of concerns.** `ip.rs` and `tcp.rs` each own one struct with one `parse` method and one `header_len` accessor. `mod.rs` composes them into `ParsedPacket` and provides the response builder.

2. **Thorough input validation in parsers.** Both `IpHeader::parse` and `TcpHeader::parse` check minimum length (20 bytes), validate computed header length against buffer bounds, and return `Err` on malformed input. No unwrap/expect in production code.

3. **Correct wire-format interpretation.** Flags, fragment offset, IHL, data offset, and endianness are all parsed correctly per RFC 791 and RFC 793.

4. **Correct IP checksum RFC 1071 implementation.** The `ip_checksum` helper handles odd-length data, uses wrapping add, and folds carries correctly.

5. **No unsafe code.** Entire implementation is safe Rust.

---

## Critical Issues

### CR-01: `build_response_packet` computes wrong acknowledgment number for data-carrying packets

**File:** `mod.rs:77`
**Issue:** The acknowledgment number is always set to `tcp.sequence_number.wrapping_add(1)`. This is only correct for SYN and FIN packets (which consume 1 byte of sequence space per RFC 793 §3.4). For data-carrying packets, the acknowledgment should advance by `payload.len()` bytes. The current code will produce a TCP segment with an incorrect acknowledgment number for any non-SYN/non-FIN packet that contains data, causing the receiver to reject it or keep retransmitting.

**Fix:**
Replace the hardcoded `wrapping_add(1)` with a computation that accounts for payload length:
```rust
// Calculate how many bytes this packet consumed from the sequence space
let consumed = if tcp.flags.syn || tcp.flags.fin {
    1u32  // SYN or FIN consumes 1 byte regardless of payload
} else {
    0u32
};
// Total bytes consumed = payload bytes + SYN/FIN byte
let ack_num = tcp.sequence_number
    .wrapping_add(original.payload.len() as u32)
    .wrapping_add(consumed);
```

Also update the code at line 77 to use `ack_num` instead of `tcp.sequence_number.wrapping_add(1)`.

---

## Warnings

### WR-01: TCP options silently stripped in `build_response_packet`

**File:** `mod.rs:55`
**Issue:** The function hardcodes `tcp_len = 20` and `data_offset = 0x50` (20 bytes). If the original TCP header has options (e.g., window scaling, SACK, timestamps), they are silently dropped in the response. This can degrade TCP performance or break connections that depend on specific options. While acceptable for MVP, it should be documented or flagged.

**Fix:** Either document this as a known limitation, or propagate at least the option bytes from the original header:
```rust
// Propagate options if the original header has any
let tcp_header_len = original.tcp.as_ref().map_or(20, |t| t.header_len());
let tcp_len = tcp_header_len;
let data_offset_byte = ((tcp_header_len / 4) as u8) << 4;
```
And then copy the original TCP option bytes from `original` data (not available directly — this requires access to the raw TCP header bytes, which is a bigger refactor). Minimally: **document the limitation** in the function doc comment.

### WR-02: No protection against IP total_length overflow

**File:** `mod.rs:56,62`
**Issue:** The computation `let ip_total = 20 + tcp_len + payload.len()` is done in `usize` (u64 on 64-bit), then truncated to u16 at line 62 via `as u16`. If `payload.len() > 65495`, the IP `total_length` field silently wraps, producing a malformed packet. An unusually large payload could also cause the TCP length field in the pseudo-header to wrap to 0 (since `segment.len()` would be > 65535).

**Fix:** Add a length check before building the packet:
```rust
let ip_total = 20 + tcp_len + payload.len();
if ip_total > u16::MAX as usize {
    return Vec::new();  // or handle error
}
```

### WR-03: Tests don't verify checksum correctness

**File:** `mod.rs:170-180`
**Issue:** The test `test_build_response` parses the response and checks IP addresses and ports, but never verifies:
1. The IP checksum is valid
2. The TCP checksum is valid
3. The payload data is preserved correctly
4. The acknowledgment number equals `original_seq + 1`

Without checksum verification, a regression in the checksum computation code would go undetected.

**Fix:** Add assertions verifying both checksums:
```rust
// Verify IP checksum
let ip_csum_expected = ip_checksum(&resp[..20]);
assert_eq!(u16::from_be_bytes([resp[10], resp[11]]), ip_csum_expected);

// Verify TCP checksum (requires external verification or recomputation)
// Verify payload
let parsed = ParsedPacket::parse(&resp).unwrap();
assert_eq!(parsed.payload, b"HTTP/1.1 200 OK\r\n");
```

### WR-04: No test for non-TCP packets

**File:** `mod.rs:137-180`
**Issue:** There are no tests for:
- `build_response_packet` returning empty Vec when `original.tcp` is None (non-TCP packet)
- `ParsedPacket::parse` with non-TCP protocol (e.g., ICMP, UDP)
- Edge cases: empty data, truncated headers, invalid checksums

**Fix:** Add tests covering these paths.

### WR-05: No test for `is_tcp_fin` or `is_tcp_syn` edge cases

**File:** `mod.rs:159-167`
**Issue:** `test_parse_syn` tests `is_tcp_syn()` returns true for a SYN packet, but there's no test for:
- `is_tcp_syn()` returns false for non-SYN packets
- `is_tcp_fin()` returns true for FIN/RST packets
- `is_tcp_syn()`/`is_tcp_fin()` handle `tcp == None`

**Fix:** Add tests for these cases:
```rust
#[test]
fn test_not_syn() {
    let mut pkt = make_syn_packet();
    pkt[33] = 0x10; // ACK only, no SYN
    let p = ParsedPacket::parse(&pkt).unwrap();
    assert!(!p.is_tcp_syn());
}

#[test]
fn test_no_tcp_syn() {
    let mut pkt = make_syn_packet();
    pkt[9] = 1; // ICMP protocol
    let p = ParsedPacket::parse(&pkt).unwrap();
    assert!(!p.is_tcp_syn());
}
```

---

## Info

### IN-01: `ihl` field stores byte count but field name suggests RFC word count

**File:** `ip.rs:7`
**RFC definition:** The Internet Header Length (IHL) field is the number of 32-bit words in the header. The `IpHeader.ihl` field stores the byte count (e.g., `20` for a standard header), which is the number of words × 4. The `header_len()` method returns `self.ihl as usize`, which works correctly, but serializing this value back to wire format would produce a malformed IHL field. Consider renaming to `header_len_bytes` for clarity, or store the word count and compute bytes in `header_len()`.

### IN-02: `data_offset` field stores byte count but RFC defines it as 32-bit words

**File:** `tcp.rs:8`
**Same pattern as IN-01:** The TCP data offset field in the RFC is the number of 32-bit words, but `TcpHeader.data_offset` stores the byte count. `header_len()` works correctly, but the naming is misleading for anyone familiar with the RFC. Consider renaming to `header_len_bytes`.

### IN-03: Redundant masking in fragment_offset parsing

**File:** `ip.rs:37`
**Issue:** The expression `u16::from_be_bytes([data[6] & 0x1F, data[7]]) & 0x1FFF` applies `0x1FFF` masking after already masking `data[6]` with `0x1F`. The second `& 0x1FFF` is redundant because the upper byte is already limited to 5 bits. Consider simplifying to just read the full word and mask once:
```rust
fragment_offset: u16::from_be_bytes([data[6], data[7]]) & 0x1FFF,
```

### IN-04: `tcp_checksum` allocates a Vec on every call

**File:** `mod.rs:118`
**Issue:** The function creates a new `Vec` every time it computes a checksum. This is not a correctness issue, but it's an unnecessary allocation. The pseudo-header could be computed without allocation by writing into a fixed-size array (max 12 + 65535 bytes) or using a streaming approach. Consider using a fixed-size buffer on the stack for small segments, or pre-allocating a reusable buffer.

### IN-05: `ip_checksum` would return `0xFFFF` for empty data

**File:** `mod.rs:104-114`
**Issue:** If `ip_checksum` were called with an empty slice, the loop doesn't execute, `sum` stays 0, and `!0` = `0xFFFF`. While neither the IP header (min 20 bytes) nor the TCP pseudo-header (min 12 + 20 bytes) would trigger this in practice, it's a latent footgun. Consider adding a `debug_assert!(!data.is_empty())` or documenting the behavior.

---

## Assessment: FLAG

| Category | Count |
|----------|-------|
| Critical | 1 |
| Warning  | 5 |
| Info     | 5 |
| **Total** | **11** |

**One ship-blocking bug** (CR-01): `build_response_packet` produces incorrect TCP acknowledgment numbers for data-carrying packets. This will cause receivers to reject or retransmit, breaking proxy functionality for anything beyond the initial handshake.

**Five warnings** that should be addressed before shipping: options stripping (WR-01), overflow protection (WR-02), and significant test coverage gaps (WR-03 through WR-05).

The core parsing logic in `ip.rs` and `tcp.rs` is solid and well-validated. The primary risks are in the packet composition code and test coverage.

---

_Reviewed: 2026-06-22_
_Reviewer: gsd-code-reviewer_
_Depth: standard_

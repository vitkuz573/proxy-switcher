# proxy-switcher — Summary

## Goal
Build a Rust-based system-wide IP switcher (TUN-based) that routes all Linux traffic through user-selected public proxies with a Web UI.

## Constraints & Preferences
- Language: Rust
- Interface: CLI + Web UI (vanilla JS SPA, no build step)
- Full enterprise structure (workspace layout, Dockerfile, systemd service, CI)
- TUN device for system-wide traffic interception
- Automatic proxy scraping from public sources with UI management
- Health checking through the proxy itself, not direct
- Linux only for v1
- Must handle thousands of proxies with health scoring
- No GitHub Actions (blocked in Russia)
- Zero warnings in build (enforced)

## Progress
### Done
- Enterprise architecture: default route `0.0.0.0/0` through TUN with proxy IP exclude route via real gateway
- Full TCP forwarding: SYN→proxy CONNECT→SYN-ACK with retransmitted-SYN guard (skips duplicate proxy connections)
- UDP forwarding engine (`router/udp.rs`): non-blocking socket pool per flow, flow tracking with 60s idle timeout, automatic cleanup
- DNS interception: `pump_udp` parses A-record responses, extracts actual hostname via DNS name pointer traversal, caches IP→hostname in `DnsCache`
- fwmark-based policy routing: `SO_MARK` on UDP sockets + `ip rule` + custom routing table prevents UDP socket→TUN loop
- `DnsCache::entries()` and `Router::dns_cache_entries()` added for API consumption
- Web UI SPA: dark-themed dashboard with 5 views (Dashboard, Proxies, Scraper, Connections, DNS Cache), auto-refresh every 3s, SPA fallback routing
- API: `GET /api/v1/stats`, `GET /api/v1/dns`, `GET/POST/DELETE /api/v1/sources` for live metrics and runtime source management
- Scraper UI: manual proxy add/delete, on-demand scrape trigger, scrape status polling, sources card with add/delete
- ProxyPool: `add()`, `remove()` methods with active-index adjustment
- `ScrapeState` shared between API and background auto-scrape cycle (running, last_run, proxies_found, healthy_count, errors)
- Add Proxy modal with host/port/protocol/country fields
- Dynamic source management: `Scraper` reads from `Arc<RwLock<Vec<String>>>` at scrape time, empty list falls back to built-in defaults, API sources CRUD, UI sources section in Scraper tab
- All unit/integration tests pass (5/5), clippy clean (0 warnings)
- Git repo pushed to GitHub (commit `d3c34e7`)
- Docker setup: optimized multi-stage `Dockerfile` with layer caching, `.dockerignore`, `docker-compose.yml` with host networking + NET_ADMIN + SYS_MODULE + `/dev/net/tun`, port 9090 config (avoids 8080 conflict with keycloak)
- Docker container verified: TUN device `ps-tun0` created, API healthy on `:9090`, 4249 proxies scraped, health check running

### In Progress
- (none)

### Blocked
- (none)

## Key Decisions
- TUN routing: full `0.0.0.0/0` via TUN + proxy IP exclude route chosen over selective routing or iptables TPROXY
- UDP forwarded directly through real interface, not through proxy — proxy pool handles TCP only
- DNS interceptor built into `pump_udp` rather than reverse DNS or manual injection
- fwmark (`SO_MARK`) + policy routing used to prevent UDP socket→TUN loop instead of `SO_BINDTODEVICE` or iptables
- `handle_outgoing` short-circuits retransmitted SYNs: resends SYN-ACK with existing `server_isn` instead of creating new proxy connection
- Vanilla JS SPA with no build step — `tower-http` serves static `ui/` directory, custom handler for SPA fallback
- Scrape sources stored as `Arc<RwLock<Vec<String>>>` shared between Scraper and API — runtime modifiable, persists only in-memory
- Docker container uses `network_mode: host` + `cap_add NET_ADMIN SYS_MODULE` + `/dev/net/tun` device because daemon needs to create TUN device and modify host routing tables
- Docker API port changed to 9090 (host port 8080 occupied by keycloak container)
- Dockerfile uses multi-stage build with layer caching (Cargo.toml→deps→source) for faster rebuilds
- `libssl-dev` added to builder image for openssl-sys (reqwest native-tls)

## Next Steps
1. Integrate routing setup into daemon `main.rs` (call `set_default_route()` and `setup_fwmark_routing()` from TunManager)
2. Start container with `docker compose up -d proxy-switcher`, verify E2E traffic routing
3. Improve performance: connection reuse, buffer pooling, reduced allocations
4. Add IPv6 support
5. Production hardening: SIGHUP reload, metrics, error rate tracking, rate limiting

## Critical Context
- Host: Debian 13 (trixie), kernel 6.12.63, Docker 26.1.5, Rust 1.96.0
- Proxy `116.101.75.173:2079` accepts `CONNECT httpbin.org:80` but rejects `CONNECT 52.5.245.178:80` (502) — hostname must be resolved before connecting
- Full E2E flow verified: curl → TUN → DNS forwarded via UDP → DNS parsed + cached → TCP SYN → proxy CONNECT with cached hostname → HTTP response → curl displays `{"origin": "<proxy_ip>"}`
- Kernel selects `src=10.99.0.1` as source IP for packets routed through TUN (`ip route get` confirms)
- fwmark=1, routing table=100: `ip rule add fwmark 1 table 100`, `ip route add default via <gateway> dev <iface> table 100`
- Host port 8080 occupied by keycloak docker-proxy — daemon API uses 9090
- `handle_outgoing` takes ~1.1s for first SYN (proxy connect + CONNECT round trip), curl retransmits SYN — retransmitted-SYN guard handles this
- Single-threaded TUN loop in `spawn_blocking`; all async router calls via `handle.block_on`
- Health checker config: concurrency=20, timeout=10s, check_interval=60s, target=http://httpbin.org/ip
- 4249 proxies scraped from 11 sources; initial health check takes ~minutes with 20 concurrent workers
- Daemon `main.rs` does NOT currently call `set_default_route()`/`setup_fwmark_routing()` — TUN device is created and loop runs, but no traffic routes through it until routing is added
- Leftover routing state on host: `ip rule` shows entry for `172.20.20.33 table 100` (from earlier tests)

## Relevant Files
- `Dockerfile`: Optimized multi-stage build (Rust bookworm slim builder → Debian bookworm slim runtime)
- `.dockerignore`: Excludes target/, .git/, docs/, scripts/, etc. from Docker context
- `docker-compose.yml`: Host networking, NET_ADMIN + SYS_MODULE caps, `/dev/net/tun` device, journald logging
- `config/proxy-switcher.docker.toml`: Docker-specific config with `api_port = 9090`, empty scrape sources (uses built-in defaults)
- `crates/proxy-core/src/scraper/mod.rs`: `Scraper` with built-in sources and dynamic source support via `Arc<RwLock<Vec<String>>>`
- `crates/proxy-core/src/tun_manager/mod.rs`: TUN loop, default route, exclude route, fwmark setup/cleanup, UDP dispatch
- `crates/proxy-core/src/router/mod.rs`: TCP `handle_outgoing`/`handle_data`/`pump_responses` + UDP `handle_udp`/`pump_udp`/`cleanup_udp`
- `crates/proxy-core/src/router/udp.rs`: `UdpTracker` — flow key, socket pool, `send_or_create`, `recv_all`, `cleanup_stale`
- `crates/proxy-core/src/router/connection.rs`: `ConnectionTracker`, `TrackedConnection`, `TcpState`, `FlowKey`
- `crates/proxy-core/src/forwarder/mod.rs`: `ForwardConnection` with `try_read` (non-blocking), HTTP CONNECT, SOCKS5/4
- `crates/proxy-core/src/dns/mod.rs`: `DnsCache`, `DnsHandler`, `parse_dns_response`, `entries()`
- `crates/proxy-core/src/packet/mod.rs`: `build_tcp_packet`, `build_udp_packet`, IP/UDP checksums
- `crates/proxy-core/src/config/mod.rs`: `TunConfig`, `Config`, `DaemonConfig`, `ScraperConfig`, etc.
- `crates/proxy-core/src/pool/mod.rs`: `ProxyPool` with `add()`, `remove()`, `apply_health_results()`, `set_active()`
- `crates/proxy-daemon/src/api.rs`: All REST API handlers + UI static file serving + `ScrapeState` + sources CRUD
- `crates/proxy-daemon/src/main.rs`: Daemon entrypoint — TUN init, scrape cycle, health check cycle, API server
- `crates/proxy-daemon/ui/index.html`: SPA with 5 views (Dashboard, Proxies, Scraper, Connections, DNS Cache)
- `crates/proxy-daemon/ui/css/app.css`: Dark theme, modal, spinner, responsive layout
- `crates/proxy-daemon/ui/js/app.js`: Client logic — polling, proxy CRUD, scrape trigger, sources management, modal, navigation
- `crates/proxy-cli/src/main.rs`: CLI tool for daemon API
- `docs/superpowers/specs/2026-06-22-proxy-core-enterprise-architecture.md`: Architecture design document

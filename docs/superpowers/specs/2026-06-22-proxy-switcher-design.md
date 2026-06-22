# Proxy Switcher — Design Document

## Overview

A system-wide IP switcher for Linux. Acts as a VPN-like TUN gateway that routes all system traffic through user-selected public proxies. Supports automatic proxy discovery via scraping, health checking, manual switching, and auto-rotation.

## Architecture

```
proxy-daemon (background daemon, runs as root for TUN)
├── TUN Manager          — creates/manages tun0, routing
├── Router / Core        — packet dispatch logic
├── Forwarder            — HTTP CONNECT, SOCKS4/5 forwarding
├── Scraper              — pulls proxies from public sources
├── Health Checker       — tests latency, anonymity, protocols
├── Pool Manager         — ranked pool + switch logic
├── HTTP REST API (axum) — control plane
└── Web UI (SPA)         — embedded, served on :8080

proxy-cli (CLI client, unprivileged)
└── Communicates with daemon REST API
```

## Data Flow

1. Daemon creates `tun0`, moves default route to it
2. All IP packets captured from tun0
3. Router parses destination, DNS, protocol
4. Forwarder opens connection via selected proxy (HTTP CONNECT/SOCKS)
5. Response packets written back to tun0
6. User switches proxy via CLI/UI — instant effect on next packet

## Components

### proxy-core (library crate)

| Module | Responsibility |
|--------|---------------|
| `tun_manager/` | TUN device lifecycle, routing setup, packet I/O |
| `proxy/` | HTTP CONNECT, SOCKS4, SOCKS5 protocol handlers |
| `forwarder/` | Stream proxy: read from TUN → proxy → write to TUN |
| `scraper/` | Public proxy list scraping + parsing |
| `health/` | Latency, anonymity, protocol support checks |
| `pool/` | Proxy pool, ranking, auto/manual switch |
| `dns/` | DNS resolution strategy (prevent leaks) |

### proxy-daemon (binary crate)

- Background process (root)
- REST API on `127.0.0.1:8080`
- Serves embedded Web UI
- Manages TUN device and routing
- Runs scrape → health → serve loop

### proxy-cli (binary crate)

- Unprivileged CLI tool
- Commands: `switch`, `list`, `status`, `rotate`, `sources`, `add`
- Connects to daemon API

### Web UI

- React SPA (or Leptos for Rust-native WASM)
- Dashboard: active proxy, latency, traffic graph
- List/switch proxies
- Source management
- Settings: auto-rotate interval, health check params

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | /api/v1/status | Daemon status, active proxy |
| GET | /api/v1/proxies | List all proxies w/ metrics |
| POST | /api/v1/proxies/:id/switch | Switch active proxy |
| POST | /api/v1/rotate | Toggle/trigger auto-rotation |
| GET | /api/v1/sources | List proxy sources |
| POST | /api/v1/sources | Add custom source |
| GET | /api/v1/stats | Traffic stats, uptime |

## Tech Stack

| Component | Choice |
|-----------|--------|
| Runtime | tokio |
| HTTP | axum |
| TUN | tun crate + libc bindings |
| SOCKS | tokio-socks |
| Scraping | reqwest + scraper / select.rs |
| Config | serde + toml |
| Logging | tracing (structured) |
| CLI | clap |
| DNS | hickory-resolver (trust-dns) |
| Web UI | React (JSX/TSX) or Leptos (Rust WASM) |

## Security

- Daemon runs as root (CAP_NET_ADMIN for TUN), drops privileges after setup
- API bound to localhost only
- DNS leak prevention via hickory-resolver forced through selected proxy
- IPv6 toggle (disable to prevent leaks)

## Non-Goals (YAGNI)

- No GUI desktop app (CLI + Web UI sufficient)
- No pluggable auth backend (single-user local)
- No cloud sync
- No Windows/macOS support in v1

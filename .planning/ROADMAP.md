# Roadmap

## Phase 1: Core Foundation (proxy-core)
- Workspace setup (Cargo.toml, lint, CI)
- TUN device manager
- Packet read/write loop
- Forwarder trait + HTTP CONNECT handler
- SOCKS4 handler
- SOCKS5 handler
- Integration test: TUN → forward → TUN

## Phase 2: Proxy Discovery & Health
- Proxy types/enums
- Scraper framework
- Built-in sources (3-5 scrapers)
- Health checker (latency, protocol, anonymity)
- Pool manager with ranking
- Persistent storage (SQLite via rusqlite)

## Phase 3: Daemon + API
- proxy-daemon binary
- Config file (TOML)
- REST API (axum)
- Scrape → health → pool loop
- Routing logic: TUN → pool → forwarder
- Graceful shutdown
- systemd service file

## Phase 4: CLI
- proxy-cli binary
- Commands: status, list, switch, rotate, sources, add
- API client library
- Human-friendly output

## Phase 5: Web UI
- React/Leptos SPA
- Dashboard (active proxy, latency, traffic)
- Proxy list with search/filter
- Live switch
- Stats page
- Embedded into daemon binary

## Phase 6: Polish & Enterprise
- Dockerfile
- GitHub Actions (CI)
- Benchmarking
- Stress testing
- Documentation
- Release scripts

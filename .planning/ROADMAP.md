# Roadmap

Phases 1–4 are delivered. Phases 5 and 6 were rewritten after the Rust daemon
stopped serving a UI of its own; the shipped design is in
[`docs/architecture-v2.md`](../docs/architecture-v2.md) and the wire contract
is in [`docs/rest-contract.md`](../docs/rest-contract.md).

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

## Phase 5: Web Console — superseded

The original plan was a React/Leptos SPA embedded in the daemon binary. That was
dropped: a UI coupled to the engine cannot evolve independently of it, and it
forced the daemon to carry a web server. The console is now a separate Next.js
application in `web/`, and the Rust daemon serves an internal API only.

Delivered instead:
- Next.js 16 console: dashboard, proxies, scraper, audit, login
- Same-origin proxy at `/api/[...path]`, so the browser never holds a token
  and the API needs no CORS surface
- shadcn/ui components, installed by CLI and never hand-edited
- Pages stream inside the static shell via Suspense

## Phase 6: Control plane — delivered

- `api/` — Fastify service, the only public surface, on `127.0.0.1:8088`
- `sdk/` — owns the wire contract; the API and both clients compile against it
- `cli/` — rewritten against the typed SDK client
- Atomic release deploy with rollback (`scripts/deploy-api.sh`)

## Phase 7: Polish & Enterprise — remaining

- [x] Dockerfile
- [x] Local CI (`scripts/ci.sh`) — GitHub Actions are blocked in Russia
- [x] Release deploy with rollback
- [x] Documentation
- [ ] Web console as a systemd service
- [ ] TLS in front of the console. It currently listens on all interfaces with
      no HTTPS, so the login form submits a password in cleartext.
- [ ] Benchmarks and stress tests
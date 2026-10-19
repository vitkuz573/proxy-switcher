# Target Architecture — v2 Enterprise Platform

Status: accepted 2026-10-19. Supersedes ROADMAP Phase 5 (Web UI) and Phase 6 (Polish).

## Decision

The Rust daemon keeps the network engine. Everything around it — public API,
Web UI, CLI — is a TypeScript monorepo. The public API is a **separate service**,
not the daemon itself, because two different clients need it and they need
different trust models:

- **Web** is a browser session client (cookie, CSRF-protected).
- **CLI** is a token client (bearer token, no browser, no cookie jar).

One public contract serves both. Generated clients keep them in sync.

## Topology

```
┌────────────┐   session cookie        ┌─────────────────┐   internal HTTP   ┌──────────────────┐
│  Browser   │ ──────────────────────► │  api/ (Fastify) │ ────────────────► │ proxy-daemon    │
│  web/ SSR  │                         │  :8080          │   loopback only   │ (Rust engine)   │
└────────────┘                         │  public, authed │                   │ :9099           │
                                       └─────────────────┘                   └──────────────────┘
┌────────────┐   Bearer token
│  Terminal  │ ──────────────────────► │
│  cli/      │                         │
└────────────┘                         └─────────────────┘
                                                        ▲
                                                        │ in-process
                                                 ┌──────────────────┐
                                                 │ sdk/ (generated) │
                                                 └──────────────────┘
```

The daemon binds `127.0.0.1` only and is treated as an **internal contract** —
trusted because it is not reachable from outside the host. The API service is the
only public surface, and the only place auth, RBAC, audit logging, rate limiting
and validation live.

## Repository layout

pnpm workspace, Rust workspace unchanged alongside it.

```
proxy-switcher/
├── crates/                Rust engine (workspace)
│   ├── proxy-core/        TUN, scraper, health, pool, storage
│   ├── proxy-daemon/      engine host + internal API (UI serving removed)
│   ├── proxy-cli/         engine-side admin tool (kept)
│   └── proxy-test/, proxy-tun-test/
├── api/                   TypeScript API service — public, authenticated
├── sdk/                   owns the wire contract; API and clients compile against it
├── web/                   Next.js App Router, SSR, Tailwind, shadcn/ui
├── cli/                   TypeScript CLI client
├── deploy/                systemd units
├── scripts/               ci.sh, deploy-api.sh, deploy-web.sh
├── pnpm-workspace.yaml
└── package.json
```

## Stack

| Layer     | Choice                                                      |
|-----------|-------------------------------------------------------------|
| API       | Fastify 5, TypeBox schemas, `@fastify/swagger`               |
| SDK       | TypeBox schemas + a hand-written typed `fetch` client       |
| Web       | Next.js (App Router), React Server Components for SSR        |
| Styling   | Tailwind CSS v4                                              |
| Components| shadcn/ui — **downloaded via MCP, never hand-edited**        |
| CLI       | Commander.js + the SDK client                               |
| Lint      | Biome 2 (lint + format)                                     | 
| Types     | TypeScript 7 — the native compiler, invoked as a CLI         |
| Runtime   | Node 26, pinned by `.nvmrc` and `engines`                    |
| Auth      | Argon2id passwords, server-side sessions (Web), bearer tokens (CLI) |
| Storage   | SQLite for auth data; engine keeps its own SQLite for proxies |
| Packaging | pnpm workspaces                                             |

CI stays off GitHub Actions — it is blocked in Russia (existing project
constraint). Local `scripts/ci.sh` runs the same gates instead.

### Why the SDK is not generated

`sdk/src/schemas.ts` defines the wire types as TypeBox schemas. The API
validates against them and both clients derive their types from them, so the
server and its clients cannot drift apart.

Generating the SDK from the published OpenAPI document was tried and abandoned.
The generators consume TypeScript as a *library*, and TypeScript 7 — the native
port — does not expose the JavaScript compiler API: its `main` is a version
stub and only `unstable/*` entry points exist. Pinning an older TypeScript to
satisfy a code generator would have held the whole workspace back, so the
dependency was removed instead. The OpenAPI document is still published and
still verified, by `api/test/contract.test.ts`.

For the same reason the lint stack is Biome rather than ESLint:
`typescript-eslint` parses with the same JavaScript API.

## Deployment

Each service is a systemd unit with an atomic release swap and instant rollback.

| Service                | Unit                        | Binds              |
|------------------------|-----------------------------|--------------------|
| `proxy-switcher`       | engine, loopback only       | `127.0.0.1:9099`   |
| `proxy-switcher-api`   | public control plane        | `127.0.0.1:8088`   |
| `proxy-switcher-web`   | web console                 | `127.0.0.1:3100`   |

`scripts/deploy-api.sh` and `scripts/deploy-web.sh` build, stage a
self-contained release from the lockfile with `pnpm deploy --prod`, swap a
symlink and verify the service came up. `--rollback <release-id>` returns to
the previous release.

**TLS.** Nothing terminates TLS. The API and the engine are loopback-only. The
console is loopback-only too, because exposing it on a public interface would
put the login password on the wire in cleartext. Remote access requires a
TLS-terminating reverse proxy in front; the session cookie picks up `Secure`
automatically when `X-Forwarded-Proto: https` arrives.

## API service responsibilities

1. Terminate auth for both client models (session cookie, bearer token).
2. Enforce RBAC (`admin`, `operator`, `viewer`).
3. Validate every request against the contract schemas.
4. Proxy to the engine, translate errors, never leak internal detail.
5. Emit an audit log of mutating actions.
6. Rate-limit per principal.
7. Serve the OpenAPI document.

The engine's own `/api/v1/*` routes mirror the public shape, so the API service
is a thin authenticated layer rather than a translator between two dialects.
They are never exposed directly.

## How the pool is scheduled

The engine keeps a pool of proxies and probes them to find out which actually
work. The naive design — re-probe every proxy that has never worked, on every
cycle — has a cost that grows with the pool, and the pool grows with every
scrape. A cycle over fifteen thousand proxies took minutes, and every restart
started from an empty pool and paid the whole cost again.

Three things fix it.

**The pool is persisted.** The pool, its measurements and its schedule are
written to SQLite and restored at boot, so a restart has a usable pool in
seconds instead of minutes. `last_success` is what makes this work: without it
every restored proxy reads as unverified and is re-probed from scratch.

**Probes are scheduled per proxy.** Each proxy carries `next_probe_at`. A
working proxy is re-checked on a fixed interval; a proxy that failed backs off
through a growing schedule, so one that has been dead for a week is looked at a
few times a day instead of on every cycle. The engine sleeps until the next
proxy comes due, so a quiet pool stays quiet. The batch is bounded by what is
due, not by the size of the pool.

**Results are applied as they arrive.** `check_batch` streams each verdict into
the pool the moment it is known, so a proxy becomes routable when it is
confirmed rather than when the last probe of the batch finishes. Eviction is
deferred to a single sweep at the end of a batch, because removing a proxy
rebuilds the id index.

Measured on a pool of fifteen thousand proxies:

|                            | before    | after            |
|----------------------------|-----------|------------------|
| Full sweep                 | 7–20 min  | 85 s             |
| Throughput                 | 24 /s     | 178 /s           |
| Due for a probe afterwards | 15 311    | 0                |
| Working proxies after boot | 0         | 2 250, in 3 s    |

The remaining levers are ordinary: probes run concurrently (500 by default),
the handshake gets a short budget of its own so dead proxies are not waited out
at full length, and the probe target is a fast, highly available endpoint.

## What counts as a working proxy

The pool was full of entries that answered HTTP and were therefore called
working, while being useless for the one thing the engine does: carrying a
TCP stream through a tunnel.

A proxy is now only working if all of this holds:

- the tunnel to the target was established — a CONNECT for HTTP proxies, the
  SOCKS handshake for the others;
- the target answered with a success status;
- the target reported a source address, and that address is not the engine's
  own.

That last point is the one that matters. The address the target sees is the
address the traffic leaves from, so it is the proof that the request really
travelled through the proxy. An endpoint that merely answers HTTP, a captive
portal, or a proxy that serves its own error page instead of tunnelling all
fail it. The address is recorded per proxy and shown in the console, which
also makes it visible when many entries share one exit.

**Failures are separated by whose fault they are.** A refused connection, a
rejected CONNECT or a non-HTTP answer is the proxy's fault, and the proxy is
condemned. A timeout, or a target that answered with an error, is not
evidence against the proxy: the tunnel was established, so the proxy did its
job, and the target may be down, overloaded or blocking this network. Counting
those as failures condemns the entire pool whenever a third-party service has
a bad day.

For the same reason the probe target is chosen carefully. It is a third
party, and in some networks most of them are unreachable, so there are
several, and the engine verifies one directly before using it. The batch
still runs when none of them answer — the proxies are on other networks, and a
target that is blocked here may be perfectly reachable through one. A target
that is genuinely down produces inconclusive verdicts, which change nothing in
the pool. The failure mode this avoids is the worst one: an unreachable probe
target emptying a pool of fifteen thousand good proxies.

## Which traffic a proxy applies to

A proxy is a tool for specific destinations, not a default route for the whole
host. The `[routing]` section says which:

```toml
[routing]
mode = "none"        # none | allowlist | denylist
cidrs = ["1.2.3.0/24"]
domains = ["example.com"]
ports = [443]
always_direct = ["8.8.8.8"]
```

`none` is the default and routes nothing. A proxy that is selected for every
connection is a single point of failure: when it dies, everything routed
through it dies with it, including the daemon's own control traffic.

Local addresses never go through a proxy whatever the mode says, and neither
does the proxy itself — a connection to the proxy that is routed through a
proxy is a loop.

### Keeping the daemon out of its own device

The daemon creates a TUN device and can point the default route at it. Its own
connections — to the proxies it tests, to the sources it scrapes — would then
be captured by that device and routed through the active proxy, which is a
loop, and which takes the daemon offline the moment the active proxy dies.

Its sockets therefore carry a firewall mark, and the marked traffic follows a
copy of the main routing table rather than the device. The copy matters: the
main table is where an installed VPN keeps its routes, and a bypass table
with a single route through the physical gateway would send the daemon's
traffic around the VPN as well, losing the very endpoints it has to reach.

Moving the default route at the device is a separate, explicit decision
(`tun.default_route`), because it changes how the whole host reaches the
internet.

## Migration order

Each step is independently verifiable and leaves the system running.

1. Monorepo skeleton, tooling, CI script.
2. `api/` service: engine client, OpenAPI, auth, audit, rate limit.
3. `sdk/` generation wired into the build.
4. `web/`: shell, layout, design tokens, then views one by one.
5. `cli/`: status, list, switch, rotate, sources.
6. Remove UI serving from the Rust daemon; point `web/` at `api/`.
7. Retire the vanilla-JS UI and its Dockerfile copy.

## Non-goals

- Replacing the Rust engine. TUN forwarding, packet parsing and 500-way
  concurrent health probing stay in Rust for good reasons.
- Public internet exposure by default. Both services bind loopback until the
  operator deliberately changes it.
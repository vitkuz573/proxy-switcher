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
# REST Contract

Status: accepted 2026-10-19. Supersedes the earlier ad-hoc route set.

## Principles

- **Resources, not verbs.** A URL names a thing. `POST /proxies/switch` is an
  action hidden in a path; the thing being acted on is *which proxy is active*,
  so that is the resource.
- **Stable identifiers.** Sources are identified by `src_…`, not by their URL.
  A URL is data, contains slashes, and can change; a path parameter that is
  data breaks routing.
- **Operations are nested under their resource.** `POST /active-proxy/rotate`.
  The alternative — `/proxies/rotate` — collides with `/proxies/{id}` and
  silently shadows it. That bug shipped once already.
- **Never a colon in a route.** `/active-proxy:rotate` is worse than a
  collision: in both axum and Fastify a colon *starts* a path parameter, so the
  route matches any suffix. `…/tokens/exchange` is safe instead, because the
  router prefers a static segment and token ids are UUIDs.
- **Proper status codes.** `201` carries the created resource and a `Location`
  header. `204` for anything that returns nothing. `404` when absent.
- **Errors are RFC 9457 problem documents** (`application/problem+json`).
- **Collections are paginated.** The pool reaches tens of thousands of rows.
- **Everything is behind the gate.** Only `/healthz`, `/readyz` and the token
  exchange are public.

## The contract lives in code

The wire types are TypeBox schemas in `sdk/src/schemas.ts`. The API validates
against them and both clients derive their types from them, so server and client
cannot drift.

They were previously generated from the OpenAPI document by
`openapi-typescript`. That was abandoned deliberately: the generator needs
TypeScript's JavaScript compiler API, and TypeScript 7 — the native port —
does not expose it (`main` is a version stub; only `unstable/*` entry points
exist). `sdk/src/contract.test.ts` exercises the published document, so the
OpenAPI output is still verified, it is just no longer the source of truth.

## Router syntax differs per framework

Both frameworks use a different placeholder for a path parameter, and getting it
wrong registers a route that matches nothing while still appearing correct in
the OpenAPI document:

| Framework | Syntax     | Wrong                     |
|-----------|------------|---------------------------|
| axum 0.8  | `{id}`     | `:id` (treated as literal) |
| Fastify 5  | `:id`      | `{id}` (treated as literal)|

An OpenAPI check cannot catch this, because Fastify rewrites `:id` to `{id}` in
the document either way. `api/test/contract.test.ts` therefore sends real
requests through `app.inject`.

## Public API — `api/`

### Sessions (browser)

| Method | Path                   | Success | Notes                                  |
|--------|------------------------|---------|----------------------------------------|
| POST   | `/api/v1/sessions`     | 201     | `{email,password}` → `{user}` + cookie |
| GET    | `/api/v1/sessions/current` | 200 | `{user}`                               |
| DELETE | `/api/v1/sessions/current` | 204 |                                        |

### Tokens (CLI, automation)

| Method | Path                        | Success | Auth  | Notes                                    |
|--------|-----------------------------|---------|-------|------------------------------------------|
| POST   | `/api/v1/tokens`            | 201     | yes   | Issues a token for the caller            |
| GET    | `/api/v1/tokens`            | 200     | yes   | Caller's tokens                          |
| GET    | `/api/v1/tokens/{id}`       | 200     | yes   |                                         |
| DELETE | `/api/v1/tokens/{id}`       | 204     | yes   |                                         |
| POST   | `/api/v1/tokens/exchange`    | 201     | **no**| Credential grant: `{email,password}`     |

`tokens/exchange` is public because it *is* the authentication step for a CLI.
It is rate-limited per IP and audited like any other credential attempt.

### Proxies

| Method | Path                    | Success | Role      | Notes                    |
|--------|-------------------------|---------|-----------|--------------------------|
| GET    | `/api/v1/proxies`       | 200     | viewer    | `?limit=&offset=&sort=&search=` |
| POST   | `/api/v1/proxies`       | 201     | operator  | `Location` header        |
| GET    | `/api/v1/proxies/{id}`  | 200     | viewer    |                          |
| DELETE | `/api/v1/proxies/{id}`  | 204     | operator  |                          |

### Active proxy (singleton)

| Method | Path                          | Success | Role     | Notes                       |
|--------|-------------------------------|---------|----------|-----------------------------|
| GET    | `/api/v1/active-proxy`        | 200     | viewer   | `404` when nothing selected |
| PUT    | `/api/v1/active-proxy`        | 200     | operator | `{proxy_id}`                |
| DELETE | `/api/v1/active-proxy`        | 204     | operator | Clear the selection         |
| POST   | `/api/v1/active-proxy/rotate` | 200     | operator | Advance to the next proxy   |

### Scrape sources

| Method | Path                     | Success | Role     | Notes             |
|--------|--------------------------|---------|----------|-------------------|
| GET    | `/api/v1/sources`        | 200     | viewer   |                   |
| POST   | `/api/v1/sources`        | 201     | admin    | `Location` header |
| GET    | `/api/v1/sources/{id}`   | 200     | viewer   |                   |
| DELETE | `/api/v1/sources/{id}`   | 204     | admin    |                   |

### Scrape runs (job resource)

| Method | Path                              | Success | Role     | Notes |
|--------|-----------------------------------|---------|----------|-------|
| POST   | `/api/v1/scrape-runs`             | 201     | operator | Trigger a cycle |
| GET    | `/api/v1/scrape-runs`             | 200     | viewer   | Run history     |
| GET    | `/api/v1/scrape-runs/current`     | 200     | viewer   | Latest run      |

### Audit

| Method | Path                    | Success | Role  | Notes         |
|--------|-------------------------|---------|-------|---------------|
| GET    | `/api/v1/audit-events`  | 200     | admin | `?limit=&offset=` |

### Operations (public)

| Method | Path        | Success | Notes                        |
|--------|-------------|---------|------------------------------|
| GET    | `/healthz`  | 200     | Liveness                     |
| GET    | `/readyz`   | 200/503 | Readiness incl. engine       |
| GET    | `/openapi.json` | 200 | This document             |

## Engine — `crates/proxy-daemon`

Internal, loopback-only, same shape so the API service is a thin authenticated
layer rather than a translator between two different dialects.

| Method | Path                          | Notes                        |
|--------|-------------------------------|------------------------------|
| GET    | `/api/v1/proxies`             |                              |
| POST   | `/api/v1/proxies`             | `201` + body                 |
| GET    | `/api/v1/proxies/{id}`        |                              |
| DELETE | `/api/v1/proxies/{id}`        | `204`                        |
| GET    | `/api/v1/active-proxy`        | `404` when none              |
| PUT    | `/api/v1/active-proxy`        | Query `?proxy_id=`           |
| DELETE | `/api/v1/active-proxy`        | `204`                        |
| POST   | `/api/v1/active-proxy/rotate` |                              |
| GET    | `/api/v1/sources`             |                              |
| POST   | `/api/v1/sources`             | `201` + `{id,url}`           |
| DELETE | `/api/v1/sources/{id}`        | `204`                        |
| POST   | `/api/v1/scrape-runs`         | `202`                        |
| GET    | `/api/v1/scrape-runs/current` |                              |
| GET    | `/api/v1/summary`             | `{pool_size, active_proxy}`  |
| GET    | `/api/v1/stats`               |                              |
| GET    | `/api/v1/dns`                 |                              |

No catch-all fallback: the engine is not a web server.

## Pagination

```
GET /api/v1/proxies?limit=100&offset=0&sort=score&search=1.2.3.4
```

`limit` defaults to 50 and is capped at 500. `offset` defaults to 0.

## Errors

```json
{
  "type": "https://proxy-switcher.dev/problems/not-found",
  "title": "Proxy not found",
  "status": 404,
  "detail": "No proxy with id 198.51.100.1:8080",
  "instance": "/api/v1/proxies/198.51.100.1:8080"
}
```

`application/problem+json`, per RFC 9457.
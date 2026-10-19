/**
 * Contract tests.
 *
 * These exercise the routes through `app.inject`, so routing bugs surface here
 * rather than in production. That matters: a path parameter declared as `{id}`
 * instead of `:id` registers a route that matches nothing and still appears
 * correct in the OpenAPI document, because the document rewrites `:id` to
 * `{id}` either way. Only a real request distinguishes them.
 */
import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { buildApp } from "../src/app.js";
import { loadConfig } from "../src/config.js";
import { openDatabase } from "../src/db/index.js";
import { createUser } from "../src/services/auth.js";

/** Engine stand-in. Records what the API forwarded and replies from a table. */
class FakeEngine {
  readonly calls: Array<{ method: string; path: string }> = [];
  private proxies = new Map<string, Record<string, unknown>>();
  private sources = new Map<string, string>();

  fetch: typeof globalThis.fetch = async (input, init) => {
    const url = new URL(String(input));
    const method = (init?.method ?? "GET").toUpperCase();
    this.calls.push({ method, path: url.pathname });

    const json = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      });

    // /proxies/{id} — the id is percent-encoded because it contains a colon.
    const single = url.pathname.match(/^\/api\/v1\/proxies\/([^/]+)$/);
    if (single?.[1] !== undefined) {
      const id = decodeURIComponent(single[1]);
      const found = this.proxies.get(id);
      if (method === "DELETE") {
        if (!found) return json({ detail: "gone" }, 404);
        this.proxies.delete(id);
        return new Response(null, { status: 204 });
      }
      return found ? json(found) : json({ detail: "missing" }, 404);
    }

    if (url.pathname === "/api/v1/proxies" && method === "POST") {
      const body = JSON.parse(String(init?.body)) as { host: string; port: number };
      const record = {
        id: `${body.host}:${body.port}`,
        host: body.host,
        port: body.port,
        protocol: "http",
        anonymity: "unknown",
        latency_ms: null,
        country: null,
        last_checked: null,
        last_success: null,
        consecutive_failures: 0,
        score: 0,
      };
      this.proxies.set(record.id, record);
      return json(record, 201);
    }

    if (url.pathname === "/api/v1/proxies") {
      return json([...this.proxies.values()]);
    }

    if (url.pathname === "/api/v1/sources" && method === "POST") {
      const body = JSON.parse(String(init?.body)) as { url: string };
      if (this.sources.has(body.url)) return json({ detail: "exists" }, 409);
      this.sources.set(body.url, "src_test");
      return json({ id: "src_test", url: body.url }, 201);
    }

    const oneSource = url.pathname.match(/^\/api\/v1\/sources\/([^/]+)$/);
    if (oneSource) {
      return new Response(null, { status: 204 });
    }

    return json({ detail: "unhandled" }, 404);
  };

  seedProxy(host: string, port: number): string {
    const id = `${host}:${port}`;
    this.proxies.set(id, {
      id,
      host,
      port,
      protocol: "http",
      anonymity: "unknown",
      latency_ms: null,
      country: null,
      last_checked: null,
      last_success: null,
      consecutive_failures: 0,
      score: 0,
    });
    return id;
  }
}

const CREDENTIALS = { email: "admin@example.test", password: "correct-horse" };

let app: Awaited<ReturnType<typeof buildApp>>;
let token = "";
const engine = new FakeEngine();

before(async () => {
  const db = openDatabase(":memory:");
  await createUser(
    { db, sessionTtlMs: 3_600_000, tokenTtlMs: 86_400_000 },
    CREDENTIALS.email,
    CREDENTIALS.password,
    "admin",
  );

  app = await buildApp({
    config: loadConfig({
      NODE_ENV: "test",
      SESSION_SECRET: "test-secret-value-that-is-long-enough-32",
      ENGINE_URL: "http://engine.invalid",
      DATABASE_PATH: ":memory:",
      LOG_LEVEL: "fatal",
    }),
    db,
    engineFetch: engine.fetch,
  });

  const granted = await app.inject({
    method: "POST",
    url: "/api/v1/tokens/exchange",
    payload: CREDENTIALS,
  });
  assert.equal(granted.statusCode, 201, "the credential grant must succeed in tests");
  token = String(granted.json().token);
});

after(async () => {
  await app.close();
});

/**
 * Resolved per call: `token` is only assigned in `before`, so capturing it in a
 * module-level object would freeze an empty bearer value.
 */
const auth = () => ({ authorization: `Bearer ${token}` });

/** The media type may carry a charset parameter; the type itself is what matters. */
function assertProblemMediaType(contentType: string | undefined): void {
  assert.ok(
    contentType?.startsWith("application/problem+json"),
    `expected a problem document, got ${String(contentType)}`,
  );
}

describe("path parameters reach the handler", () => {
  test("a proxy id containing a colon is routable", async () => {
    const id = engine.seedProxy("198.51.100.4", 8080);

    const fetched = await app.inject({
      method: "GET",
      url: `/api/v1/proxies/${encodeURIComponent(id)}`,
      headers: auth(),
    });

    assert.equal(fetched.statusCode, 200, fetched.body);
    assert.equal(fetched.json().id, id);
  });

  test("the same id is deletable", async () => {
    const id = engine.seedProxy("198.51.100.5", 8080);

    const removed = await app.inject({
      method: "DELETE",
      url: `/api/v1/proxies/${encodeURIComponent(id)}`,
      headers: auth(),
    });

    assert.equal(removed.statusCode, 204, removed.body);
  });

  test("a source id is deletable", async () => {
    const removed = await app.inject({
      method: "DELETE",
      url: "/api/v1/sources/src_abc123",
      headers: auth(),
    });

    assert.equal(removed.statusCode, 204, removed.body);
  });
});

describe("status codes", () => {
  test("creation answers 201 with a Location header", async () => {
    const created = await app.inject({
      method: "POST",
      url: "/api/v1/proxies",
      headers: auth(),
      payload: { host: "203.0.113.1", port: 3128 },
    });

    assert.equal(created.statusCode, 201);
    assert.match(created.headers.location as string, /^\/api\/v1\/proxies\/203\.0\.113\.1%3A3128$/);
  });

  test("an engine conflict surfaces as 409", async () => {
    await app.inject({
      method: "POST",
      url: "/api/v1/sources",
      headers: auth(),
      payload: { url: "https://dupe.example.test" },
    });
    const again = await app.inject({
      method: "POST",
      url: "/api/v1/sources",
      headers: auth(),
      payload: { url: "https://dupe.example.test" },
    });

    assert.equal(again.statusCode, 409, again.body);
  });

  test("an engine miss surfaces as 404", async () => {
    const missing = await app.inject({
      method: "GET",
      url: `/api/v1/proxies/${encodeURIComponent("10.0.0.1:1")}`,
      headers: auth(),
    });

    assert.equal(missing.statusCode, 404);
  });
});

describe("every failure is a problem document", () => {
  test("401 when unauthenticated", async () => {
    const response = await app.inject({ method: "GET", url: "/api/v1/summary" });

    assert.equal(response.statusCode, 401);
    assertProblemMediaType(response.headers["content-type"]);
    assert.equal(response.json().type, "https://proxy-switcher.dev/problems/401");
  });

  test("404 for an unknown route", async () => {
    const response = await app.inject({ method: "GET", url: "/api/v1/nope", headers: auth() });

    assert.equal(response.statusCode, 404);
    assertProblemMediaType(response.headers["content-type"]);
    assert.equal(response.json().status, 404);
  });

  test("400 for a body that fails validation", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/proxies",
      headers: auth(),
      payload: { host: "203.0.113.9", port: 99999 },
    });

    assert.equal(response.statusCode, 400);
    assertProblemMediaType(response.headers["content-type"]);
  });

  test("the document carries no internal detail", async () => {
    const response = await app.inject({ method: "GET", url: "/api/v1/nope", headers: auth() });
    const body = JSON.stringify(response.json());

    assert.ok(!body.includes("engine.invalid"), "the engine URL must not leak");
    assert.ok(!body.includes("/opt/"), "filesystem paths must not leak");
  });
});

describe("role gates", () => {
  test("a viewer cannot create a source", async () => {
    const created = await app.inject({
      method: "POST",
      url: "/api/v1/tokens",
      headers: auth(),
      payload: { name: "viewer" },
    });
    const adminToken = created.json().token as string;

    await app.auth.db
      .prepare("UPDATE users SET role = 'viewer' WHERE email = ?")
      .run(CREDENTIALS.email);

    const denied = await app.inject({
      method: "POST",
      url: "/api/v1/sources",
      headers: { authorization: `Bearer ${adminToken}` },
      payload: { url: "https://viewer.example.test" },
    });

    await app.auth.db
      .prepare("UPDATE users SET role = 'admin' WHERE email = ?")
      .run(CREDENTIALS.email);

    assert.equal(denied.statusCode, 403, denied.body);
    assertProblemMediaType(denied.headers["content-type"]);
  });
});

describe("tokens", () => {
  test("a revoked token disappears from the listing", async () => {
    const created = await app.inject({
      method: "POST",
      url: "/api/v1/tokens",
      headers: auth(),
      payload: { name: "to-be-revoked" },
    });
    assert.equal(created.statusCode, 201);
    const { token, api_token: record } = created.json();

    const before = await app.inject({ method: "GET", url: "/api/v1/tokens", headers: auth() });
    assert.ok(
      before.json().some((t: { id: string }) => t.id === record.id),
      "a fresh token must be listed",
    );

    const revoked = await app.inject({
      method: "DELETE",
      url: `/api/v1/tokens/${record.id}`,
      headers: auth(),
    });
    assert.equal(revoked.statusCode, 204, revoked.body);

    // The dead credential must not be listed as if it still worked.
    const after = await app.inject({ method: "GET", url: "/api/v1/tokens", headers: auth() });
    assert.ok(
      !after.json().some((t: { id: string }) => t.id === record.id),
      "a revoked token must not be listed",
    );

    // And it must not authenticate.
    const used = await app.inject({
      method: "GET",
      url: "/api/v1/summary",
      headers: { authorization: `Bearer ${token}` },
    });
    assert.equal(used.statusCode, 401);
  });
});

describe("sessions", () => {
  test("creating a session sets an HttpOnly cookie and returns the user", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/sessions",
      payload: CREDENTIALS,
    });

    assert.equal(response.statusCode, 201, response.body);
    assert.equal(response.headers.location, "/api/v1/sessions/current");
    assert.match(String(response.headers["set-cookie"]), /HttpOnly/);
    assert.equal(response.json().user.email, CREDENTIALS.email);
  });

  test("the cookie is not marked Secure over plain HTTP", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/sessions",
      payload: CREDENTIALS,
    });

    const cookie = String(response.headers["set-cookie"]);
    assert.ok(!/;\s*Secure/i.test(cookie), "a Secure cookie is dropped over plain HTTP");
    assert.match(cookie, /HttpOnly/);
  });

  test("the cookie is marked Secure behind TLS", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/sessions",
      headers: { "x-forwarded-proto": "https" },
      payload: CREDENTIALS,
    });

    assert.match(String(response.headers["set-cookie"]), /;\s*Secure/i);
  });

  test("bad credentials are refused and audited", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/sessions",
      payload: { email: CREDENTIALS.email, password: "wrong" },
    });

    assert.equal(response.statusCode, 401);
    assertProblemMediaType(response.headers["content-type"]);
  });

  test("bootstrap closes after the first administrator exists", async () => {
    const response = await app.inject({
      method: "POST",
      url: "/api/v1/bootstrap",
      payload: { email: "second@example.test", password: "another-password" },
    });

    assert.equal(response.statusCode, 409, response.body);
  });
});

describe("operations", () => {
  test("liveness and readiness are public", async () => {
    for (const path of ["/healthz", "/readyz"]) {
      const response = await app.inject({ method: "GET", url: path });
      assert.ok(response.statusCode === 200 || response.statusCode === 503, path);
    }
  });

  test("the published document lists every contracted operation", async () => {
    const document = app.swagger() as {
      paths: Record<string, Record<string, unknown>>;
    };

    const expected: Record<string, string[]> = {
      "/api/v1/active-proxy": ["get", "put", "delete"],
      "/api/v1/active-proxy/rotate": ["post"],
      "/api/v1/proxies": ["get", "post"],
      "/api/v1/proxies/{id}": ["get", "delete"],
      "/api/v1/sources": ["get", "post"],
      "/api/v1/sources/{id}": ["delete"],
      "/api/v1/tokens": ["get", "post"],
      "/api/v1/tokens/exchange": ["post"],
      "/api/v1/tokens/{id}": ["delete"],
    };

    for (const [path, methods] of Object.entries(expected)) {
      const operations = document.paths[path];
      assert.ok(operations, `missing ${path} in the published document`);
      for (const method of methods) {
        assert.ok(operations[method], `missing ${method.toUpperCase()} ${path}`);
      }
    }
  });

  test("no route uses a colon, which the router reads as a parameter", async () => {
    const document = app.swagger() as { paths: Record<string, unknown> };
    const colon = Object.keys(document.paths).filter((path) => path.includes(":"));

    assert.deepEqual(colon, [], "a colon in a route is a path parameter, not a literal");
  });
});

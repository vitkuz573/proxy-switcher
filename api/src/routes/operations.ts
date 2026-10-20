import { Type } from "@sinclair/typebox";
import type { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { EngineError } from "../engine/client.js";
import { describeEngineError, PROBLEM_CONTENT_TYPE } from "../plugins/errors.js";
import {
  AnonymitySchema,
  ProxyPolicySettingsSchema,
  ProxyProtocolSchema,
  Ref,
} from "../schemas/index.js";
import { recordAudit } from "../services/auth.js";

/** Default and ceiling for collection size. The pool reaches tens of thousands. */
const DEFAULT_LIMIT = 50;
const MAX_LIMIT = 500;

type EngineOutcome<T> = { ok: true; value: T } | { ok: false; reply: FastifyReply };

async function callEngine<T>(reply: FastifyReply, fn: () => Promise<T>): Promise<EngineOutcome<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (err) {
    if (err instanceof EngineError) {
      const instance = reply.request.url;
      const { status, body } = describeEngineError(err, instance);
      return { ok: false, reply: reply.code(status).type(PROBLEM_CONTENT_TYPE).send(body) };
    }
    throw err;
  }
}

function audit(
  app: FastifyInstance,
  req: FastifyRequest,
  action: string,
  target: string | null,
): void {
  const principal = req.principal;
  if (!principal) {
    return;
  }
  recordAudit(app.auth.db, {
    actorType: principal.kind === "token" ? "token" : "user",
    actorId: principal.tokenId ?? principal.userId,
    action,
    target,
    outcome: "allowed",
    ip: app.clientIp(req),
  });
}

const ListQuery = Type.Object({
  limit: Type.Optional(Type.Integer({ minimum: 1, maximum: MAX_LIMIT })),
  offset: Type.Optional(Type.Integer({ minimum: 0 })),
  sort: Type.Optional(
    Type.Union([Type.Literal("score"), Type.Literal("latency"), Type.Literal("host")]),
  ),
  search: Type.Optional(Type.String({ maxLength: 128 })),
});

export async function operationRoutes(app: FastifyInstance): Promise<void> {
  /* ── Proxies ─────────────────────────────────────────────────────────── */

  app.get(
    "/api/v1/proxies",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["proxies"],
        summary: "List proxies",
        querystring: ListQuery,
        response: {
          200: { type: "array", items: Ref("Proxy") },
          502: Ref("Problem"),
        },
      },
    },
    async (req, reply) => {
      const q = req.query as {
        limit?: number;
        offset?: number;
        sort?: string;
        search?: string;
      };

      const result = await callEngine(reply, () =>
        app.engine.listProxies({
          limit: q.limit ?? DEFAULT_LIMIT,
          offset: q.offset ?? 0,
          ...(q.sort ? { sort: q.sort } : {}),
          ...(q.search ? { search: q.search } : {}),
        }),
      );

      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    "/api/v1/proxies",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["proxies"],
        summary: "Add a proxy to the pool",
        body: Type.Object({
          host: Type.String({ minLength: 1, maxLength: 255 }),
          port: Type.Integer({ minimum: 1, maximum: 65535 }),
          protocol: Type.Optional(ProxyProtocolSchema),
          country: Type.Optional(Type.String({ maxLength: 64 })),
          anonymity: Type.Optional(AnonymitySchema),
        }),
        response: { 201: Ref("Proxy"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const body = req.body as {
        host: string;
        port: number;
        protocol?: "http" | "https" | "socks4" | "socks5";
        country?: string;
      };

      const result = await callEngine(reply, () =>
        app.engine.createProxy({
          host: body.host,
          port: body.port,
          protocol: body.protocol ?? "http",
          ...(body.country ? { country: body.country } : {}),
        }),
      );

      if (!result.ok) {
        return result.reply;
      }

      // 201 carries the created resource and points at its canonical URL.
      reply.header("location", `/api/v1/proxies/${encodeURIComponent(result.value.id)}`);
      audit(app, req, "proxy.create", result.value.id);
      return reply.code(201).send(result.value);
    },
  );

  app.get(
    "/api/v1/proxies/:id",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["proxies"],
        summary: "Retrieve one proxy",
        params: Type.Object({ id: Type.String({ minLength: 1 }) }),
        response: { 200: Ref("Proxy"), 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { id } = req.params as { id: string };
      const result = await callEngine(reply, () => app.engine.getProxy(id));
      return result.ok ? result.value : result.reply;
    },
  );

  app.delete(
    "/api/v1/proxies/:id",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["proxies"],
        summary: "Remove a proxy from the pool",
        params: Type.Object({ id: Type.String({ minLength: 1 }) }),
        response: { 204: { type: "null" }, 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { id } = req.params as { id: string };
      const result = await callEngine(reply, () => app.engine.deleteProxy(id));

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, "proxy.delete", id);
      return reply.code(204).send();
    },
  );

  /* ── Active proxy: a singleton resource, not an action on the pool ──── */

  app.get(
    "/api/v1/active-proxy",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["proxies"],
        summary: "The selected proxy and pool counters",
        response: { 200: Ref("Summary"), 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getActiveProxy());
      return result.ok ? result.value : result.reply;
    },
  );

  app.put(
    "/api/v1/active-proxy",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["proxies"],
        summary: "Select the proxy traffic is routed through",
        body: Type.Object({ proxy_id: Type.String({ minLength: 1 }) }),
        response: { 200: Ref("Proxy"), 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { proxy_id } = req.body as { proxy_id: string };
      const result = await callEngine(reply, () => app.engine.setActiveProxy(proxy_id));

      if (result.ok) {
        audit(app, req, "active-proxy.set", proxy_id);
        return result.value;
      }
      return result.reply;
    },
  );

  app.delete(
    "/api/v1/active-proxy",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["proxies"],
        summary: "Clear the selection",
        response: { 204: { type: "null" }, 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const result = await callEngine(reply, () => app.engine.clearActiveProxy());

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, "active-proxy.clear", null);
      return reply.code(204).send();
    },
  );

  // Nested under the singleton, not `active-proxy:rotate`: a colon starts a
  // path parameter, so the colon form would match any suffix.
  app.post(
    "/api/v1/active-proxy/rotate",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["proxies"],
        summary: "Advance to the next proxy in the pool",
        response: { 200: Ref("Proxy"), 409: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const result = await callEngine(reply, () => app.engine.rotateActiveProxy());

      if (result.ok) {
        audit(app, req, "active-proxy.rotate", result.value?.id ?? null);
        return result.value;
      }
      return result.reply;
    },
  );

  /* ── Scrape sources ──────────────────────────────────────────────────── */

  app.get(
    "/api/v1/sources",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["scraper"],
        summary: "List scrape sources",
        response: { 200: { type: "array", items: Ref("Source") }, 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.listSources());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    "/api/v1/sources",
    {
      preHandler: [app.authenticate, app.requireRole("admin")],
      schema: {
        tags: ["scraper"],
        summary: "Register a scrape source",
        body: Type.Object({ url: Type.String({ minLength: 1, format: "uri", maxLength: 2048 }) }),
        response: { 201: Ref("Source"), 409: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { url } = req.body as { url: string };
      const result = await callEngine(reply, () => app.engine.createSource(url));

      if (!result.ok) {
        return result.reply;
      }

      reply.header("location", `/api/v1/sources/${encodeURIComponent(result.value.id)}`);
      audit(app, req, "source.create", result.value.id);
      return reply.code(201).send(result.value);
    },
  );

  app.delete(
    "/api/v1/sources/:id",
    {
      preHandler: [app.authenticate, app.requireRole("admin")],
      schema: {
        tags: ["scraper"],
        summary: "Remove a scrape source by id",
        params: Type.Object({ id: Type.String({ minLength: 1 }) }),
        response: { 204: { type: "null" }, 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { id } = req.params as { id: string };
      const result = await callEngine(reply, () => app.engine.deleteSource(id));

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, "source.delete", id);
      return reply.code(204).send();
    },
  );

  /* ── Scrape runs ─────────────────────────────────────────────────────── */

  app.post(
    "/api/v1/scrape-runs",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["scraper"],
        summary: "Start a scrape run",
        response: { 201: { type: "null" }, 409: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const result = await callEngine(reply, () => app.engine.createScrapeRun());

      if (!result.ok) {
        return result.reply;
      }

      reply.header("location", "/api/v1/scrape-runs/current");
      audit(app, req, "scrape-run.create", null);
      return reply.code(201).send();
    },
  );

  app.get(
    "/api/v1/scrape-runs/current",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["scraper"],
        summary: "The latest scrape run and health-check progress",
        response: { 200: Ref("ScrapeRun"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getCurrentScrapeRun());
      return result.ok ? result.value : result.reply;
    },
  );

  /* ── Routing ────────────────────────────────────────────────────────── */

  app.get(
    "/api/v1/statistics",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["statistics"],
        summary: "The distribution of the pool, and how it has moved",
        response: { 200: Ref("PoolStatistics"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getStatistics());
      return result.ok ? result.value : result.reply;
    },
  );

  app.get(
    "/api/v1/routing",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["routing"],
        summary: "The routing policy in force and the state of the TUN bypass",
        response: { 200: Ref("RoutingInfo"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getRouting());
      return result.ok ? result.value : result.reply;
    },
  );

  /* ── Telemetry ───────────────────────────────────────────────────────── */

  app.get(
    "/api/v1/summary",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["telemetry"],
        summary: "Pool size, healthy count and active proxy",
        response: { 200: Ref("Summary"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getSummary());
      return result.ok ? result.value : result.reply;
    },
  );

  app.get(
    "/api/v1/stats",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["telemetry"],
        summary: "Live connection and pool counters",
        response: { 200: Ref("Stats"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getStats());
      return result.ok ? result.value : result.reply;
    },
  );

  /* ── The local proxy, its policies and its credentials ───────────────── */

  app.get(
    "/api/v1/local-proxy",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["local-proxy"],
        summary: "The local proxy a program can be pointed at",
        response: { 200: Ref("LocalProxy"), 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getLocalProxy());
      return result.ok ? result.value : result.reply;
    },
  );

  app.get(
    "/api/v1/policies",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["local-proxy"],
        summary: "The policies a credential may carry",
        response: { 200: { type: "array", items: Ref("ProxyPolicy") }, 502: Ref("Problem") },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.listPolicies());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    "/api/v1/policies",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["local-proxy"],
        summary: "Write a policy down",
        body: Type.Object({
          name: Type.String({ minLength: 1, maxLength: 64 }),
          settings: ProxyPolicySettingsSchema,
        }),
        response: { 201: Ref("ProxyPolicy"), 400: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const body = req.body as { name: string; settings: never };
      const result = await callEngine(reply, () => app.engine.createPolicy(body));
      if (!result.ok) return result.reply;

      audit(app, req, "policy.create", body.name);
      return reply.code(201).send(result.value);
    },
  );

  app.delete(
    "/api/v1/policies/:name",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["local-proxy"],
        summary: "Remove a policy",
        params: Type.Object({ name: Type.String() }),
        response: { 204: { type: "null" }, 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { name } = req.params as { name: string };
      const result = await callEngine(reply, () => app.engine.deletePolicy(name));
      if (!result.ok) return result.reply;

      audit(app, req, "policy.delete", name);
      return reply.code(204).send();
    },
  );

  app.get(
    "/api/v1/credentials",
    {
      preHandler: [app.authenticate, app.requireRole("viewer")],
      schema: {
        tags: ["local-proxy"],
        summary: "The credentials the local proxy accepts",
        response: {
          200: { type: "array", items: Ref("ProxyCredential") },
          502: Ref("Problem"),
        },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.listCredentials());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    "/api/v1/credentials",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["local-proxy"],
        summary: "Create a credential, and return its password once",
        body: Type.Object({
          username: Type.String({ minLength: 1, maxLength: 64 }),
          policy: Type.String({ minLength: 1, maxLength: 64 }),
        }),
        response: { 201: Ref("ProxyCredential"), 400: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const body = req.body as { username: string; policy: string };
      const result = await callEngine(reply, () => app.engine.createCredential(body));
      if (!result.ok) return result.reply;

      audit(app, req, "credential.create", body.username);
      return reply.code(201).send(result.value);
    },
  );

  app.delete(
    "/api/v1/credentials/:username",
    {
      preHandler: [app.authenticate, app.requireRole("operator")],
      schema: {
        tags: ["local-proxy"],
        summary: "Remove a credential",
        params: Type.Object({ username: Type.String() }),
        response: { 204: { type: "null" }, 404: Ref("Problem"), 502: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const { username } = req.params as { username: string };
      const result = await callEngine(reply, () => app.engine.deleteCredential(username));
      if (!result.ok) return result.reply;

      audit(app, req, "credential.delete", username);
      return reply.code(204).send();
    },
  );

  /* ── Audit ───────────────────────────────────────────────────────────── */

  app.get(
    "/api/v1/audit-events",
    {
      preHandler: [app.authenticate, app.requireRole("admin")],
      schema: {
        tags: ["audit"],
        summary: "Read the audit log",
        querystring: Type.Object({
          limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 500, default: 100 })),
          offset: Type.Optional(Type.Integer({ minimum: 0, default: 0 })),
        }),
        response: { 200: { type: "array", items: Ref("AuditEvent") } },
      },
    },
    async (req) => {
      const { limit, offset } = req.query as { limit: number; offset: number };
      const rows = app.auth.db
        .prepare("SELECT * FROM audit_log ORDER BY ts DESC LIMIT ? OFFSET ?")
        .all(limit, offset) as Array<Record<string, unknown>>;

      return rows.map((row) => ({
        id: row.id as number,
        ts: new Date(row.ts as number).toISOString(),
        actor_type: row.actor_type as string,
        actor_id: (row.actor_id as string | null) ?? null,
        action: row.action as string,
        target: (row.target as string | null) ?? null,
        outcome: row.outcome as string,
        ip: (row.ip as string | null) ?? null,
        detail: (row.detail as string | null) ?? null,
      }));
    },
  );
}

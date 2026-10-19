import type { DatabaseSync } from "node:sqlite";
import cookie from "@fastify/cookie";
import helmet from "@fastify/helmet";
import rateLimit from "@fastify/rate-limit";
import swagger from "@fastify/swagger";
import swaggerUi from "@fastify/swagger-ui";
import addFormats from "ajv-formats";
import Fastify, {
  type FastifyError,
  type FastifyInstance,
  type FastifyServerOptions,
} from "fastify";
import type { Config } from "./config.js";
import { EngineClient, isEngineHealthy } from "./engine/client.js";
import { authPlugin } from "./plugins/auth.js";
import { enginePlugin } from "./plugins/engine.js";
import { PROBLEM_CONTENT_TYPE, problem } from "./plugins/errors.js";
import { rbacPlugin } from "./plugins/rbac.js";
import { authRoutes } from "./routes/auth.js";
import { operationRoutes } from "./routes/operations.js";
import { SCHEMAS } from "./schemas/index.js";
import type { AuthContext } from "./services/auth.js";

/**
 * `ajv-formats` ships CommonJS typings that do not line up with Fastify's
 * `Plugin` alias, though the runtime shape is exactly what Fastify expects.
 */
const AJV_PLUGINS = [addFormats] as unknown as NonNullable<
  NonNullable<FastifyServerOptions["ajv"]>["plugins"]
>;

/**
 * @fastify/swagger names every shared component `def-0`, `def-1`, ... unless
 * this is overridden. Returning `$id` keeps named types (`Proxy`, `Stats`) in the
 * document, which is what the generated SDK keys off.
 */
const REF_RESOLVER = {
  buildLocalReference(json: { $id?: string; title?: string }): string {
    if (!json.title && json.$id) {
      json.title = json.$id;
    }
    return json.$id ?? json.title ?? "schema";
  },
};

export interface BuildAppOptions {
  config: Config;
  db: DatabaseSync;
  /**
   * Transport for the internal engine. Contract tests substitute a stand-in so
   * routing and status mapping can be verified without a running daemon.
   */
  engineFetch?: typeof globalThis.fetch;
}

export async function buildApp({
  config,
  db,
  engineFetch,
}: BuildAppOptions): Promise<FastifyInstance> {
  const app: FastifyInstance = Fastify({
    logger: {
      level: config.LOG_LEVEL,
      // Never log cookies or authorization headers.
      redact: ["req.headers.authorization", "req.headers.cookie"],
    },
    trustProxy: true,
    bodyLimit: 1_048_576,
    ajv: {
      // Without this, `format: "email"` and `format: "uri"` are unknown to ajv
      // and every such request is rejected as invalid.
      plugins: AJV_PLUGINS,
    },
  });

  const auth: AuthContext = {
    db,
    sessionTtlMs: config.SESSION_TTL_HOURS * 3_600_000,
    tokenTtlMs: config.API_TOKEN_TTL_DAYS * 86_400_000,
  };

  // Registered before swagger so every `Type.Ref` resolves to a named component.
  // Fastify v5 derives the shared-schema key from the schema's `$id`.
  for (const schema of Object.values(SCHEMAS)) {
    app.addSchema(schema);
  }

  // Registered before routes so the generated document sees every schema.
  await app.register(swagger, {
    openapi: {
      openapi: "3.1.0",
      info: {
        title: "Proxy Switcher API",
        version: "2.0.0",
        description:
          "Public control plane for the proxy-switcher engine. Two client models are " +
          "supported: browsers authenticate with a session cookie, CLIs with a bearer token.",
      },
      servers: [{ url: `http://${config.API_HOST}:${config.API_PORT}` }],
      components: {
        securitySchemes: {
          sessionCookie: { type: "apiKey", in: "cookie", name: "ps_session" },
          bearerToken: { type: "http", scheme: "bearer" },
        },
      },
      security: [{ sessionCookie: [] }, { bearerToken: [] }],
    },
    // Without this the document names every component `def-0`, `def-1`, ... and
    // the generated SDK ends up with anonymous structural types.
    refResolver: REF_RESOLVER,
  });

  await app.register(swaggerUi, { routePrefix: "/docs" });

  await app.register(helmet, {
    contentSecurityPolicy: false,
  });

  await app.register(cookie, { secret: config.SESSION_SECRET });

  await app.register(rateLimit, {
    global: false,
    max: config.RATE_LIMIT_MAX,
    timeWindow: config.RATE_LIMIT_WINDOW_MS,
    keyGenerator: (req) => req.principal?.tokenId ?? req.principal?.userId ?? req.ip,
  });

  await app.register(enginePlugin, { engine: EngineClient.fromConfig(config, engineFetch) });
  await app.register(authPlugin, { auth });
  await app.register(rbacPlugin);

  /* ── Operations: public, no auth ─────────────────────────────────────── */

  /**
   * Every failure leaves as an RFC 9457 problem document. Clients get one shape
   * to handle, and internal detail never crosses the boundary.
   *
   * Installed before any route: Fastify captures the handler when a route is
   * registered, so one added afterwards never sees validation failures.
   */
  app.setErrorHandler((err: FastifyError, req, reply) => {
    const status = err.statusCode ?? 500;
    const instance = req.url;

    if (status >= 500) {
      req.log.error({ err }, "unhandled error");
    }

    const send = (code: number, detail: string) =>
      reply
        .code(code)
        .type(PROBLEM_CONTENT_TYPE)
        .send(problem(code, detail, { instance }));

    if (err.validation) {
      return send(400, "Request validation failed");
    }
    if (status === 429) {
      return send(429, "Rate limit exceeded");
    }
    if (status >= 400 && status < 500) {
      return send(status, err.message);
    }

    // Internal detail stays in the log, never in the response.
    return send(500, "Internal server error");
  });

  app.setNotFoundHandler(async (req, reply) => {
    return reply
      .code(404)
      .type(PROBLEM_CONTENT_TYPE)
      .send(problem(404, `No route for ${req.method} ${req.url}`, { instance: req.url }));
  });

  // Kubernetes-style names: /healthz is liveness, /readyz is readiness.
  app.get(
    "/healthz",
    {
      schema: { tags: ["ops"], summary: "Liveness probe", hide: true },
    },
    async () => ({ status: "ok" }),
  );

  app.get(
    "/readyz",
    {
      schema: {
        tags: ["ops"],
        summary: "Readiness probe including engine reachability",
      },
    },
    async (_req, reply) => {
      const engineOk = await isEngineHealthy(app.engine);
      return reply.code(engineOk ? 200 : 503).send({
        status: engineOk ? "ready" : "degraded",
        engine: engineOk ? "up" : "down",
      });
    },
  );

  await app.register(async (instance) => {
    await authRoutes(instance);
    await operationRoutes(instance);
  });

  return app;
}

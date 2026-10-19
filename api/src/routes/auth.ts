import { Type } from "@sinclair/typebox";
import type { FastifyInstance, FastifyRequest } from "fastify";
import { SESSION_COOKIE } from "../plugins/auth.js";
import { PROBLEM_CONTENT_TYPE, problem } from "../plugins/errors.js";
import { Ref } from "../schemas/index.js";
import {
  authenticate,
  countUsers,
  createApiToken,
  createSession,
  createUser,
  destroySession,
  listApiTokens,
  recordAudit,
  revokeApiToken,
} from "../services/auth.js";

/**
 * Brute-force damping for credential endpoints. In-memory on purpose: a speed
 * bump against online guessing, not a durable quota.
 */
class AttemptLimiter {
  private readonly attempts = new Map<string, { count: number; resetAt: number }>();

  constructor(
    private readonly max: number,
    private readonly windowMs: number,
  ) {}

  check(key: string): { allowed: boolean; retryAfterSec: number } {
    const now = Date.now();
    const entry = this.attempts.get(key);

    if (!entry || entry.resetAt <= now) {
      this.attempts.set(key, { count: 1, resetAt: now + this.windowMs });
      this.sweep(now);
      return { allowed: true, retryAfterSec: 0 };
    }

    entry.count += 1;
    if (entry.count > this.max) {
      return { allowed: false, retryAfterSec: Math.ceil((entry.resetAt - now) / 1000) };
    }
    return { allowed: true, retryAfterSec: 0 };
  }

  clear(key: string): void {
    this.attempts.delete(key);
  }

  private sweep(now: number): void {
    if (this.attempts.size < 1000) {
      return;
    }
    for (const [key, entry] of this.attempts) {
      if (entry.resetAt <= now) {
        this.attempts.delete(key);
      }
    }
  }
}

const toPublicUser = (user: {
  id: string;
  email: string;
  role: "admin" | "operator" | "viewer";
  created_at: number;
}) => ({
  id: user.id,
  email: user.email,
  role: user.role,
  created_at: new Date(user.created_at).toISOString(),
});

const toPublicToken = (row: {
  id: string;
  name: string;
  prefix: string;
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
}) => ({
  id: row.id,
  name: row.name,
  prefix: row.prefix,
  created_at: new Date(row.created_at).toISOString(),
  expires_at: row.expires_at === null ? null : new Date(row.expires_at).toISOString(),
  last_used_at: row.last_used_at === null ? null : new Date(row.last_used_at).toISOString(),
});

/**
 * The authenticated principal.
 *
 * `app.authenticate` has already run and rejects with 401 when there is none, so
 * the remaining case is a programming error — reaching a handler that declares
 * it but forgot the guard. Failing loudly beats an assertion that silently
 * produces `undefined`.
 */
function principalOf(req: FastifyRequest): NonNullable<FastifyRequest["principal"]> {
  const principal = req.principal;
  if (!principal) {
    throw new Error(`${req.method} ${req.url} is missing the authenticate preHandler`);
  }
  return principal;
}

const Credentials = Type.Object({
  email: Type.String({ format: "email", maxLength: 320 }),
  // Input length only; stored hashes are argon2id regardless.
  password: Type.String({ minLength: 1, maxLength: 200 }),
});

export async function authRoutes(app: FastifyInstance): Promise<void> {
  const limiter = new AttemptLimiter(10, 15 * 60_000);
  const db = app.auth.db;

  /**
   * The session cookie is marked `Secure` only when the request actually
   * arrived over TLS.
   *
   * Keying it off `NODE_ENV` instead is wrong in both directions: behind a
   * plain-HTTP loopback deployment the browser silently refuses to store the
   * cookie, so login never completes, while a real TLS deployment reached
   * through a proxy needs the flag whatever the environment says.
   * `trustProxy` is enabled, so `req.protocol` already reflects
   * `X-Forwarded-Proto` from a terminating reverse proxy.
   */
  const cookieOptions = (req: FastifyRequest) => ({
    path: "/",
    httpOnly: true,
    sameSite: "lax" as const,
    secure: req.protocol === "https",
  });

  /* ── Sessions (browser) ──────────────────────────────────────────────── */

  app.post(
    "/api/v1/sessions",
    {
      schema: {
        tags: ["auth"],
        summary: "Authenticate a browser and start a session",
        body: Credentials,
        response: {
          201: { type: "object", properties: { user: Ref("User") } },
          401: Ref("Problem"),
          429: Ref("Problem"),
        },
      },
    },
    async (req, reply) => {
      const { email, password } = req.body as { email: string; password: string };
      const ip = app.clientIp(req);

      const limit = limiter.check(`${ip ?? "unknown"}:${email.toLowerCase()}`);
      if (!limit.allowed) {
        return reply
          .code(429)
          .header("retry-after", String(limit.retryAfterSec))
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(429, "Too many failed attempts"));
      }

      const user = await authenticate(app.auth, email, password);

      if (!user) {
        recordAudit(db, {
          actorType: "anonymous",
          actorId: null,
          action: "session.create",
          outcome: "denied",
          ip,
          detail: email.toLowerCase(),
        });
        return reply
          .code(401)
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(401, "Invalid email or password"));
      }

      limiter.clear(`${ip ?? "unknown"}:${email.toLowerCase()}`);

      const session = createSession(app.auth, user.id, {
        ...(ip === null ? {} : { ip }),
        ...(req.headers["user-agent"] === undefined
          ? {}
          : { userAgent: req.headers["user-agent"] }),
      });

      reply.setCookie(SESSION_COOKIE, session.id, {
        ...cookieOptions(req),
        expires: new Date(session.expires_at),
      });

      recordAudit(db, {
        actorType: "user",
        actorId: user.id,
        action: "session.create",
        outcome: "allowed",
        ip,
      });

      // 201: a session was created, and Location points at it.
      reply.header("location", "/api/v1/sessions/current");
      return reply.code(201).send({ user: toPublicUser(user) });
    },
  );

  app.get(
    "/api/v1/sessions/current",
    {
      preHandler: app.authenticate,
      schema: {
        tags: ["auth"],
        summary: "The principal behind the current session",
        response: {
          200: { type: "object", properties: { user: Ref("User") } },
          401: Ref("Problem"),
        },
      },
    },
    async (req) => {
      const principal = principalOf(req);
      const row = app.auth.db
        .prepare("SELECT * FROM users WHERE id = ?")
        .get(principal.userId) as Parameters<typeof toPublicUser>[0];

      if (!row) {
        return {
          user: toPublicUser({
            id: principal.userId,
            email: principal.email,
            role: principal.role,
            created_at: 0,
          }),
        };
      }
      return { user: toPublicUser(row) };
    },
  );

  app.delete(
    "/api/v1/sessions/current",
    {
      schema: {
        tags: ["auth"],
        summary: "Destroy the current session",
        response: { 204: { type: "null" } },
      },
    },
    async (req, reply) => {
      const sessionId = req.cookies[SESSION_COOKIE];
      if (sessionId) {
        destroySession(app.auth, sessionId);
      }
      reply.clearCookie(SESSION_COOKIE, { path: "/" });

      if (req.principal) {
        recordAudit(db, {
          actorType: req.principal.kind === "token" ? "token" : "user",
          actorId: req.principal.tokenId ?? req.principal.userId,
          action: "session.delete",
          outcome: "allowed",
          ip: app.clientIp(req),
        });
      }

      return reply.code(204).send();
    },
  );

  /* ── Tokens ──────────────────────────────────────────────────────────── */

  app.post(
    "/api/v1/tokens",
    {
      preHandler: app.authenticate,
      schema: {
        tags: ["auth"],
        summary: "Issue an API token for the caller",
        body: Type.Object({ name: Type.String({ minLength: 1, maxLength: 64 }) }),
        response: { 201: Ref("CreatedApiToken"), 401: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const principal = principalOf(req);
      const { name } = req.body as { name: string };

      const { token, row } = createApiToken(app.auth, principal.userId, name);

      recordAudit(db, {
        actorType: principal.kind === "token" ? "token" : "user",
        actorId: principal.tokenId ?? principal.userId,
        action: "token.create",
        target: name,
        outcome: "allowed",
        ip: app.clientIp(req),
      });

      reply.header("location", `/api/v1/tokens/${row.id}`);
      return reply.code(201).send({ token, api_token: toPublicToken(row) });
    },
  );

  // Credential grant for CLIs. Public because this *is* the authentication step
  // for a client with no cookie jar; rate limited and audited like any attempt.
  //
  // `/exchange` sits beside `/tokens/{id}` safely: the router prefers a static
  // segment over a parameter, and token ids are UUIDs, so no real id can ever
  // collide with this name.
  app.post(
    "/api/v1/tokens/exchange",
    {
      schema: {
        tags: ["auth"],
        summary: "Exchange credentials for an API token (CLI sign-in)",
        body: Type.Object({
          ...Credentials.properties,
          name: Type.Optional(Type.String({ minLength: 1, maxLength: 64 })),
        }),
        response: {
          201: {
            type: "object",
            properties: {
              token: Type.String(),
              api_token: Ref("ApiToken"),
              user: Ref("User"),
            },
          },
          401: Ref("Problem"),
          429: Ref("Problem"),
        },
      },
    },
    async (req, reply) => {
      const { email, password, name } = req.body as {
        email: string;
        password: string;
        name?: string;
      };
      const ip = app.clientIp(req);

      const limit = limiter.check(`${ip ?? "unknown"}:${email.toLowerCase()}`);
      if (!limit.allowed) {
        return reply
          .code(429)
          .header("retry-after", String(limit.retryAfterSec))
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(429, "Too many failed attempts"));
      }

      const user = await authenticate(app.auth, email, password);

      if (!user) {
        recordAudit(db, {
          actorType: "anonymous",
          actorId: null,
          action: "token.exchange",
          outcome: "denied",
          ip,
          detail: email.toLowerCase(),
        });
        return reply
          .code(401)
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(401, "Invalid email or password"));
      }

      limiter.clear(`${ip ?? "unknown"}:${email.toLowerCase()}`);

      const { token, row } = createApiToken(
        app.auth,
        user.id,
        name?.trim() || `cli:${ip ?? "unknown"}`,
      );

      recordAudit(db, {
        actorType: "user",
        actorId: user.id,
        action: "token.exchange",
        target: row.prefix,
        outcome: "allowed",
        ip,
      });

      reply.header("location", `/api/v1/tokens/${row.id}`);
      return reply
        .code(201)
        .send({ token, api_token: toPublicToken(row), user: toPublicUser(user) });
    },
  );

  app.get(
    "/api/v1/tokens",
    {
      preHandler: app.authenticate,
      schema: {
        tags: ["auth"],
        summary: "List the caller's tokens",
        response: { 200: { type: "array", items: Ref("ApiToken") }, 401: Ref("Problem") },
      },
    },
    async (req) => listApiTokens(db, principalOf(req).userId).map(toPublicToken),
  );

  app.delete(
    "/api/v1/tokens/:id",
    {
      preHandler: app.authenticate,
      schema: {
        tags: ["auth"],
        summary: "Revoke a token",
        params: Type.Object({ id: Type.String({ format: "uuid" }) }),
        response: { 204: { type: "null" }, 404: Ref("Problem"), 401: Ref("Problem") },
      },
    },
    async (req, reply) => {
      const principal = principalOf(req);
      const { id } = req.params as { id: string };

      if (!revokeApiToken(db, principal.userId, id)) {
        return reply.code(404).type(PROBLEM_CONTENT_TYPE).send(problem(404, "Token not found"));
      }

      recordAudit(db, {
        actorType: principal.kind === "token" ? "token" : "user",
        actorId: principal.tokenId ?? principal.userId,
        action: "token.delete",
        target: id,
        outcome: "allowed",
        ip: app.clientIp(req),
      });

      return reply.code(204).send();
    },
  );

  /* ── First-run bootstrap ─────────────────────────────────────────────── */

  // Only usable while no users exist; becomes inert immediately afterwards.
  app.post(
    "/api/v1/bootstrap",
    {
      schema: {
        tags: ["auth"],
        summary: "Create the first administrator (refused once any user exists)",
        body: Type.Object({
          email: Type.String({ format: "email", maxLength: 320 }),
          // Floor lowered to 8 by operator decision (2026-10-19).
          password: Type.String({ minLength: 8, maxLength: 200 }),
        }),
        response: {
          201: { type: "object", properties: { user: Ref("User") } },
          409: Ref("Problem"),
        },
      },
    },
    async (req, reply) => {
      if (countUsers(db) > 0) {
        return reply
          .code(409)
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(409, "Bootstrap is already complete"));
      }

      const { email, password } = req.body as { email: string; password: string };
      const user = await createUser(app.auth, email, password, "admin");

      recordAudit(db, {
        actorType: "user",
        actorId: user.id,
        action: "bootstrap.create",
        outcome: "allowed",
        ip: app.clientIp(req),
      });

      return reply.code(201).send({ user: toPublicUser(user) });
    },
  );
}

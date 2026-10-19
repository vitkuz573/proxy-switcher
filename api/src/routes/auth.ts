import { Type } from '@sinclair/typebox';
import type { FastifyInstance } from 'fastify';
import { SESSION_COOKIE } from '../plugins/auth.js';
import { errorBody } from '../plugins/errors.js';
import { Ref } from '../schemas/index.js';
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
} from '../services/auth.js';

const LoginBody = Type.Object({
  email: Type.String({ format: 'email' }),
  password: Type.String({ minLength: 1 }),
});

/**
 * Brute-force damping for password endpoints. In-memory on purpose: it is a
 * speed bump against online guessing, not a durable quota.
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
  role: 'admin' | 'operator' | 'viewer';
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

export async function authRoutes(app: FastifyInstance, opts: { secureCookies: boolean }): Promise<void> {
  const limiter = new AttemptLimiter(10, 15 * 60_000);
  const db = app.auth.db;

  const cookieOptions = {
    path: '/',
    httpOnly: true,
    sameSite: 'lax' as const,
    secure: opts.secureCookies,
  };

  /* ── Session lifecycle (Web) ─────────────────────────────────────────── */

  app.post(
    '/api/v1/auth/login',
    {
      schema: {
        tags: ['auth'],
        summary: 'Exchange credentials for a session cookie',
        body: LoginBody,
        response: {
          200: { type: 'object', properties: { user: Ref('User') } },
          401: errorSchema(),
          429: errorSchema(),
        },
      },
    },
    async (req, reply) => {
      const { email, password } = req.body as { email: string; password: string };
      const ip = app.clientIp(req);
      const limiterKey = `${ip ?? 'unknown'}:${email.toLowerCase()}`;

      const limit = limiter.check(limiterKey);
      if (!limit.allowed) {
        return reply
          .code(429)
          .header('retry-after', String(limit.retryAfterSec))
          .send(errorBody(429, 'Too many failed attempts'));
      }

      const user = await authenticate(app.auth, email, password);

      if (!user) {
        recordAudit(db, {
          actorType: 'anonymous',
          actorId: null,
          action: 'auth.login',
          outcome: 'denied',
          ip,
          detail: email.toLowerCase(),
        });
        return reply.code(401).send(errorBody(401, 'Invalid email or password'));
      }

      limiter.clear(limiterKey);

      const session = createSession(app.auth, user.id, {
        ...(ip === null ? {} : { ip }),
        ...(req.headers['user-agent'] === undefined
          ? {}
          : { userAgent: req.headers['user-agent'] }),
      });

      reply.setCookie(SESSION_COOKIE, session.id, {
        ...cookieOptions,
        expires: new Date(session.expires_at),
      });

      recordAudit(db, {
        actorType: 'user',
        actorId: user.id,
        action: 'auth.login',
        outcome: 'allowed',
        ip,
      });

      return { user: toPublicUser(user) };
    },
  );

  app.post(
    '/api/v1/auth/logout',
    {
      schema: {
        tags: ['auth'],
        summary: 'Destroy the current session',
        response: { 204: { type: 'null' } },
      },
    },
    async (req, reply) => {
      const sessionId = req.cookies[SESSION_COOKIE];
      if (sessionId) {
        destroySession(app.auth, sessionId);
      }
      reply.clearCookie(SESSION_COOKIE, { path: '/' });

      if (req.principal) {
        recordAudit(db, {
          actorType: req.principal.kind === 'token' ? 'token' : 'user',
          actorId: req.principal.tokenId ?? req.principal.userId,
          action: 'auth.logout',
          outcome: 'allowed',
          ip: app.clientIp(req),
        });
      }

      return reply.code(204).send();
    },
  );

  /* ── Identity ────────────────────────────────────────────────────────── */

  app.get(
    '/api/v1/auth/me',
    {
      preHandler: app.authenticate,
      schema: {
        tags: ['auth'],
        summary: 'The authenticated principal',
        response: { 200: { type: 'object', properties: { user: Ref('User') } } },
      },
    },
    async (req) => {
      const principal = req.principal!;
      const row = db.prepare('SELECT * FROM users WHERE id = ?').get(principal.userId) as
        | Parameters<typeof toPublicUser>[0]
        | undefined;
      return { user: toPublicUser(row!) };
    },
  );

  /* ── API tokens (CLI) ────────────────────────────────────────────────── */

  app.get(
    '/api/v1/tokens',
    {
      preHandler: app.authenticate,
      schema: {
        tags: ['auth'],
        summary: 'List the caller\'s API tokens',
        response: {
          200: { type: 'array', items: Ref('ApiToken') },
        },
      },
    },
    async (req) => {
      const principal = req.principal!;
      return listApiTokens(db, principal.userId).map(toPublicToken);
    },
  );

  app.post(
    '/api/v1/tokens',
    {
      preHandler: app.authenticate,
      schema: {
        tags: ['auth'],
        summary: 'Issue a new API token for the CLI',
        body: Type.Object({ name: Type.String({ minLength: 1, maxLength: 64 }) }),
        response: { 201: Ref('CreatedApiToken'), 400: errorSchema() },
      },
    },
    async (req, reply) => {
      const principal = req.principal!;
      const { name } = req.body as { name: string };

      const { token, row } = createApiToken(app.auth, principal.userId, name);

      recordAudit(db, {
        actorType: principal.kind === 'token' ? 'token' : 'user',
        actorId: principal.tokenId ?? principal.userId,
        action: 'token.create',
        target: name,
        outcome: 'allowed',
        ip: app.clientIp(req),
      });

      return reply.code(201).send({ token, api_token: toPublicToken(row) });
    },
  );

  app.delete(
    '/api/v1/tokens/{id}',
    {
      preHandler: app.authenticate,
      schema: {
        tags: ['auth'],
        summary: 'Revoke an API token',
        params: Type.Object({ id: Type.String({ format: 'uuid' }) }),
        response: { 204: { type: 'null' }, 404: errorSchema() },
      },
    },
    async (req, reply) => {
      const principal = req.principal!;
      const { id } = req.params as { id: string };

      if (!revokeApiToken(db, principal.userId, id)) {
        return reply.code(404).send(errorBody(404, 'Token not found'));
      }

      recordAudit(db, {
        actorType: principal.kind === 'token' ? 'token' : 'user',
        actorId: principal.tokenId ?? principal.userId,
        action: 'token.revoke',
        target: id,
        outcome: 'allowed',
        ip: app.clientIp(req),
      });

      return reply.code(204).send();
    },
  );

  /* ── Audit log ───────────────────────────────────────────────────────── */

  app.get(
    '/api/v1/audit',
    {
      preHandler: [app.authenticate, app.requireRole('admin')],
      schema: {
        tags: ['auth'],
        summary: 'Read the audit log',
        querystring: Type.Object({
          limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 500, default: 100 })),
        }),
        response: { 200: { type: 'array', items: Ref('AuditEntry') } },
      },
    },
    async (req) => {
      const { limit } = req.query as { limit: number };
      const rows = db
        .prepare('SELECT * FROM audit_log ORDER BY ts DESC LIMIT ?')
        .all(limit) as Array<Record<string, unknown>>;

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

  /* ── First-run bootstrap ─────────────────────────────────────────────── */

  // Only usable while no users exist; becomes inert immediately afterwards.
  app.post(
    '/api/v1/auth/bootstrap',
    {
      schema: {
        tags: ['auth'],
        summary: 'Create the first administrator (refused once any user exists)',
        body: Type.Object({
          email: Type.String({ format: 'email' }),
          // Floor lowered to 8 by operator decision (2026-10-19). The stored
          // hash is argon2id regardless; this only bounds the input length.
          password: Type.String({ minLength: 8, maxLength: 200 }),
        }),
        response: { 201: { type: 'object', properties: { user: Ref('User') } }, 409: errorSchema() },
      },
    },
    async (req, reply) => {
      if (countUsers(db) > 0) {
        return reply.code(409).send(errorBody(409, 'Bootstrap is already complete'));
      }

      const { email, password } = req.body as { email: string; password: string };
      const user = await createUser(app.auth, email, password, 'admin');

      recordAudit(db, {
        actorType: 'user',
        actorId: user.id,
        action: 'auth.bootstrap',
        outcome: 'allowed',
        ip: app.clientIp(req),
      });

      return reply.code(201).send({ user: toPublicUser(user) });
    },
  );
}

function errorSchema() {
  return {
    type: 'object',
    properties: {
      error: { type: 'string' },
      message: { type: 'string' },
      details: {},
    },
  } as const;
}
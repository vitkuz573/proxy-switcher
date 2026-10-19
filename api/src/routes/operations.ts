import { Type } from '@sinclair/typebox';
import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify';
import { EngineError } from '../engine/client.js';
import { describeEngineError } from '../plugins/errors.js';
import { recordAudit } from '../services/auth.js';
import {
  AnonymitySchema,
  ProxyProtocolSchema,
  Ref,
} from '../schemas/index.js';

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

/**
 * Outcome of an engine call. Modelled as a discriminated union rather than by
 * inspecting the reply, so route handlers stay type-safe.
 */
type EngineOutcome<T> = { ok: true; value: T } | { ok: false; reply: FastifyReply };

async function callEngine<T>(reply: FastifyReply, fn: () => Promise<T>): Promise<EngineOutcome<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (err) {
    if (err instanceof EngineError) {
      const { status, body } = describeEngineError(err);
      return { ok: false, reply: reply.code(status).send(body) };
    }
    throw err;
  }
}

function audit(app: FastifyInstance, req: FastifyRequest, action: string, target: string | null): void {
  const principal = req.principal;
  if (!principal) {
    return;
  }
  recordAudit(app.auth.db, {
    actorType: principal.kind === 'token' ? 'token' : 'user',
    actorId: principal.tokenId ?? principal.userId,
    action,
    target,
    outcome: 'allowed',
    ip: app.clientIp(req),
  });
}

export async function operationRoutes(app: FastifyInstance): Promise<void> {
  /* ── Read-only telemetry ─────────────────────────────────────────────── */

  app.get(
    '/api/v1/status',
    {
      preHandler: [app.authenticate, app.requireRole('viewer')],
      schema: {
        tags: ['status'],
        summary: 'Active proxy and pool size',
        response: { 200: Ref('Status'), 502: errorSchema() },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getStatus());
      return result.ok ? result.value : result.reply;
    },
  );

  app.get(
    '/api/v1/stats',
    {
      preHandler: [app.authenticate, app.requireRole('viewer')],
      schema: {
        tags: ['status'],
        summary: 'Live connection and pool counters',
        response: { 200: Ref('Stats'), 502: errorSchema() },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getStats());
      return result.ok ? result.value : result.reply;
    },
  );

  /* ── Proxies ─────────────────────────────────────────────────────────── */

  app.get(
    '/api/v1/proxies',
    {
      preHandler: [app.authenticate, app.requireRole('viewer')],
      schema: {
        tags: ['proxies'],
        summary: 'List every proxy in the engine pool',
        response: { 200: { type: 'array', items: Ref('Proxy') }, 502: errorSchema() },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.listProxies());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    '/api/v1/proxies',
    {
      preHandler: [app.authenticate, app.requireRole('operator')],
      schema: {
        tags: ['proxies'],
        summary: 'Add a proxy manually',
        body: Type.Object({
          host: Type.String({ minLength: 1, maxLength: 255 }),
          port: Type.Integer({ minimum: 1, maximum: 65535 }),
          protocol: ProxyProtocolSchema,
          anonymity: Type.Optional(AnonymitySchema),
          country: Type.Optional(Type.String({ maxLength: 64 })),
        }),
        response: { 201: Ref('Proxy'), 400: errorSchema(), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const body = req.body as {
        host: string;
        port: number;
        protocol: 'http' | 'https' | 'socks4' | 'socks5';
        anonymity?: 'transparent' | 'anonymous' | 'elite' | 'unknown';
        country?: string;
      };

      const result = await callEngine(reply, () => app.engine.addProxy(body));
      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, 'proxy.add', `${body.host}:${body.port}`);
      return reply.code(201).send(result.value);
    },
  );

  app.delete(
    '/api/v1/proxies/{id}',
    {
      preHandler: [app.authenticate, app.requireRole('operator')],
      schema: {
        tags: ['proxies'],
        summary: 'Remove a proxy from the pool',
        params: Type.Object({ id: Type.String({ minLength: 1 }) }),
        response: { 204: { type: 'null' }, 404: errorSchema(), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const { id } = req.params as { id: string };
      const result = await callEngine(reply, () => app.engine.deleteProxy(id));

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, 'proxy.delete', id);
      return reply.code(204).send();
    },
  );

  app.post(
    '/api/v1/proxies/switch',
    {
      preHandler: [app.authenticate, app.requireRole('operator')],
      schema: {
        tags: ['proxies'],
        summary: 'Route traffic through a specific proxy',
        body: Type.Object({ id: Type.String({ minLength: 1 }) }),
        response: { 200: Ref('Status'), 404: errorSchema(), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const { id } = req.body as { id: string };
      const result = await callEngine(reply, () => app.engine.switchProxy(id));

      if (result.ok) {
        audit(app, req, 'proxy.switch', id);
        return result.value;
      }
      return result.reply;
    },
  );

  app.post(
    '/api/v1/proxies/rotate',
    {
      preHandler: [app.authenticate, app.requireRole('operator')],
      schema: {
        tags: ['proxies'],
        summary: 'Move to the next proxy in the pool',
        response: { 200: Ref('Status'), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const result = await callEngine(reply, () => app.engine.rotate());

      if (result.ok) {
        audit(app, req, 'proxy.rotate', null);
        return result.value;
      }
      return result.reply;
    },
  );

  /* ── Scrape sources ──────────────────────────────────────────────────── */

  app.get(
    '/api/v1/sources',
    {
      preHandler: [app.authenticate, app.requireRole('viewer')],
      schema: {
        tags: ['scraper'],
        summary: 'List scrape sources',
        response: { 200: { type: 'array', items: Ref('Source') }, 502: errorSchema() },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.listSources());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    '/api/v1/sources',
    {
      preHandler: [app.authenticate, app.requireRole('admin')],
      schema: {
        tags: ['scraper'],
        summary: 'Add a scrape source',
        body: Type.Object({ url: Type.String({ minLength: 1, format: 'uri' }) }),
        response: { 201: Ref('Source'), 400: errorSchema(), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const { url } = req.body as { url: string };
      const result = await callEngine(reply, () => app.engine.addSource(url));

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, 'source.add', url);
      return reply.code(201).send(result.value);
    },
  );

  app.delete(
    '/api/v1/sources/{url}',
    {
      preHandler: [app.authenticate, app.requireRole('admin')],
      schema: {
        tags: ['scraper'],
        summary: 'Remove a scrape source',
        params: Type.Object({ url: Type.String({ minLength: 1 }) }),
        response: { 204: { type: 'null' }, 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const { url } = req.params as { url: string };
      const result = await callEngine(reply, () => app.engine.deleteSource(url));

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, 'source.delete', url);
      return reply.code(204).send();
    },
  );

  app.get(
    '/api/v1/scrape/status',
    {
      preHandler: [app.authenticate, app.requireRole('viewer')],
      schema: {
        tags: ['scraper'],
        summary: 'Scrape state and live health-check progress',
        response: { 200: Ref('ScrapeStatus'), 502: errorSchema() },
      },
    },
    async (_req, reply) => {
      const result = await callEngine(reply, () => app.engine.getScrapeStatus());
      return result.ok ? result.value : result.reply;
    },
  );

  app.post(
    '/api/v1/scrape',
    {
      preHandler: [app.authenticate, app.requireRole('operator')],
      schema: {
        tags: ['scraper'],
        summary: 'Trigger a scrape cycle now',
        response: { 202: { type: 'null' }, 409: errorSchema(), 502: errorSchema() },
      },
    },
    async (req, reply) => {
      const result = await callEngine(reply, () => app.engine.triggerScrape());

      if (!result.ok) {
        return result.reply;
      }

      audit(app, req, 'scrape.trigger', null);
      return reply.code(202).send();
    },
  );
}
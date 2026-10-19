import type { FastifyReply, FastifyRequest } from 'fastify';
import fp from 'fastify-plugin';
import { recordAudit } from '../services/auth.js';
import type { Principal, Role } from './auth.js';

/** Roles are ordered; each one inherits everything the lower role may do. */
const RANK: Record<Role, number> = { viewer: 1, operator: 2, admin: 3 };

export function roleAtLeast(actual: Role, required: Role): boolean {
  return RANK[actual] >= RANK[required];
}

declare module 'fastify' {
  interface FastifyInstance {
    requireRole: (required: Role) => (req: FastifyRequest, reply: FastifyReply) => Promise<void>;
  }
}

export const rbacPlugin = fp(async (app) => {
  app.decorate('requireRole', (required: Role) => {
    return async (req: FastifyRequest, reply: FastifyReply) => {
      const principal = req.principal;

      if (!principal) {
        await reply.code(401).send({ error: 'unauthorized', message: 'Authentication required' });
        return;
      }

      if (!roleAtLeast(principal.role, required)) {
        // Denials are audited: a repeatedly failing principal is a signal.
        recordAudit(app.auth.db, {
          actorType: principal.kind === 'token' ? 'token' : 'user',
          actorId: principal.tokenId ?? principal.userId,
          action: req.routeOptions?.url ?? 'unknown',
          outcome: 'denied',
          ip: app.clientIp(req),
          detail: `requires ${required}, principal is ${principal.role}`,
        });

        await reply.code(403).send({
          error: 'forbidden',
          message: `This action requires the ${required} role`,
        });
      }
    };
  });
});

export type { Principal };
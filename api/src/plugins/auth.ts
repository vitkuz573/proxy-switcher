import type { FastifyReply, FastifyRequest } from "fastify";
import fp from "fastify-plugin";
import type { AuthContext } from "../services/auth.js";
import { resolveSession, verifyApiToken } from "../services/auth.js";
import { PROBLEM_CONTENT_TYPE, problem } from "./errors.js";

export const SESSION_COOKIE = "ps_session";

export type Role = "admin" | "operator" | "viewer";

/**
 * Whoever is behind a request. `kind` distinguishes the two client models so
 * audit records stay meaningful when a CLI and a browser act identically.
 */
export interface Principal {
  userId: string;
  email: string;
  role: Role;
  kind: "session" | "token";
  tokenId?: string;
}

declare module "fastify" {
  interface FastifyRequest {
    principal?: Principal;
  }
  interface FastifyInstance {
    auth: AuthContext;
    authenticate: (req: FastifyRequest, reply: FastifyReply) => Promise<void>;
    clientIp: (req: FastifyRequest) => string | null;
  }
}

function clientIp(req: FastifyRequest): string | null {
  return req.ip ?? null;
}

function bearerToken(req: FastifyRequest): string | null {
  const header = req.headers.authorization;
  if (!header?.startsWith("Bearer ")) {
    return null;
  }
  const token = header.slice("Bearer ".length).trim();
  return token.length > 0 ? token : null;
}

export const authPlugin = fp(async (app, opts: { auth: AuthContext }) => {
  const ctx = opts.auth;

  app.decorate("auth", ctx);

  /**
   * Establishes the caller's identity or rejects the request.
   *
   * Accepts either trust model:
   *   - Web: `ps_session` cookie, set by the login route
   *   - CLI: `Authorization: Bearer ps_<prefix>_<secret>`
   *
   * Always ends in a 401 unless a valid principal was attached. Routes that
   * must stay public (`/health`, `/ready`, `/auth/login`, `/auth/bootstrap`)
   * simply do not list it as a preHandler.
   */
  app.decorate("authenticate", async (req: FastifyRequest, reply: FastifyReply) => {
    const token = bearerToken(req);

    if (token) {
      const resolved = verifyApiToken(ctx, token);
      if (!resolved) {
        await reply
          .code(401)
          .type(PROBLEM_CONTENT_TYPE)
          .send(problem(401, "Invalid or expired API token", { instance: req.url }));
        return;
      }
      req.principal = {
        userId: resolved.user.id,
        email: resolved.user.email,
        role: resolved.user.role,
        kind: "token",
        tokenId: resolved.token.id,
      };
      return;
    }

    const sessionId = req.cookies[SESSION_COOKIE];
    if (sessionId) {
      const resolved = resolveSession(ctx, sessionId);
      if (resolved) {
        req.principal = {
          userId: resolved.user.id,
          email: resolved.user.email,
          role: resolved.user.role,
          kind: "session",
        };
        return;
      }
      // Stale cookie: clear it so the browser stops sending it.
      reply.clearCookie(SESSION_COOKIE, { path: "/" });
    }

    await reply
      .code(401)
      .type(PROBLEM_CONTENT_TYPE)
      .send(problem(401, "Authentication required", { instance: req.url }));
  });

  // Exposed for login/logout routes.
  app.decorate("clientIp", clientIp);
});

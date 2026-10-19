import fp from 'fastify-plugin';
import type { EngineClient } from '../engine/client.js';

declare module 'fastify' {
  interface FastifyInstance {
    engine: EngineClient;
  }
}

/**
 * The Rust daemon is an internal dependency, not a client-facing service. It is
 * attached to the instance so route handlers never build URLs themselves.
 */
export const enginePlugin = fp(async (app, opts: { engine: EngineClient }) => {
  app.decorate('engine', opts.engine);
});